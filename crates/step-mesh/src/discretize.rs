//! Edge → polyline. Every STEP `EDGE_CURVE` is discretised exactly once and both adjacent faces
//! reuse the result, which makes the mesh crack-free by construction.

use glam::DVec3;
use rayon::prelude::*;

use crate::geom::Curve3;
use crate::mesh::TessParams;
use crate::param::{TAU, curve_domain};
use crate::topo::{Edge, ShapeTopology};

/// Resolved tolerances for one tessellation run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tol {
    /// Max chord deviation in mm.
    pub chord: f64,
    /// Max turn between consecutive edge segments, radians.
    pub max_angle_edge: f64,
    /// Max normal turn across a surface quad, radians.
    pub max_angle_surf: f64,
    /// Model closure tolerance (mm).
    pub tol: f64,
}

impl Tol {
    pub fn new(params: &TessParams, bbox_diag: f64, model_tol: f64) -> Tol {
        let chord = if params.chord > 0.0 {
            params.chord
        } else {
            (2.5e-4 * bbox_diag).clamp(0.005, 0.5)
        };
        Tol {
            chord: chord.max(1e-6),
            max_angle_edge: params.max_angle_edge_deg.to_radians().clamp(1e-3, 1.5),
            max_angle_surf: params.max_angle_surf_deg.to_radians().clamp(1e-3, 1.5),
            tol: if model_tol.is_finite() && model_tol > 0.0 { model_tol } else { 1e-3 },
        }
    }
}

/// Sampled points of one edge, always ordered from `v0` to `v1`.
#[derive(Clone, Debug, Default)]
pub(crate) struct EdgePolyline {
    pub pts: Vec<DVec3>,
}

impl EdgePolyline {
    pub fn is_degenerate(&self) -> bool {
        self.pts.len() < 2
    }
}

/// Discretise every edge in parallel.
pub(crate) fn discretize_all(topo: &ShapeTopology, tol: &Tol) -> Vec<EdgePolyline> {
    topo.edges.par_iter().map(|e| discretize_edge(topo, e, tol)).collect()
}

/// Parameter interval of an edge, oriented `v0 → v1`.
fn edge_range(topo: &ShapeTopology, e: &Edge, tol: &Tol) -> (f64, f64) {
    let p0 = topo.vertices.get(e.v0.idx()).map(|v| v.p).unwrap_or(DVec3::ZERO);
    let p1 = topo.vertices.get(e.v1.idx()).map(|v| v.p).unwrap_or(DVec3::ZERO);
    let closed = e.v0 == e.v1 || p0.distance(p1) <= tol.tol.max(1e-9);
    match &e.curve {
        Curve3::Line { p, d } => {
            let t0 = (p0 - *p).dot(*d);
            let t1 = (p1 - *p).dot(*d);
            if (t1 - t0).abs() < 1e-12 { (t0, t0 + p0.distance(p1).max(1e-9)) } else { (t0, t1) }
        }
        Curve3::Circle { f, .. } | Curve3::Ellipse { f, .. } => {
            let a0 = {
                let l = f.to_local(p0);
                l.y.atan2(l.x)
            };
            let a1 = {
                let l = f.to_local(p1);
                l.y.atan2(l.x)
            };
            if closed {
                return if e.same_sense { (a0, a0 + TAU) } else { (a0, a0 - TAU) };
            }
            let mut t1 = a1;
            if e.same_sense {
                while t1 <= a0 + 1e-12 {
                    t1 += TAU;
                }
            } else {
                while t1 >= a0 - 1e-12 {
                    t1 -= TAU;
                }
            }
            (a0, t1)
        }
        Curve3::Nurbs(c) => {
            let (a, b) = c.domain();
            if closed {
                return if e.same_sense { (a, b) } else { (b, a) };
            }
            // Pick the orientation whose start is nearest `v0`.
            if c.point(a).distance(p0) > c.point(b).distance(p0) { (b, a) } else { (a, b) }
        }
        Curve3::Polyline(pts) => {
            let n = (pts.len().max(2) - 1) as f64;
            if pts.first().map(|q| q.distance(p0)).unwrap_or(0.0)
                > pts.last().map(|q| q.distance(p0)).unwrap_or(0.0)
            {
                (n, 0.0)
            } else {
                (0.0, n)
            }
        }
    }
}

fn discretize_edge(topo: &ShapeTopology, e: &Edge, tol: &Tol) -> EdgePolyline {
    let (t0, t1) = edge_range(topo, e, tol);
    if !t0.is_finite() || !t1.is_finite() {
        return EdgePolyline { pts: Vec::new() };
    }
    let mut pts = sample_curve(&e.curve, t0, t1, tol);
    if pts.len() < 2 {
        return EdgePolyline { pts: Vec::new() };
    }
    // Snap the ends bit-exactly onto the shared vertices so adjacent faces agree.
    if let Some(v) = topo.vertices.get(e.v0.idx()) {
        pts[0] = v.p;
    }
    if let Some(v) = topo.vertices.get(e.v1.idx()) {
        let n = pts.len() - 1;
        pts[n] = v.p;
    }
    EdgePolyline { pts }
}

/// Sample a curve over `[t0, t1]` (either direction) to the given tolerances.
pub(crate) fn sample_curve(c: &Curve3, t0: f64, t1: f64, tol: &Tol) -> Vec<DVec3> {
    match c {
        Curve3::Line { .. } => vec![c.point(t0), c.point(t1)],
        Curve3::Circle { r, .. } => arc_samples(c, t0, t1, *r, tol),
        Curve3::Ellipse { a, b, .. } => arc_samples(c, t0, t1, a.max(*b), tol),
        Curve3::Polyline(pts) => {
            let n = pts.len();
            if n < 2 {
                return Vec::new();
            }
            if t1 >= t0 { pts.clone() } else { pts.iter().rev().copied().collect() }
        }
        Curve3::Nurbs(_) => nurbs_samples(c, t0, t1, tol),
    }
}

fn arc_samples(c: &Curve3, t0: f64, t1: f64, r: f64, tol: &Tol) -> Vec<DVec3> {
    let sweep = t1 - t0;
    // Angular step from the chord tolerance; guard tiny radii.
    let step = if r > tol.chord {
        (2.0 * (1.0 - tol.chord / r).clamp(-1.0, 1.0).acos()).min(tol.max_angle_edge)
    } else {
        tol.max_angle_edge
    }
    .max(1e-3);
    let mut n = (sweep.abs() / step).ceil().max(1.0) as usize;
    if sweep.abs() >= TAU - 1e-9 {
        n = n.max(24);
    }
    n = n.clamp(1, 4096);
    (0..=n).map(|i| c.point(t0 + sweep * (i as f64) / (n as f64))).collect()
}

fn nurbs_samples(c: &Curve3, t0: f64, t1: f64, tol: &Tol) -> Vec<DVec3> {
    let (da, db) = curve_domain(c);
    let (lo, hi) = (da.min(db), da.max(db));
    // Seed with the distinct interior knots plus `degree` samples per span.
    let mut ts: Vec<f64> = Vec::new();
    if let Curve3::Nurbs(n) = c {
        let deg = n.degree.max(1);
        let mut knots: Vec<f64> = n
            .knots
            .iter()
            .copied()
            .filter(|k| *k > lo + 1e-12 && *k < hi - 1e-12)
            .collect();
        knots.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        knots.dedup_by(|a, b| (*a - *b).abs() < 1e-12);
        let mut bounds = vec![lo];
        bounds.extend(knots);
        bounds.push(hi);
        for w in bounds.windows(2) {
            for k in 0..deg {
                ts.push(w[0] + (w[1] - w[0]) * (k as f64) / (deg as f64));
            }
        }
        ts.push(hi);
    } else {
        ts = vec![lo, hi];
    }
    // Restrict to the requested interval and orient it.
    let (a, b) = (t0.min(t1), t0.max(t1));
    let mut keep: Vec<f64> = ts.into_iter().filter(|t| *t > a + 1e-12 && *t < b - 1e-12).collect();
    keep.insert(0, a);
    keep.push(b);
    keep.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
    if keep.len() < 2 {
        keep = vec![a, b];
    }

    let mut out: Vec<DVec3> = Vec::with_capacity(keep.len() * 2);
    out.push(c.point(keep[0]));
    for w in keep.windows(2) {
        refine(c, w[0], w[1], tol, 0, &mut out);
    }
    // Ensure at least four segments so a curved edge is never a single chord.
    if out.len() < 5 {
        let n = 4usize;
        out = (0..=n).map(|i| c.point(a + (b - a) * (i as f64) / (n as f64))).collect();
    }
    if t1 < t0 {
        out.reverse();
    }
    out
}

/// Recursive midpoint subdivision on `[ta, tb]`, appending everything after `C(ta)`.
fn refine(c: &Curve3, ta: f64, tb: f64, tol: &Tol, depth: u32, out: &mut Vec<DVec3>) {
    let pa = c.point(ta);
    let pb = c.point(tb);
    let tm = 0.5 * (ta + tb);
    let pm = c.point(tm);
    let sag = pm.distance((pa + pb) * 0.5);
    let turn = angle_between(c.tangent(ta), c.tangent(tb));
    if depth < 10 && (sag > tol.chord || turn > tol.max_angle_edge) && (tb - ta).abs() > 1e-12 {
        refine(c, ta, tm, tol, depth + 1, out);
        refine(c, tm, tb, tol, depth + 1, out);
    } else {
        out.push(pb);
    }
}

#[inline]
pub(crate) fn angle_between(a: DVec3, b: DVec3) -> f64 {
    match (a.try_normalize(), b.try_normalize()) {
        (Some(x), Some(y)) => x.dot(y).clamp(-1.0, 1.0).acos(),
        _ => 0.0,
    }
}
