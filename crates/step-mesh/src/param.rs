//! Surface parametrisation: evaluation, analytic normals, point inversion, periodicity,
//! near-isometric uv scaling and curvature radii.
//!
//! Every entry point is total: no `unwrap`, no unguarded `normalize`, no division by a value that
//! can legitimately be zero. Point inversion returns the seed (and reports non-convergence) rather
//! than failing.
//!
//! Parametrisations follow the table in [`crate::geom::Surface`].

use std::sync::OnceLock;

use glam::{DVec2, DVec3};

use crate::geom::{Curve3, Frame, Surface};
use crate::mesh::SurfaceKind;

pub(crate) const TAU: f64 = std::f64::consts::TAU;
/// Newton convergence threshold in mm.
const NEWTON_EPS: f64 = 1e-9;
const NEWTON_ITERS: usize = 12;

pub(crate) fn surface_kind(s: &Surface) -> SurfaceKind {
    match s {
        Surface::Plane { .. } => SurfaceKind::Plane,
        Surface::Cylinder { .. } => SurfaceKind::Cylinder,
        Surface::Cone { .. } => SurfaceKind::Cone,
        Surface::Sphere { .. } => SurfaceKind::Sphere,
        Surface::Torus { .. } => SurfaceKind::Torus,
        Surface::Extrusion { .. } => SurfaceKind::Extrusion,
        Surface::Revolution { .. } => SurfaceKind::Revolution,
        Surface::Nurbs(_) => SurfaceKind::Nurbs,
        Surface::Unsupported(_) => SurfaceKind::Unsupported,
    }
}

/// Relative work weight used to sort faces so the expensive ones start first.
pub(crate) fn cost_rank(s: &Surface) -> u8 {
    match s {
        Surface::Nurbs(_) => 0,
        Surface::Revolution { .. } => 1,
        Surface::Torus { .. } | Surface::Sphere { .. } => 2,
        Surface::Extrusion { .. } => 3,
        Surface::Cone { .. } | Surface::Cylinder { .. } => 4,
        Surface::Plane { .. } => 5,
        Surface::Unsupported(_) => 6,
    }
}

#[inline]
pub(crate) fn safe_normalize(v: DVec3, fallback: DVec3) -> DVec3 {
    v.try_normalize().unwrap_or(fallback)
}

/// Rotate `v` about the unit `axis` by the angle whose cosine/sine are given (Rodrigues).
#[inline]
fn rotate_about(v: DVec3, axis: DVec3, ca: f64, sa: f64) -> DVec3 {
    v * ca + axis.cross(v) * sa + axis * (axis.dot(v) * (1.0 - ca))
}

/// Wrap `x` into `(-p/2, p/2]`.
#[inline]
pub(crate) fn wrap_half(x: f64, p: f64) -> f64 {
    if p <= 0.0 || !x.is_finite() {
        return x;
    }
    let mut r = x % p;
    if r > 0.5 * p {
        r -= p;
    } else if r < -0.5 * p {
        r += p;
    }
    r
}

/// Reduce `x` into `[lo, lo + p)`.
#[inline]
pub(crate) fn wrap_into(x: f64, lo: f64, p: f64) -> f64 {
    if p <= 0.0 || !x.is_finite() {
        return x;
    }
    let mut r = (x - lo) % p;
    if r < 0.0 {
        r += p;
    }
    lo + r
}

// --------------------------------------------------------------------------------------------
// curve helpers (shared by extrusion / revolution profiles and by edge discretisation)
// --------------------------------------------------------------------------------------------

/// Natural parameter domain of a curve. Lines are unbounded; a wide symmetric interval is used and
/// the extrusion inverse solves lines analytically anyway.
pub(crate) fn curve_domain(c: &Curve3) -> (f64, f64) {
    match c {
        Curve3::Line { .. } => (-1.0e6, 1.0e6),
        Curve3::Circle { .. } | Curve3::Ellipse { .. } => (0.0, TAU),
        Curve3::Nurbs(n) => n.domain(),
        Curve3::Polyline(p) => (0.0, (p.len().max(2) - 1) as f64),
    }
}

/// Second derivative by central differences (only needed for curvature estimates and the 1-D
/// Newton used by extrusion / revolution inversion).
fn curve_second_deriv(c: &Curve3, t: f64, h: f64) -> DVec3 {
    let (a, b) = curve_domain(c);
    let h = h.max(1e-7).min(((b - a) * 0.25).max(1e-7));
    let tm = (t - h).max(a);
    let tp = (t + h).min(b);
    if tp - tm < 1e-12 {
        return DVec3::ZERO;
    }
    (c.tangent(tp) - c.tangent(tm)) / (tp - tm)
}

/// Radius of curvature of a curve at `t`, or `None` where the curve is (locally) straight.
pub(crate) fn curve_radius(c: &Curve3, t: f64) -> Option<f64> {
    match c {
        Curve3::Line { .. } => None,
        Curve3::Circle { r, .. } => Some(*r),
        Curve3::Ellipse { a, b, .. } => {
            let (ca, sa) = (t.cos(), t.sin());
            let num = (a * a * sa * sa + b * b * ca * ca).powf(1.5);
            let den = (a * b).abs();
            if den > 1e-12 { Some(num / den) } else { None }
        }
        _ => {
            let d1 = c.tangent(t);
            let d2 = curve_second_deriv(c, t, (curve_domain(c).1 - curve_domain(c).0) * 1e-3);
            let n = d1.cross(d2).length();
            let s = d1.length();
            if n > 1e-14 && s > 1e-14 { Some(s * s * s / n) } else { None }
        }
    }
}

/// Uniform samples of a curve over its domain (used as Newton seeds).
#[derive(Debug)]
pub(crate) struct CurveSamples {
    pub ts: Vec<f64>,
    pub pts: Vec<DVec3>,
    /// Mean |C'| over the samples (uv scaling).
    pub mean_deriv: f64,
}

impl CurveSamples {
    fn build(c: &Curve3, n: usize) -> CurveSamples {
        let (a, b) = curve_domain(c);
        let n = n.max(2);
        let mut ts = Vec::with_capacity(n);
        let mut pts = Vec::with_capacity(n);
        let mut acc = 0.0;
        for i in 0..n {
            let t = a + (b - a) * (i as f64) / ((n - 1) as f64);
            ts.push(t);
            pts.push(c.point(t));
            acc += c.tangent(t).length();
        }
        CurveSamples { ts, pts, mean_deriv: (acc / n as f64).max(1e-12) }
    }

    fn nearest(&self, p: DVec3) -> f64 {
        let mut best = self.ts[0];
        let mut bd = f64::INFINITY;
        for (t, q) in self.ts.iter().zip(&self.pts) {
            let d = q.distance_squared(p);
            if d < bd {
                bd = d;
                best = *t;
            }
        }
        best
    }
}

/// 1-D closest point on a curve, seeded then Newton-refined on `g(t) = C'(t)·(C(t) − p)`.
/// Returns `(t, converged)`.
fn closest_on_curve(c: &Curve3, samples: &CurveSamples, p: DVec3, seed: Option<f64>) -> (f64, bool) {
    let (a, b) = curve_domain(c);
    let mut t = seed.unwrap_or_else(|| samples.nearest(p));
    let span = (b - a).abs().max(1e-12);
    let h = span * 1e-4;
    for _ in 0..NEWTON_ITERS {
        let ct = c.point(t);
        let d1 = c.tangent(t);
        let r = ct - p;
        if r.length() < NEWTON_EPS {
            return (t, true);
        }
        let g = d1.dot(r);
        let d2 = curve_second_deriv(c, t, h);
        let gp = d1.length_squared() + d2.dot(r);
        if gp.abs() < 1e-18 {
            break;
        }
        let step = (g / gp).clamp(-span * 0.5, span * 0.5);
        let nt = (t - step).clamp(a, b);
        let moved = (nt - t).abs();
        t = nt;
        if moved < span * 1e-12 {
            return (t, true);
        }
    }
    let conv = c.point(t).distance(p) < (span * 1e-3).max(1e-6);
    (t, conv)
}

// --------------------------------------------------------------------------------------------
// seed grid for NURBS point inversion
// --------------------------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct SeedGrid {
    us: Vec<f64>,
    vs: Vec<f64>,
    pts: Vec<DVec3>,
    /// Mean |S_u| and |S_v| across the grid, used by `scale_uv`.
    scale: DVec2,
}

impl SeedGrid {
    fn nearest(&self, p: DVec3) -> DVec2 {
        let mut best = DVec2::new(self.us[0], self.vs[0]);
        let mut bd = f64::INFINITY;
        for (i, u) in self.us.iter().enumerate() {
            for (j, v) in self.vs.iter().enumerate() {
                let d = self.pts[i * self.vs.len() + j].distance_squared(p);
                if d < bd {
                    bd = d;
                    best = DVec2::new(*u, *v);
                }
            }
        }
        best
    }
}

// --------------------------------------------------------------------------------------------
// Param
// --------------------------------------------------------------------------------------------

/// Per-face parametrisation helper. Caches the seed data a surface needs for point inversion; it is
/// built once per face and used by every projection of that face.
pub(crate) struct Param<'a> {
    pub surf: &'a Surface,
    grid: OnceLock<SeedGrid>,
    profile: OnceLock<CurveSamples>,
    /// Angle of the revolution profile plane about the axis (radians).
    profile_angle: OnceLock<(f64, f64)>,
}

impl<'a> Param<'a> {
    pub fn new(surf: &'a Surface) -> Param<'a> {
        Param { surf, grid: OnceLock::new(), profile: OnceLock::new(), profile_angle: OnceLock::new() }
    }


    fn samples(&self) -> &CurveSamples {
        self.profile.get_or_init(|| match self.surf {
            Surface::Extrusion { profile, .. } | Surface::Revolution { profile, .. } => {
                CurveSamples::build(profile, 32)
            }
            _ => CurveSamples { ts: vec![0.0], pts: vec![DVec3::ZERO], mean_deriv: 1.0 },
        })
    }

    fn seed_grid(&self) -> &SeedGrid {
        self.grid.get_or_init(|| match self.surf {
            Surface::Nurbs(s) => {
                let dom = s.domain();
                let mk = |dir: usize| -> Vec<f64> {
                    let (a, b) = dom[dir];
                    let spans = distinct_knots(&s.knots[dir], s.degree[dir]).saturating_sub(1).max(1);
                    let n = (2 * spans + 1).clamp(8, 24);
                    (0..n).map(|i| a + (b - a) * (i as f64) / ((n - 1) as f64)).collect()
                };
                let us = mk(0);
                let vs = mk(1);
                let mut pts = Vec::with_capacity(us.len() * vs.len());
                let (mut su_acc, mut sv_acc) = (0.0, 0.0);
                for u in &us {
                    for v in &vs {
                        let (p, du, dv) = s.derivs1(*u, *v);
                        pts.push(p);
                        su_acc += du.length();
                        sv_acc += dv.length();
                    }
                }
                let n = (us.len() * vs.len()) as f64;
                SeedGrid {
                    us,
                    vs,
                    pts,
                    scale: DVec2::new((su_acc / n).max(1e-9), (sv_acc / n).max(1e-9)),
                }
            }
            _ => SeedGrid { us: vec![0.0], vs: vec![0.0], pts: vec![DVec3::ZERO], scale: DVec2::ONE },
        })
    }

    /// Angle (radians) of the revolution profile plane about the axis, plus the mean profile radius.
    fn revolution_ref(&self) -> (f64, f64) {
        *self.profile_angle.get_or_init(|| match self.surf {
            Surface::Revolution { axis, .. } => {
                let s = self.samples();
                let mut acc = DVec2::ZERO;
                let mut rad = 0.0;
                for p in &s.pts {
                    let l = axis.to_local(*p);
                    let r = (l.x * l.x + l.y * l.y).sqrt();
                    rad += r;
                    if r > 1e-12 {
                        acc += DVec2::new(l.x / r, l.y / r) * r;
                    }
                }
                let ang = if acc.length_squared() > 1e-24 { acc.y.atan2(acc.x) } else { 0.0 };
                (ang, (rad / s.pts.len().max(1) as f64).max(1e-9))
            }
            _ => (0.0, 1.0),
        })
    }

    // ---------------------------------------------------------------------------------------
    // evaluation
    // ---------------------------------------------------------------------------------------

    /// `P(u, v)`. Periodic parameters are reduced modulo the period first.
    pub fn eval(&self, u: f64, v: f64) -> DVec3 {
        match self.surf {
            Surface::Plane { f } => f.o + f.x * u + f.y * v,
            Surface::Cylinder { f, r } => {
                f.o + (f.x * u.cos() + f.y * u.sin()) * *r + f.z * v
            }
            Surface::Cone { f, r, alpha } => {
                let rho = r + v * alpha.tan();
                f.o + (f.x * u.cos() + f.y * u.sin()) * rho + f.z * v
            }
            Surface::Sphere { f, r } => {
                let (cv, sv) = (v.cos(), v.sin());
                f.o + (f.x * (cv * u.cos()) + f.y * (cv * u.sin()) + f.z * sv) * *r
            }
            Surface::Torus { f, big_r, r } => {
                let (cv, sv) = (v.cos(), v.sin());
                let rho = big_r + r * cv;
                f.o + (f.x * u.cos() + f.y * u.sin()) * rho + f.z * (r * sv)
            }
            Surface::Extrusion { profile, dir } => profile.point(u) + *dir * v,
            Surface::Revolution { profile, axis } => {
                let c = profile.point(v);
                axis.o + rotate_about(c - axis.o, axis.z, u.cos(), u.sin())
            }
            Surface::Nurbs(s) => {
                let (u, v) = self.clamp_nurbs(s, u, v);
                s.point(u, v)
            }
            Surface::Unsupported(_) => DVec3::ZERO,
        }
    }

    fn clamp_nurbs(&self, s: &crate::geom::NurbsSurface, u: f64, v: f64) -> (f64, f64) {
        let d = s.domain();
        let f = |x: f64, (a, b): (f64, f64), closed: bool| {
            if closed && b > a { wrap_into(x, a, b - a) } else { x.clamp(a, b) }
        };
        (f(u, d[0], s.closed[0]), f(v, d[1], s.closed[1]))
    }

    /// `(P, ∂P/∂u, ∂P/∂v)`.
    pub fn derivs(&self, u: f64, v: f64) -> (DVec3, DVec3, DVec3) {
        match self.surf {
            Surface::Plane { f } => (f.o + f.x * u + f.y * v, f.x, f.y),
            Surface::Cylinder { f, r } => {
                let (cu, su) = (u.cos(), u.sin());
                let er = f.x * cu + f.y * su;
                let et = f.x * -su + f.y * cu;
                (f.o + er * *r + f.z * v, et * *r, f.z)
            }
            Surface::Cone { f, r, alpha } => {
                let (cu, su) = (u.cos(), u.sin());
                let er = f.x * cu + f.y * su;
                let et = f.x * -su + f.y * cu;
                let ta = alpha.tan();
                let rho = r + v * ta;
                (f.o + er * rho + f.z * v, et * rho, er * ta + f.z)
            }
            Surface::Sphere { f, r } => {
                let (cu, su, cv, sv) = (u.cos(), u.sin(), v.cos(), v.sin());
                let er = f.x * cu + f.y * su;
                let et = f.x * -su + f.y * cu;
                let p = f.o + (er * cv + f.z * sv) * *r;
                (p, et * (*r * cv), (er * -sv + f.z * cv) * *r)
            }
            Surface::Torus { f, big_r, r } => {
                let (cu, su, cv, sv) = (u.cos(), u.sin(), v.cos(), v.sin());
                let er = f.x * cu + f.y * su;
                let et = f.x * -su + f.y * cu;
                let rho = big_r + r * cv;
                (f.o + er * rho + f.z * (r * sv), et * rho, (er * -sv + f.z * cv) * *r)
            }
            Surface::Extrusion { profile, dir } => {
                (profile.point(u) + *dir * v, profile.tangent(u), *dir)
            }
            Surface::Revolution { profile, axis } => {
                let (cu, su) = (u.cos(), u.sin());
                let c = profile.point(v);
                let p = axis.o + rotate_about(c - axis.o, axis.z, cu, su);
                let dv = rotate_about(profile.tangent(v), axis.z, cu, su);
                (p, axis.z.cross(p - axis.o), dv)
            }
            Surface::Nurbs(s) => {
                let (u, v) = self.clamp_nurbs(s, u, v);
                s.derivs1(u, v)
            }
            Surface::Unsupported(_) => (DVec3::ZERO, DVec3::X, DVec3::Y),
        }
    }

    /// Unit surface normal *before* `same_sense` is applied (i.e. along `∂u × ∂v`).
    pub fn normal(&self, u: f64, v: f64) -> DVec3 {
        match self.surf {
            Surface::Plane { f } => f.z,
            Surface::Cylinder { f, .. } => f.x * u.cos() + f.y * u.sin(),
            Surface::Cone { f, alpha, .. } => {
                let er = f.x * u.cos() + f.y * u.sin();
                er * alpha.cos() - f.z * alpha.sin()
            }
            Surface::Sphere { f, .. } => {
                let (cv, sv) = (v.cos(), v.sin());
                f.x * (cv * u.cos()) + f.y * (cv * u.sin()) + f.z * sv
            }
            Surface::Torus { f, .. } => {
                let (cv, sv) = (v.cos(), v.sin());
                (f.x * u.cos() + f.y * u.sin()) * cv + f.z * sv
            }
            _ => {
                let (_, du, dv) = self.derivs(u, v);
                safe_normalize(du.cross(dv), fallback_normal(du, dv))
            }
        }
    }

    /// Periods of the two parametric directions, if any.
    pub fn period(&self) -> [Option<f64>; 2] {
        match self.surf {
            Surface::Plane { .. } => [None, None],
            Surface::Cylinder { .. } | Surface::Cone { .. } | Surface::Sphere { .. } => {
                [Some(TAU), None]
            }
            Surface::Torus { .. } => [Some(TAU), Some(TAU)],
            Surface::Extrusion { profile, .. } => [profile.period(), None],
            Surface::Revolution { profile, .. } => [Some(TAU), profile.period()],
            Surface::Nurbs(s) => {
                let d = s.domain();
                let f = |i: usize| {
                    if s.closed[i] && d[i].1 > d[i].0 { Some(d[i].1 - d[i].0) } else { None }
                };
                [f(0), f(1)]
            }
            Surface::Unsupported(_) => [None, None],
        }
    }

    /// The natural parametric domain where the surface has one (used for pole detection and the
    /// full-domain fallback).
    pub fn natural_domain(&self) -> [Option<(f64, f64)>; 2] {
        match self.surf {
            Surface::Plane { .. } => [None, None],
            Surface::Cylinder { .. } => [Some((0.0, TAU)), None],
            Surface::Cone { r, alpha, .. } => {
                let ta = alpha.tan();
                let apex = if ta.abs() > 1e-12 { Some(-r / ta) } else { None };
                // The apex bounds the cone on one side only.
                let v = apex.map(|a| if ta > 0.0 { (a, a + 1.0e6) } else { (a - 1.0e6, a) });
                [Some((0.0, TAU)), v]
            }
            Surface::Sphere { .. } => {
                [Some((0.0, TAU)), Some((-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2))]
            }
            Surface::Torus { .. } => [Some((0.0, TAU)), Some((-std::f64::consts::PI, std::f64::consts::PI))],
            Surface::Extrusion { profile, .. } => [Some(curve_domain(profile)), None],
            Surface::Revolution { profile, .. } => [Some((0.0, TAU)), Some(curve_domain(profile))],
            Surface::Nurbs(s) => {
                let d = s.domain();
                [Some(d[0]), Some(d[1])]
            }
            Surface::Unsupported(_) => [None, None],
        }
    }

    /// Scale factors that make the `(u, v)` plane approximately isometric to the surface at the
    /// given mid-`v`. Never returns a zero or non-finite component.
    pub fn scale_uv(&self, v_mid: f64) -> DVec2 {
        let s = match self.surf {
            Surface::Plane { .. } => DVec2::ONE,
            Surface::Cylinder { r, .. } => DVec2::new(*r, 1.0),
            Surface::Cone { r, alpha, .. } => {
                DVec2::new(r + v_mid * alpha.tan(), 1.0 / alpha.cos().abs().max(1e-9))
            }
            Surface::Sphere { r, .. } => DVec2::new(r * v_mid.cos(), *r),
            Surface::Torus { big_r, r, .. } => DVec2::new(big_r + r * v_mid.cos(), *r),
            Surface::Extrusion { dir, .. } => DVec2::new(self.samples().mean_deriv, dir.length()),
            Surface::Revolution { .. } => {
                let (_, rad) = self.revolution_ref();
                DVec2::new(rad, self.samples().mean_deriv)
            }
            Surface::Nurbs(_) => self.seed_grid().scale,
            Surface::Unsupported(_) => DVec2::ONE,
        };
        let f = |x: f64| if x.is_finite() && x.abs() > 1e-9 { x.abs() } else { 1.0 };
        DVec2::new(f(s.x), f(s.y))
    }

    /// Radii of curvature along `u` and `v`; `None` where the surface is ruled/flat in that
    /// direction (no Steiner refinement needed there).
    pub fn curvature_radii(&self, u: f64, v: f64) -> [Option<f64>; 2] {
        match self.surf {
            Surface::Plane { .. } => [None, None],
            Surface::Cylinder { r, .. } => [Some(*r), None],
            Surface::Cone { r, alpha, .. } => {
                let rho = (r + v * alpha.tan()).abs();
                [if rho > 1e-9 { Some(rho) } else { None }, None]
            }
            Surface::Sphere { r, .. } => {
                let ru = (r * v.cos()).abs();
                [if ru > 1e-9 { Some(ru) } else { None }, Some(*r)]
            }
            Surface::Torus { big_r, r, .. } => {
                let ru = (big_r + r * v.cos()).abs();
                [if ru > 1e-9 { Some(ru) } else { None }, Some(*r)]
            }
            Surface::Extrusion { profile, .. } => [curve_radius(profile, u), None],
            Surface::Revolution { profile, axis } => {
                let p = self.eval(u, v);
                let l = axis.to_local(p);
                let ru = (l.x * l.x + l.y * l.y).sqrt();
                [if ru > 1e-9 { Some(ru) } else { None }, curve_radius(profile, v)]
            }
            Surface::Nurbs(s) => {
                let (uc, vc) = self.clamp_nurbs(s, u, v);
                let (_, du, dv, duu, _, dvv) = s.derivs2(uc, vc);
                let n = safe_normalize(du.cross(dv), DVec3::Z);
                let f = |d: DVec3, dd: DVec3| {
                    let k = n.dot(dd).abs();
                    let e = d.length_squared();
                    if k > 1e-12 && e > 1e-18 { Some(e / k) } else { None }
                };
                [f(du, duu), f(dv, dvv)]
            }
            Surface::Unsupported(_) => [None, None],
        }
    }

    /// Point inversion. `seed` is the previous sample's parameters (never cold-start when a
    /// neighbouring value is available). Returns `(uv, converged)`.
    pub fn inverse(&self, p: DVec3, seed: Option<DVec2>) -> (DVec2, bool) {
        let (uv, ok) = self.inverse_raw(p, seed);
        // Continue periodic parameters from the seed so rings come out unwrapped already.
        let mut uv = uv;
        if let Some(s) = seed {
            let per = self.period();
            if let Some(pu) = per[0] {
                uv.x = s.x + wrap_half(uv.x - s.x, pu);
            }
            if let Some(pv) = per[1] {
                uv.y = s.y + wrap_half(uv.y - s.y, pv);
            }
        }
        (uv, ok)
    }

    fn inverse_raw(&self, p: DVec3, seed: Option<DVec2>) -> (DVec2, bool) {
        match self.surf {
            Surface::Plane { f } => {
                let l = f.to_local(p);
                (DVec2::new(l.x, l.y), true)
            }
            Surface::Cylinder { f, .. } => {
                let l = f.to_local(p);
                (DVec2::new(atan2_or(l.y, l.x, seed), l.z), true)
            }
            Surface::Cone { f, .. } => {
                let l = f.to_local(p);
                (DVec2::new(atan2_or(l.y, l.x, seed), l.z), true)
            }
            Surface::Sphere { f, .. } => {
                let l = f.to_local(p);
                let h = (l.x * l.x + l.y * l.y).sqrt();
                (DVec2::new(atan2_or(l.y, l.x, seed), l.z.atan2(h)), true)
            }
            Surface::Torus { f, big_r, .. } => {
                let l = f.to_local(p);
                let h = (l.x * l.x + l.y * l.y).sqrt() - big_r;
                let v = if l.z.abs() < 1e-15 && h.abs() < 1e-15 { 0.0 } else { l.z.atan2(h) };
                (DVec2::new(atan2_or(l.y, l.x, seed), v), true)
            }
            Surface::Extrusion { profile, dir } => self.inverse_extrusion(profile, *dir, p, seed),
            Surface::Revolution { profile, axis } => {
                self.inverse_revolution(profile, axis, p, seed)
            }
            Surface::Nurbs(s) => self.inverse_nurbs(s, p, seed),
            Surface::Unsupported(_) => (DVec2::ZERO, false),
        }
    }

    fn inverse_extrusion(
        &self,
        profile: &Curve3,
        dir: DVec3,
        p: DVec3,
        seed: Option<DVec2>,
    ) -> (DVec2, bool) {
        let dl2 = dir.length_squared();
        if dl2 < 1e-24 {
            return (seed.unwrap_or(DVec2::ZERO), false);
        }
        // A straight profile makes the surface a plane: solve the 2×2 normal equations exactly.
        if let Curve3::Line { p: lp, d } = profile {
            let a = DVec3::dot(*d, *d);
            let b = DVec3::dot(*d, dir);
            let c = dl2;
            let rhs = p - *lp;
            let det = a * c - b * b;
            if det.abs() > 1e-18 {
                let r0 = d.dot(rhs);
                let r1 = dir.dot(rhs);
                return (DVec2::new((r0 * c - b * r1) / det, (a * r1 - b * r0) / det), true);
            }
        }
        let samples = self.samples();
        let mut u = seed.map(|s| s.x).unwrap_or(f64::NAN);
        if !u.is_finite() {
            // Seed on the profile projected perpendicular to the extrusion direction.
            let dn = dir / dl2.sqrt();
            let pp = p - dn * dn.dot(p);
            let mut bd = f64::INFINITY;
            u = samples.ts[0];
            for (t, q) in samples.ts.iter().zip(&samples.pts) {
                let qq = *q - dn * dn.dot(*q);
                let d2 = qq.distance_squared(pp);
                if d2 < bd {
                    bd = d2;
                    u = *t;
                }
            }
        }
        let (a, b) = curve_domain(profile);
        let span = (b - a).abs().max(1e-12);
        let h = span * 1e-4;
        let mut conv = false;
        for _ in 0..NEWTON_ITERS {
            let c = profile.point(u);
            let v = (p - c).dot(dir) / dl2;
            let r = c + dir * v - p;
            if r.length() < NEWTON_EPS {
                conv = true;
                break;
            }
            let d1 = profile.tangent(u);
            let g = d1.dot(r);
            let d2 = curve_second_deriv(profile, u, h);
            let dn2 = d1.length_squared() - d1.dot(dir).powi(2) / dl2;
            let gp = dn2 + d2.dot(r);
            if gp.abs() < 1e-18 {
                break;
            }
            let step = (g / gp).clamp(-span * 0.5, span * 0.5);
            let nu = (u - step).clamp(a, b);
            let moved = (nu - u).abs();
            u = nu;
            if moved < span * 1e-12 {
                conv = true;
                break;
            }
        }
        let c = profile.point(u);
        let v = (p - c).dot(dir) / dl2;
        let residual = (c + dir * v).distance(p);
        (DVec2::new(u, v), conv || residual < 1e-6)
    }

    fn inverse_revolution(
        &self,
        profile: &Curve3,
        axis: &Frame,
        p: DVec3,
        seed: Option<DVec2>,
    ) -> (DVec2, bool) {
        let (theta0, _) = self.revolution_ref();
        let l = axis.to_local(p);
        let r = (l.x * l.x + l.y * l.y).sqrt();
        let u = if r > 1e-12 { l.y.atan2(l.x) - theta0 } else { seed.map(|s| s.x).unwrap_or(0.0) };
        // Rotate the point back into the profile plane and solve for v there.
        let q = axis.o + rotate_about(p - axis.o, axis.z, u.cos(), -u.sin());
        let (v, conv) = closest_on_curve(profile, self.samples(), q, seed.map(|s| s.y));
        (DVec2::new(u, v), conv)
    }

    fn inverse_nurbs(
        &self,
        s: &crate::geom::NurbsSurface,
        p: DVec3,
        seed: Option<DVec2>,
    ) -> (DVec2, bool) {
        let dom = s.domain();
        let grid = self.seed_grid();
        let seed_uv = seed
            .map(|x| DVec2::new(x.x, x.y))
            .unwrap_or_else(|| grid.nearest(p));
        let mut uv = seed_uv;
        let clamp = |uv: DVec2| -> DVec2 {
            let f = |x: f64, (a, b): (f64, f64), closed: bool| {
                if closed && b > a { wrap_into(x, a, b - a) } else { x.clamp(a, b) }
            };
            DVec2::new(f(uv.x, dom[0], s.closed[0]), f(uv.y, dom[1], s.closed[1]))
        };
        uv = clamp(uv);
        for _ in 0..NEWTON_ITERS {
            let (sp, su, sv, suu, suv, svv) = s.derivs2(uv.x, uv.y);
            let r = sp - p;
            if r.length() < 1e-6 {
                return (uv, true);
            }
            let f0 = su.dot(r);
            let f1 = sv.dot(r);
            let a11 = su.length_squared() + suu.dot(r);
            let a12 = su.dot(sv) + suv.dot(r);
            let a22 = sv.length_squared() + svv.dot(r);
            let det = a11 * a22 - a12 * a12;
            if det.abs() < 1e-20 {
                break;
            }
            let du = (-f0 * a22 + f1 * a12) / det;
            let dv = (-a11 * f1 + a12 * f0) / det;
            if !du.is_finite() || !dv.is_finite() {
                break;
            }
            let next = clamp(uv + DVec2::new(du, dv));
            let moved = (next - uv).length();
            uv = next;
            if moved < 1e-14 {
                break;
            }
        }
        let ok = s.point(uv.x, uv.y).distance(p) < 1e-4;
        if ok { (uv, true) } else { (seed_uv, false) }
    }
}

/// `atan2` continued from the seed's u when one is available (keeps rings unwrapped).
#[inline]
fn atan2_or(y: f64, x: f64, seed: Option<DVec2>) -> f64 {
    if x.abs() < 1e-15 && y.abs() < 1e-15 {
        return seed.map(|s| s.x).unwrap_or(0.0);
    }
    y.atan2(x)
}

/// A usable normal when `∂u × ∂v` degenerates (poles, apexes).
fn fallback_normal(du: DVec3, dv: DVec3) -> DVec3 {
    for c in [du, dv, DVec3::Z, DVec3::Y] {
        if let Some(n) = c.try_normalize() {
            return n;
        }
    }
    DVec3::Z
}

fn distinct_knots(knots: &[f64], degree: usize) -> usize {
    if knots.len() < 2 * degree + 2 {
        return 2;
    }
    let inner = &knots[degree..knots.len() - degree];
    let mut n = 0;
    let mut last = f64::NAN;
    for k in inner {
        if !(*k == last) {
            n += 1;
            last = *k;
        }
    }
    n.max(2)
}
