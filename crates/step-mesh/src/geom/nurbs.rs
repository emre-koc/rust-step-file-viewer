//! Rational B-spline curves and surfaces (Piegl & Tiller conventions).
//!
//! Control points are stored homogeneous (`wx, wy, wz, w`); non-rational data uses `w = 1`.
//! Knot vectors are stored *expanded* (multiplicities unrolled), so `knots.len() == n + deg + 1`.

use glam::{DVec3, DVec4};

#[derive(Clone, Debug)]
pub struct NurbsCurve {
    pub degree: usize,
    /// Homogeneous control points, `n` of them.
    pub ctrl: Vec<DVec4>,
    /// Expanded knots, `n + degree + 1`.
    pub knots: Vec<f64>,
    /// Whether the curve is geometrically closed (first and last control points coincide).
    pub closed: bool,
}

#[derive(Clone, Debug)]
pub struct NurbsSurface {
    pub degree: [usize; 2],
    /// Number of control points in u and v.
    pub n: [usize; 2],
    /// Homogeneous control points, row-major: index `i * n[1] + j` for `(u_i, v_j)`.
    pub ctrl: Vec<DVec4>,
    /// Expanded knots per direction.
    pub knots: [Vec<f64>; 2],
    /// Geometric closure per direction, detected from the control net rather than trusted from the
    /// file's `closed_u/closed_v` flags.
    pub closed: [bool; 2],
}

impl NurbsCurve {
    /// Build from STEP `B_SPLINE_CURVE_WITH_KNOTS` data: control points, optional weights,
    /// knot multiplicities and knot values.
    pub fn from_step(
        degree: usize,
        ctrl: &[DVec3],
        weights: Option<&[f64]>,
        knot_mults: &[i64],
        knots: &[f64],
    ) -> NurbsCurve {
        let ctrl: Vec<DVec4> = ctrl
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let w = weights.and_then(|w| w.get(i).copied()).unwrap_or(1.0);
                DVec4::new(p.x * w, p.y * w, p.z * w, w)
            })
            .collect();
        let knots = expand_knots(knot_mults, knots);
        let closed = ctrl.len() > 2 && point_of(ctrl[0]).distance(point_of(ctrl[ctrl.len() - 1])) < 1e-9;
        NurbsCurve { degree, ctrl, knots, closed }
    }

    pub fn domain(&self) -> (f64, f64) {
        let p = self.degree;
        if self.knots.len() < 2 * p + 2 {
            return (0.0, 1.0);
        }
        (self.knots[p], self.knots[self.knots.len() - 1 - p])
    }

    pub fn point(&self, t: f64) -> DVec3 {
        point_of(self.homogeneous(t))
    }

    pub fn derivative(&self, t: f64) -> DVec3 {
        let (c, dc) = self.homogeneous_derivs(t);
        rational_first_derivative(c, dc)
    }

    /// Control point `i`, clamped so malformed knot data cannot panic.
    #[inline]
    fn cp(&self, i: usize) -> DVec4 {
        self.ctrl.get(i.min(self.ctrl.len().saturating_sub(1))).copied().unwrap_or(DVec4::W)
    }

    /// Homogeneous point `C^w(t)`.
    pub fn homogeneous(&self, t: f64) -> DVec4 {
        let (span, basis) = basis_functions(&self.knots, self.degree, t);
        let mut acc = DVec4::ZERO;
        // Only the first `degree + 1` entries of the fixed-size basis array are meaningful; the
        // remaining slots are padding and must not be used to index the control points.
        for (k, b) in basis.iter().take(self.degree + 1).enumerate() {
            acc += self.cp(span - self.degree + k) * *b;
        }
        acc
    }

    /// Homogeneous point and first derivative.
    pub fn homogeneous_derivs(&self, t: f64) -> (DVec4, DVec4) {
        let (span, ders) = basis_derivatives(&self.knots, self.degree, t, 1);
        let mut c = DVec4::ZERO;
        let mut dc = DVec4::ZERO;
        for k in 0..=self.degree {
            let cp = self.cp(span - self.degree + k);
            c += cp * ders[0][k];
            dc += cp * ders[1][k];
        }
        (c, dc)
    }
}

impl NurbsSurface {
    /// Build from STEP `B_SPLINE_SURFACE_WITH_KNOTS` data. `ctrl` is the list of rows from the file
    /// (`n_u` rows of `n_v` points); `weights` has the same layout when present.
    pub fn from_step(
        degree: [usize; 2],
        ctrl_rows: &[Vec<DVec3>],
        weight_rows: Option<&[Vec<f64>]>,
        u_mults: &[i64],
        u_knots: &[f64],
        v_mults: &[i64],
        v_knots: &[f64],
    ) -> NurbsSurface {
        let n_u = ctrl_rows.len();
        let n_v = ctrl_rows.first().map_or(0, |r| r.len());
        let mut ctrl = Vec::with_capacity(n_u * n_v);
        for (i, row) in ctrl_rows.iter().enumerate() {
            for (j, p) in row.iter().enumerate() {
                let w = weight_rows
                    .and_then(|ws| ws.get(i))
                    .and_then(|r| r.get(j))
                    .copied()
                    .unwrap_or(1.0);
                ctrl.push(DVec4::new(p.x * w, p.y * w, p.z * w, w));
            }
        }
        let knots = [expand_knots(u_mults, u_knots), expand_knots(v_mults, v_knots)];
        let closed_u = n_u > 2
            && (0..n_v).all(|j| point_of(ctrl[j]).distance(point_of(ctrl[(n_u - 1) * n_v + j])) < 1e-9);
        let closed_v = n_v > 2
            && (0..n_u).all(|i| point_of(ctrl[i * n_v]).distance(point_of(ctrl[i * n_v + n_v - 1])) < 1e-9);
        NurbsSurface { degree, n: [n_u, n_v], ctrl, knots, closed: [closed_u, closed_v] }
    }

    pub fn domain(&self) -> [(f64, f64); 2] {
        let [p, q] = self.degree;
        let d = |k: &[f64], p: usize| -> (f64, f64) {
            if k.len() < 2 * p + 2 { (0.0, 1.0) } else { (k[p], k[k.len() - 1 - p]) }
        };
        [d(&self.knots[0], p), d(&self.knots[1], q)]
    }

    /// Control point `(i, j)`, clamped to the net so a malformed knot vector cannot panic.
    #[inline]
    pub fn ctrl_at(&self, i: usize, j: usize) -> DVec4 {
        let i = i.min(self.n[0].saturating_sub(1));
        let j = j.min(self.n[1].saturating_sub(1));
        self.ctrl.get(i * self.n[1] + j).copied().unwrap_or(DVec4::W)
    }

    pub fn point(&self, u: f64, v: f64) -> DVec3 {
        point_of(self.homogeneous(u, v))
    }

    /// Homogeneous point `S^w(u,v)`.
    pub fn homogeneous(&self, u: f64, v: f64) -> DVec4 {
        let (su, bu) = basis_functions(&self.knots[0], self.degree[0], u);
        let (sv, bv) = basis_functions(&self.knots[1], self.degree[1], v);
        let mut acc = DVec4::ZERO;
        // As in `NurbsCurve::homogeneous`: the basis arrays are fixed-size buffers, only
        // `degree + 1` entries are live.
        for (k, bku) in bu.iter().take(self.degree[0] + 1).enumerate() {
            let i = su - self.degree[0] + k;
            let mut row = DVec4::ZERO;
            for (l, blv) in bv.iter().take(self.degree[1] + 1).enumerate() {
                row += self.ctrl_at(i, sv - self.degree[1] + l) * *blv;
            }
            acc += row * *bku;
        }
        acc
    }

    /// Point plus first partial derivatives `(S, S_u, S_v)` in Cartesian space.
    pub fn derivs1(&self, u: f64, v: f64) -> (DVec3, DVec3, DVec3) {
        let d = self.homogeneous_derivs(u, v, 1);
        let s = point_of(d[0][0]);
        let su = rational_first_derivative(d[0][0], d[1][0]);
        let sv = rational_first_derivative(d[0][0], d[0][1]);
        (s, su, sv)
    }

    /// Point, first and second partial derivatives in Cartesian space:
    /// `(S, S_u, S_v, S_uu, S_uv, S_vv)`; used by Newton point inversion.
    pub fn derivs2(&self, u: f64, v: f64) -> (DVec3, DVec3, DVec3, DVec3, DVec3, DVec3) {
        let a = self.homogeneous_derivs(u, v, 2);
        // Piegl & Tiller A4.4, generalised to second order.
        let w = a[0][0].w;
        let s = a[0][0].truncate() / w;
        let su = (a[1][0].truncate() - s * a[1][0].w) / w;
        let sv = (a[0][1].truncate() - s * a[0][1].w) / w;
        let suu = (a[2][0].truncate() - su * (2.0 * a[1][0].w) - s * a[2][0].w) / w;
        let svv = (a[0][2].truncate() - sv * (2.0 * a[0][1].w) - s * a[0][2].w) / w;
        let suv = (a[1][1].truncate() - su * a[0][1].w - sv * a[1][0].w - s * a[1][1].w) / w;
        (s, su, sv, suu, suv, svv)
    }

    /// Homogeneous partial derivatives `d[k][l] = ∂^{k+l} S^w / ∂u^k ∂v^l` for `k + l ≤ order`
    /// (entries with `k + l > order` are zero). `order ≤ 2`.
    pub fn homogeneous_derivs(&self, u: f64, v: f64, order: usize) -> [[DVec4; 3]; 3] {
        let (su, du) = basis_derivatives(&self.knots[0], self.degree[0], u, order);
        let (sv, dv) = basis_derivatives(&self.knots[1], self.degree[1], v, order);
        let mut out = [[DVec4::ZERO; 3]; 3];
        for k in 0..=order {
            for l in 0..=(order - k) {
                let mut acc = DVec4::ZERO;
                for a in 0..=self.degree[0] {
                    let i = su - self.degree[0] + a;
                    let mut row = DVec4::ZERO;
                    for b in 0..=self.degree[1] {
                        row += self.ctrl_at(i, sv - self.degree[1] + b) * dv[l][b];
                    }
                    acc += row * du[k][a];
                }
                out[k][l] = acc;
            }
        }
        out
    }
}

#[inline]
pub fn point_of(h: DVec4) -> DVec3 {
    if h.w != 0.0 { h.truncate() / h.w } else { h.truncate() }
}

#[inline]
fn rational_first_derivative(c: DVec4, dc: DVec4) -> DVec3 {
    // C = A/w  ⇒  C' = (A' − C·w') / w
    let p = point_of(c);
    (dc.truncate() - p * dc.w) / c.w
}

/// Unroll `(multiplicity, knot)` pairs into a flat knot vector.
pub fn expand_knots(mults: &[i64], knots: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(mults.iter().map(|m| (*m).max(0) as usize).sum());
    for (m, k) in mults.iter().zip(knots) {
        for _ in 0..(*m).max(0) {
            out.push(*k);
        }
    }
    out
}

/// Knot span index for parameter `t` (Piegl & Tiller A2.1), clamped to the valid domain.
pub fn find_span(knots: &[f64], degree: usize, t: f64) -> usize {
    // Malformed data from a broken file must not panic; fall back to the first legal span.
    if knots.len() < degree + 2 || !t.is_finite() {
        return degree;
    }
    let n = knots.len() - degree - 2; // last control point index
    if t >= knots[n + 1] {
        return n;
    }
    if t <= knots[degree] {
        return degree;
    }
    let (mut lo, mut hi) = (degree, n + 1);
    let mut mid = (lo + hi) / 2;
    while t < knots[mid] || t >= knots[mid + 1] {
        if t < knots[mid] { hi = mid } else { lo = mid }
        mid = (lo + hi) / 2;
    }
    mid
}

/// Non-zero basis functions at `t` (A2.2). Returns `(span, N[0..=degree])`.
pub fn basis_functions(knots: &[f64], degree: usize, t: f64) -> (usize, [f64; 8]) {
    debug_assert!(degree < 8, "degree ≥ 8 unsupported");
    let span = find_span(knots, degree, t);
    let mut n = [0.0f64; 8];
    let mut left = [0.0f64; 8];
    let mut right = [0.0f64; 8];
    n[0] = 1.0;
    for j in 1..=degree {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let denom = right[r + 1] + left[j - r];
            let temp = if denom != 0.0 { n[r] / denom } else { 0.0 };
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    (span, n)
}

/// Basis functions and derivatives up to `order` (A2.3). `ders[k][j]` is the k-th derivative of the
/// j-th non-zero basis function. Rows beyond `order` are zero.
pub fn basis_derivatives(knots: &[f64], degree: usize, t: f64, order: usize) -> (usize, [[f64; 8]; 3]) {
    debug_assert!(degree < 8 && order <= 2);
    let span = find_span(knots, degree, t);
    let p = degree;
    let mut ndu = [[0.0f64; 8]; 8];
    let mut left = [0.0f64; 8];
    let mut right = [0.0f64; 8];
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            ndu[j][r] = right[r + 1] + left[j - r];
            let temp = if ndu[j][r] != 0.0 { ndu[r][j - 1] / ndu[j][r] } else { 0.0 };
            ndu[r][j] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        ndu[j][j] = saved;
    }
    let mut ders = [[0.0f64; 8]; 3];
    for j in 0..=p {
        ders[0][j] = ndu[j][p];
    }
    let order = order.min(p);
    let mut a = [[0.0f64; 8]; 2];
    for r in 0..=p {
        let (mut s1, mut s2) = (0usize, 1usize);
        a[0][0] = 1.0;
        for k in 1..=order {
            let mut d = 0.0;
            let rk = r as isize - k as isize;
            let pk = p - k;
            if r >= k {
                a[s2][0] = a[s1][0] / ndu[pk + 1][rk as usize];
                d = a[s2][0] * ndu[rk as usize][pk];
            }
            let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
            let j2 = if (r as isize - 1) <= pk as isize { k - 1 } else { p - r };
            for j in j1..=j2 {
                a[s2][j] = (a[s1][j] - a[s1][j - 1]) / ndu[pk + 1][(rk + j as isize) as usize];
                d += a[s2][j] * ndu[(rk + j as isize) as usize][pk];
            }
            if r <= pk {
                a[s2][k] = -a[s1][k - 1] / ndu[pk + 1][r];
                d += a[s2][k] * ndu[r][pk];
            }
            ders[k][r] = d;
            std::mem::swap(&mut s1, &mut s2);
        }
    }
    let mut r = p as f64;
    for k in 1..=order {
        for j in 0..=p {
            ders[k][j] *= r;
        }
        r *= (p - k) as f64;
    }
    (span, ders)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quarter_circle() -> NurbsCurve {
        // Rational quadratic quarter circle of radius 1 in the xy plane.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        NurbsCurve::from_step(
            2,
            &[DVec3::X, DVec3::new(1.0, 1.0, 0.0), DVec3::Y],
            Some(&[1.0, w, 1.0]),
            &[3, 3],
            &[0.0, 1.0],
        )
    }

    #[test]
    fn rational_quarter_circle_lies_on_unit_circle() {
        let c = quarter_circle();
        for i in 0..=20 {
            let t = i as f64 / 20.0;
            let p = c.point(t);
            assert!((p.length() - 1.0).abs() < 1e-12, "t={t} |p|={}", p.length());
            // derivative is tangent to the circle
            let d = c.derivative(t);
            assert!(p.dot(d).abs() < 1e-9);
        }
    }

    /// Rational quarter-cylinder patch of radius 1 (quadratic in u, linear in v).
    fn rational_quarter_cylinder() -> NurbsSurface {
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let rows = vec![
            vec![DVec3::new(1.0, 0.0, 0.0), DVec3::new(1.0, 0.0, 2.0)],
            vec![DVec3::new(1.0, 1.0, 0.0), DVec3::new(1.0, 1.0, 2.0)],
            vec![DVec3::new(0.0, 1.0, 0.0), DVec3::new(0.0, 1.0, 2.0)],
        ];
        let weights = vec![vec![1.0, 1.0], vec![w, w], vec![1.0, 1.0]];
        NurbsSurface::from_step(
            [2, 1],
            &rows,
            Some(&weights),
            &[3, 3],
            &[0.0, 1.0],
            &[2, 2],
            &[0.0, 1.0],
        )
    }

    #[test]
    fn rational_surface_lies_on_the_cylinder() {
        let s = rational_quarter_cylinder();
        for i in 0..=8 {
            for j in 0..=4 {
                let (u, v) = (i as f64 / 8.0, j as f64 / 4.0);
                let p = s.point(u, v);
                assert!((p.x.hypot(p.y) - 1.0).abs() < 1e-12, "radius {}", p.x.hypot(p.y));
                assert!((p.z - 2.0 * v).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn rational_second_derivatives_match_finite_differences() {
        let s = rational_quarter_cylinder();
        let h = 1e-5;
        for i in 1..8 {
            for j in 1..4 {
                let (u, v) = (i as f64 / 8.0, j as f64 / 4.0);
                let (_, su, sv, suu, suv, svv) = s.derivs2(u, v);
                let (_, su_p, _) = s.derivs1(u + h, v);
                let (_, su_m, _) = s.derivs1(u - h, v);
                let (_, _, sv_p) = s.derivs1(u, v + h);
                let (_, _, sv_m) = s.derivs1(u, v - h);
                let (_, su_vp, _) = s.derivs1(u, v + h);
                let (_, su_vm, _) = s.derivs1(u, v - h);
                assert!((su - (s.point(u + h, v) - s.point(u - h, v)) / (2.0 * h)).length() < 1e-6);
                assert!((sv - (s.point(u, v + h) - s.point(u, v - h)) / (2.0 * h)).length() < 1e-6);
                assert!(
                    (suu - (su_p - su_m) / (2.0 * h)).length() < 1e-4,
                    "S_uu {suu} vs {}",
                    (su_p - su_m) / (2.0 * h)
                );
                assert!((svv - (sv_p - sv_m) / (2.0 * h)).length() < 1e-4);
                assert!(
                    (suv - (su_vp - su_vm) / (2.0 * h)).length() < 1e-4,
                    "S_uv {suv} vs {}",
                    (su_vp - su_vm) / (2.0 * h)
                );
            }
        }
    }

    #[test]
    fn bilinear_surface_and_derivatives() {
        let rows = vec![
            vec![DVec3::new(0.0, 0.0, 0.0), DVec3::new(0.0, 2.0, 0.0)],
            vec![DVec3::new(3.0, 0.0, 0.0), DVec3::new(3.0, 2.0, 1.0)],
        ];
        let s = NurbsSurface::from_step([1, 1], &rows, None, &[2, 2], &[0.0, 1.0], &[2, 2], &[0.0, 1.0]);
        let (p, su, sv) = s.derivs1(0.5, 0.5);
        assert!((p - DVec3::new(1.5, 1.0, 0.25)).length() < 1e-12);
        assert!((su - DVec3::new(3.0, 0.0, 0.5)).length() < 1e-12);
        assert!((sv - DVec3::new(0.0, 2.0, 0.5)).length() < 1e-12);
        let (_, _, _, suu, suv, svv) = s.derivs2(0.5, 0.5);
        assert!(suu.length() < 1e-12 && svv.length() < 1e-12);
        assert!((suv - DVec3::new(0.0, 0.0, 1.0)).length() < 1e-12);
    }
}
