//! Planar triangulation of a face's trimming rings in (scaled) parameter space.
//!
//! Rings are scaled to be near-isometric to the surface, a Steiner grid is added where the surface
//! is curved, and a constrained Delaunay triangulation is built. Triangles are then classified by
//! the winding number of their centroid against *all* rings, which handles holes, dropped
//! constraints and self-intersections uniformly. A ladder of cruder fallbacks follows.

use glam::DVec2;
use rustc_hash::FxHashMap;
use spade::{ConstrainedDelaunayTriangulation, HasPosition, Point2, Triangulation as _};

use crate::discretize::Tol;
use crate::mesh::DiagKind;
use crate::param::Param;
use crate::topo::EdgeId;

/// Maximum interior points added to a single face.
const MAX_STEINER: usize = 4096;

/// One trimming ring in parameter space, with a back-reference to the shared 3-D edge samples.
#[derive(Clone, Debug)]
pub(crate) struct UvRing {
    pub uv: Vec<DVec2>,
    /// `(edge, sample index)` of the shared polyline point, or `None` for synthesized points whose
    /// 3-D position comes from `Param::eval`.
    pub src: Vec<Option<(EdgeId, u32)>>,
    pub is_outer: bool,
}

impl UvRing {
    pub fn new(is_outer: bool) -> UvRing {
        UvRing { uv: Vec::new(), src: Vec::new(), is_outer }
    }
    #[inline]
    pub fn push(&mut self, p: DVec2, s: Option<(EdgeId, u32)>) {
        self.uv.push(p);
        self.src.push(s);
    }
    pub fn len(&self) -> usize {
        self.uv.len()
    }
    pub fn reverse(&mut self) {
        self.uv.reverse();
        self.src.reverse();
    }
    pub fn rotate_left(&mut self, k: usize) {
        if k > 0 && k < self.uv.len() {
            self.uv.rotate_left(k);
            self.src.rotate_left(k);
        }
    }
    /// Drop consecutive duplicates (and the wrap-around duplicate).
    pub fn dedup(&mut self, eps: f64) {
        let mut uv = Vec::with_capacity(self.uv.len());
        let mut src = Vec::with_capacity(self.src.len());
        for i in 0..self.uv.len() {
            if uv.last().is_some_and(|last| DVec2::distance(*last, self.uv[i]) <= eps) {
                continue;
            }
            uv.push(self.uv[i]);
            src.push(self.src[i]);
        }
        while uv.len() > 1 && DVec2::distance(uv[0], uv[uv.len() - 1]) <= eps {
            uv.pop();
            src.pop();
        }
        self.uv = uv;
        self.src = src;
    }
}

/// Triangulated parameter-space patch. Triangles are counter-clockwise in `uv`.
#[derive(Debug, Default)]
pub(crate) struct UvMesh {
    pub uv: Vec<DVec2>,
    pub src: Vec<Option<(EdgeId, u32)>>,
    pub tris: Vec<[u32; 3]>,
    /// 0 = primary path, 1..=4 = fallback level actually used.
    pub fallback: u8,
    pub diags: Vec<(DiagKind, String)>,
}

#[inline]
fn cross(a: DVec2, b: DVec2) -> f64 {
    a.x * b.y - a.y * b.x
}

pub(crate) fn signed_area(p: &[DVec2]) -> f64 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        a += cross(p[i], p[(i + 1) % n]);
    }
    0.5 * a
}

/// Winding number of `p` with respect to every ring (rings must be oriented outer-CCW/holes-CW).
fn winding(rings: &[Vec<DVec2>], p: DVec2) -> i32 {
    let mut w = 0;
    for r in rings {
        let n = r.len();
        for i in 0..n {
            let a = r[i];
            let b = r[(i + 1) % n];
            if a.y <= p.y {
                if b.y > p.y && cross(b - a, p - a) > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && cross(b - a, p - a) < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

fn seg_dist_sq(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-30 {
        return p.distance_squared(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.distance_squared(a + ab * t)
}

fn uv_extent(rings: &[UvRing]) -> DVec2 {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for r in rings {
        for p in &r.uv {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    if lo.is_finite() && hi.is_finite() {
        (hi - lo).max(DVec2::splat(1e-12))
    } else {
        DVec2::splat(1e-12)
    }
}

/// Drop duplicate points, discard rings with fewer than three distinct points, and orient the
/// rings so the outer one is counter-clockwise and every hole clockwise. The outer ring ends up
/// first. Returns `None` when nothing usable remains.
pub(crate) fn prepare_rings(rings: Vec<UvRing>) -> Option<Vec<UvRing>> {
    let extent = uv_extent(&rings);
    let dedup_eps = extent.x.max(extent.y) * 1e-9;
    let mut clean: Vec<UvRing> = Vec::with_capacity(rings.len());
    for mut r in rings {
        r.dedup(dedup_eps);
        if r.len() >= 3 {
            clean.push(r);
        } else if r.is_outer {
            return None;
        }
    }
    if clean.is_empty() {
        return None;
    }
    let areas: Vec<f64> = clean.iter().map(|r| signed_area(&r.uv)).collect();
    let outer_idx = clean.iter().position(|r| r.is_outer).unwrap_or_else(|| {
        let mut best = 0usize;
        for i in 1..areas.len() {
            if areas[i].abs() > areas[best].abs() {
                best = i;
            }
        }
        best
    });
    for (i, r) in clean.iter_mut().enumerate() {
        let want_positive = i == outer_idx;
        if areas[i] != 0.0 && (areas[i] > 0.0) != want_positive {
            r.reverse();
        }
    }
    clean.swap(0, outer_idx);
    Some(clean)
}

/// Triangulate a face's rings. `steiner` disables interior refinement when false.
pub(crate) fn triangulate(
    rings: Vec<UvRing>,
    param: &Param<'_>,
    tol: &Tol,
    steiner: bool,
) -> Option<UvMesh> {
    let mut diags: Vec<(DiagKind, String)> = Vec::new();

    // --- scale ------------------------------------------------------------------------------
    let mut v_sum = 0.0;
    let mut v_n = 0usize;
    for r in &rings {
        for p in &r.uv {
            v_sum += p.y;
            v_n += 1;
        }
    }
    if v_n == 0 {
        return None;
    }
    let scale = param.scale_uv(v_sum / v_n as f64);

    // --- clean rings ------------------------------------------------------------------------
    let extent = uv_extent(&rings);
    let clean = prepare_rings(rings)?;

    let scaled: Vec<Vec<DVec2>> =
        clean.iter().map(|r| r.uv.iter().map(|p| *p * scale).collect()).collect();

    // --- assemble the point set --------------------------------------------------------------
    let mut uv: Vec<DVec2> = Vec::new();
    let mut src: Vec<Option<(EdgeId, u32)>> = Vec::new();
    let mut sp: Vec<DVec2> = Vec::new();
    let mut constraints: Vec<[usize; 2]> = Vec::new();
    let mut ring_ranges: Vec<(usize, usize)> = Vec::new();
    for (r, s) in clean.iter().zip(&scaled) {
        let start = uv.len();
        for (p, q) in r.uv.iter().zip(s) {
            uv.push(*p);
            sp.push(*q);
        }
        src.extend_from_slice(&r.src);
        let end = uv.len();
        for i in start..end {
            constraints.push([i, if i + 1 == end { start } else { i + 1 }]);
        }
        ring_ranges.push((start, end));
    }
    let boundary_count = uv.len();

    if steiner {
        for p in steiner_points(&clean, &scaled, param, tol, scale, &extent) {
            uv.push(p / scale);
            sp.push(p);
            src.push(None);
        }
    }

    // --- primary: constrained Delaunay --------------------------------------------------------
    if let Some(tris) = cdt(&sp, &constraints, &scaled, &mut diags)
        && !tris.is_empty()
    {
        return Some(UvMesh { uv, src, tris, fallback: 0, diags });
    }

    // --- fallback 1: the same CDT without the interior points ---------------------------------
    if uv.len() > boundary_count {
        let bsp = &sp[..boundary_count];
        if let Some(tris) = cdt(bsp, &constraints, &scaled, &mut diags)
            && !tris.is_empty()
        {
            diags.push((
                DiagKind::TriangulationFallback,
                "cdt failed with Steiner points, retried on the boundary only".into(),
            ));
            uv.truncate(boundary_count);
            src.truncate(boundary_count);
            return Some(UvMesh { uv, src, tris, fallback: 1, diags });
        }
    }

    // --- fallback 2: i_triangle (integer-robust, survives self-intersections) ------------------
    if let Some(m) = i_triangle_fallback(&clean, &scaled, scale, &extent) {
        diags.push((DiagKind::TriangulationFallback, "cdt failed, used i_triangle".into()));
        return Some(UvMesh { fallback: 2, diags, ..m });
    }

    // --- fallback 3: ear clipping on the rings without Steiner points -------------------------
    if let Some(tris) = earcut_fallback(&scaled, &ring_ranges) {
        diags.push((DiagKind::TriangulationFallback, "cdt failed, used earcut".into()));
        uv.truncate(boundary_count);
        src.truncate(boundary_count);
        return Some(UvMesh { uv, src, tris, fallback: 3, diags });
    }

    // --- fallback 4: convex fan of the outer ring ---------------------------------------------
    let (s0, e0) = ring_ranges[0];
    if e0 - s0 >= 3 {
        let tris: Vec<[u32; 3]> = (s0 + 1..e0 - 1)
            .map(|i| [s0 as u32, i as u32, (i + 1) as u32])
            .collect();
        diags.push((DiagKind::TriangulationFallback, "degenerate rings, used a convex fan".into()));
        uv.truncate(boundary_count);
        src.truncate(boundary_count);
        return Some(UvMesh { uv, src, tris, fallback: 4, diags });
    }
    None
}

// ------------------------------------------------------------------------------------------
// Steiner grid
// ------------------------------------------------------------------------------------------

fn direction_step(param: &Param<'_>, tol: &Tol, dir: usize, scale: DVec2, extent: DVec2) -> f64 {
    let ext = if dir == 0 { extent.x } else { extent.y };
    let s = if dir == 0 { scale.x } else { scale.y };
    let radii = param.curvature_radii(0.0, 0.0);
    match radii[dir] {
        Some(r) if r > 1e-9 => {
            // θ = min(max_angle_surf, 2·acos(1 − 2·chord/ρ))
            let c = (1.0 - 2.0 * tol.chord / r).clamp(-1.0, 1.0);
            let ang = (2.0 * c.acos()).min(tol.max_angle_surf).max(1e-3);
            (ang * r / s.max(1e-12)).clamp(ext * 1e-4, ext)
        }
        // Ruled direction: no refinement for curvature reasons.
        _ => ext,
    }
}

fn steiner_points(
    rings: &[UvRing],
    scaled: &[Vec<DVec2>],
    param: &Param<'_>,
    tol: &Tol,
    scale: DVec2,
    extent: &DVec2,
) -> Vec<DVec2> {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for s in scaled {
        for p in s {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    let _ = rings;
    let mut hu = direction_step(param, tol, 0, scale, *extent) * scale.x;
    let mut hv = direction_step(param, tol, 1, scale, *extent) * scale.y;
    let size = hi - lo;
    if !(size.x > 0.0 && size.y > 0.0) {
        return Vec::new();
    }
    hu = hu.max(size.x * 1e-3);
    hv = hv.max(size.y * 1e-3);
    // Grow the spacing until the grid fits the budget.
    for _ in 0..8 {
        let nu = (size.x / hu).ceil().max(1.0);
        let nv = (size.y / hv).ceil().max(1.0);
        if nu * nv <= MAX_STEINER as f64 {
            break;
        }
        let f = ((nu * nv) / MAX_STEINER as f64).sqrt();
        hu *= f;
        hv *= f;
    }
    let nu = (size.x / hu).ceil().max(1.0) as usize;
    let nv = (size.y / hv).ceil().max(1.0) as usize;
    if nu * nv > MAX_STEINER {
        return Vec::new();
    }
    let (cu, cv) = (size.x / nu as f64, size.y / nv as f64);
    let clearance = 0.3 * cu.min(cv);

    // Mark cells that a ring segment passes close to.
    let mut blocked = vec![false; nu * nv];
    for s in scaled {
        for i in 0..s.len() {
            let a = s[i];
            let b = s[(i + 1) % s.len()];
            let seg_lo = a.min(b) - DVec2::splat(clearance);
            let seg_hi = a.max(b) + DVec2::splat(clearance);
            let i0 = (((seg_lo.x - lo.x) / cu).floor().max(0.0) as usize).min(nu.saturating_sub(1));
            let i1 = (((seg_hi.x - lo.x) / cu).ceil().max(0.0) as usize).min(nu);
            let j0 = (((seg_lo.y - lo.y) / cv).floor().max(0.0) as usize).min(nv.saturating_sub(1));
            let j1 = (((seg_hi.y - lo.y) / cv).ceil().max(0.0) as usize).min(nv);
            for ii in i0..i1 {
                for jj in j0..j1 {
                    if blocked[ii * nv + jj] {
                        continue;
                    }
                    let c = lo + DVec2::new((ii as f64 + 0.5) * cu, (jj as f64 + 0.5) * cv);
                    if seg_dist_sq(c, a, b) < clearance * clearance {
                        blocked[ii * nv + jj] = true;
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for ii in 0..nu {
        for jj in 0..nv {
            if blocked[ii * nv + jj] {
                continue;
            }
            let c = lo + DVec2::new((ii as f64 + 0.5) * cu, (jj as f64 + 0.5) * cv);
            if winding(scaled, c) != 0 {
                out.push(c);
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// CDT
// ------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct CdtVert {
    p: Point2<f64>,
    idx: u32,
}

impl HasPosition for CdtVert {
    type Scalar = f64;
    fn position(&self) -> Point2<f64> {
        self.p
    }
}

fn cdt(
    sp: &[DVec2],
    constraints: &[[usize; 2]],
    rings: &[Vec<DVec2>],
    diags: &mut Vec<(DiagKind, String)>,
) -> Option<Vec<[u32; 3]>> {
    if sp.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let verts: Vec<CdtVert> = sp
        .iter()
        .enumerate()
        .map(|(i, p)| CdtVert { p: Point2::new(p.x, p.y), idx: i as u32 })
        .collect();
    let mut conflicts = 0usize;
    let t: ConstrainedDelaunayTriangulation<CdtVert> = ConstrainedDelaunayTriangulation::
        try_bulk_load_cdt(verts, constraints.to_vec(), |_| conflicts += 1)
        .ok()?;
    if conflicts > 0 {
        diags.push((
            DiagKind::CdtConflict,
            format!("{conflicts} crossing constraint(s) dropped"),
        ));
    }
    let mut tris = Vec::new();
    for f in t.inner_faces() {
        let vs = f.vertices();
        let a = vs[0].data().idx;
        let b = vs[1].data().idx;
        let c = vs[2].data().idx;
        let centroid = (sp[a as usize] + sp[b as usize] + sp[c as usize]) / 3.0;
        if winding(rings, centroid) != 0 {
            tris.push([a, b, c]);
        }
    }
    Some(tris)
}

// ------------------------------------------------------------------------------------------
// fallbacks
// ------------------------------------------------------------------------------------------

fn i_triangle_fallback(
    rings: &[UvRing],
    scaled: &[Vec<DVec2>],
    scale: DVec2,
    extent: &DVec2,
) -> Option<UvMesh> {
    use i_triangle::float::triangulatable::Triangulatable;

    let shape: Vec<Vec<[f64; 2]>> =
        scaled.iter().map(|r| r.iter().map(|p| [p.x, p.y]).collect()).collect();
    if !shape.first().is_some_and(|c| c.len() >= 3) {
        return None;
    }
    let raw = shape.as_slice().triangulate();
    let pts: Vec<[f64; 2]> = raw.points();
    let idx: Vec<u32> = raw.triangle_indices::<u32>();
    if idx.len() < 3 || pts.is_empty() {
        return None;
    }

    // Map the output points back onto the original ring samples where possible so the shared edge
    // discretisation (and therefore crack-freeness) is preserved.
    let eps = (extent.x.max(extent.y)) * scale.x.max(scale.y) * 1e-6 + 1e-12;
    let key = |p: [f64; 2]| ((p[0] / eps).round() as i64, (p[1] / eps).round() as i64);
    let mut lookup: FxHashMap<(i64, i64), usize> = FxHashMap::default();
    let mut uv: Vec<DVec2> = Vec::new();
    let mut src: Vec<Option<(EdgeId, u32)>> = Vec::new();
    for (r, s) in rings.iter().zip(scaled) {
        for (k, q) in s.iter().enumerate() {
            lookup.entry(key([q.x, q.y])).or_insert(uv.len());
            uv.push(r.uv[k]);
            src.push(r.src[k]);
        }
    }
    let mut remap: Vec<u32> = Vec::with_capacity(pts.len());
    for p in &pts {
        match lookup.get(&key(*p)) {
            Some(i) => remap.push(*i as u32),
            None => {
                remap.push(uv.len() as u32);
                uv.push(DVec2::new(p[0], p[1]) / scale);
                src.push(None);
            }
        }
    }
    let mut tris = Vec::with_capacity(idx.len() / 3);
    for t in idx.as_chunks::<3>().0 {
        let (a, b, c) = (remap[t[0] as usize], remap[t[1] as usize], remap[t[2] as usize]);
        if a != b && b != c && a != c {
            tris.push([a, b, c]);
        }
    }
    if tris.is_empty() {
        return None;
    }
    Some(UvMesh { uv, src, tris, fallback: 2, diags: Vec::new() })
}

fn earcut_fallback(scaled: &[Vec<DVec2>], ranges: &[(usize, usize)]) -> Option<Vec<[u32; 3]>> {
    let mut flat: Vec<f64> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    for (k, r) in scaled.iter().enumerate() {
        if k > 0 {
            holes.push(flat.len() / 2);
        }
        for p in r {
            flat.push(p.x);
            flat.push(p.y);
        }
    }
    let idx = earcutr::earcut(&flat, &holes, 2).ok()?;
    if idx.len() < 3 {
        return None;
    }
    // Ring points were pushed contiguously in the same order, so indices line up already.
    let base = ranges.first().map(|r| r.0).unwrap_or(0);
    Some(
        idx.as_chunks::<3>().0.iter()
            .map(|t| {
                [(t[0] + base) as u32, (t[1] + base) as u32, (t[2] + base) as u32]
            })
            .collect(),
    )
}

/// Ear clipping on a plain point list (used by the planar fast path).
pub(crate) fn earcut_planar(
    rings: &[Vec<DVec2>],
) -> Option<(Vec<[u32; 3]>, f64)> {
    let mut flat: Vec<f64> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    for (k, r) in rings.iter().enumerate() {
        if k > 0 {
            holes.push(flat.len() / 2);
        }
        for p in r {
            flat.push(p.x);
            flat.push(p.y);
        }
    }
    let idx = earcutr::earcut(&flat, &holes, 2).ok()?;
    if idx.len() < 3 {
        return None;
    }
    let dev = earcutr::deviation(&flat, &holes, 2, &idx);
    let tris = idx
        .as_chunks::<3>().0.iter()
        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect();
    Some((tris, dev))
}
