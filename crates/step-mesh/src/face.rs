//! Per-face pipeline: rings → parameter space → seam handling → triangulation → 3-D mesh.
//!
//! Every stage degrades instead of failing: a face that cannot be meshed produces diagnostics and
//! is skipped, and the whole pipeline runs inside `catch_unwind` because third-party triangulators
//! can panic on pathological input.

use std::panic::{AssertUnwindSafe, catch_unwind};

use glam::{DVec2, DVec3};

use crate::discretize::{EdgePolyline, Tol};
use crate::geom::Surface;
use crate::mesh::{Diag, DiagKind};
use crate::param::{Param, safe_normalize};
use crate::seam;
use crate::topo::{EdgeId, FaceId, ShapeTopology};
use crate::triangulate::{self, UvMesh, UvRing};

/// Triangles and vertex attributes of one face, in world (mm) coordinates.
#[derive(Debug, Default)]
pub(crate) struct FaceMesh {
    pub positions: Vec<DVec3>,
    pub normals: Vec<DVec3>,
    pub indices: Vec<u32>,
    /// 0 = fast path or primary CDT, 1..=4 = fallback level used.
    pub fallback: u8,
}

pub(crate) struct FaceResult {
    pub mesh: Option<FaceMesh>,
    pub diags: Vec<Diag>,
}

/// Tessellate one face, catching panics from the triangulators.
pub(crate) fn tessellate_face(
    topo: &ShapeTopology,
    fid: FaceId,
    polys: &[EdgePolyline],
    tol: &Tol,
) -> FaceResult {
    let res = catch_unwind(AssertUnwindSafe(|| tessellate_face_inner(topo, fid, polys, tol)));
    match res {
        Ok(r) => r,
        Err(_) => FaceResult {
            mesh: None,
            diags: vec![diag(topo, fid, DiagKind::Panic, 0, "panic during tessellation")],
        },
    }
}

fn diag(topo: &ShapeTopology, fid: FaceId, kind: DiagKind, level: u8, msg: &str) -> Diag {
    Diag {
        kind,
        face: Some(fid),
        src: topo.faces.get(fid.idx()).map(|f| f.src).unwrap_or(0),
        level,
        msg: msg.to_string(),
    }
}

// ------------------------------------------------------------------------------------------
// ring assembly
// ------------------------------------------------------------------------------------------

struct Ring3 {
    pts: Vec<DVec3>,
    src: Vec<Option<(EdgeId, u32)>>,
    is_outer: bool,
}

/// Walk a loop's oriented edges and concatenate the shared edge polylines.
fn build_ring(
    topo: &ShapeTopology,
    lid: crate::topo::LoopId,
    polys: &[EdgePolyline],
    tol: &Tol,
    out_diags: &mut Vec<(DiagKind, String)>,
) -> Option<Ring3> {
    let lp = topo.loops.get(lid.idx())?;
    // `gap_tol` decides whether consecutive edges are connected at all (flip / bridge);
    // `coincide_tol` decides whether two points are the *same* point. Shared edge polylines end
    // bit-exactly on their vertices, so coincidence can be tested far tighter than the file's
    // closure tolerance — otherwise real vertices of micrometre-sized sliver faces get merged.
    let gap_tol = 10.0 * tol.tol;
    let coincide_tol = (tol.tol * 1e-3).max(1e-9);
    let mut pts: Vec<DVec3> = Vec::new();
    let mut src: Vec<Option<(EdgeId, u32)>> = Vec::new();

    for oe in &lp.edges {
        let poly = polys.get(oe.edge.idx())?;
        if poly.is_degenerate() {
            continue;
        }
        let n = poly.pts.len();
        let forward = !oe.reversed;
        let take = |fwd: bool| -> (DVec3, DVec3) {
            if fwd { (poly.pts[0], poly.pts[n - 1]) } else { (poly.pts[n - 1], poly.pts[0]) }
        };
        let mut forward = forward;
        if let Some(last) = pts.last() {
            let (s, _) = take(forward);
            if last.distance(s) > gap_tol {
                let (s2, _) = take(!forward);
                if last.distance(s2) <= gap_tol {
                    forward = !forward;
                    out_diags.push((
                        DiagKind::LoopFlippedEdge,
                        format!("edge #{} flipped to close the loop", topo.edges[oe.edge.idx()].src),
                    ));
                } else {
                    out_diags.push((
                        DiagKind::LoopGap,
                        format!("gap of {:.4} mm bridged", last.distance(s.min(s2))),
                    ));
                }
            }
        }
        let idx: Vec<usize> = if forward { (0..n).collect() } else { (0..n).rev().collect() };
        let start = if pts.is_empty() {
            0
        } else {
            // Drop the shared junction point when the ends already coincide.
            usize::from(pts[pts.len() - 1].distance(poly.pts[idx[0]]) <= coincide_tol)
        };
        for &k in &idx[start..] {
            pts.push(poly.pts[k]);
            src.push(Some((oe.edge, k as u32)));
        }
    }

    // The ring is implicitly closed; drop a duplicated closing point.
    while pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) <= coincide_tol {
        pts.pop();
        src.pop();
    }
    if pts.len() < 3 {
        return None;
    }
    Some(Ring3 { pts, src, is_outer: lp.is_outer })
}

// ------------------------------------------------------------------------------------------
// main pipeline
// ------------------------------------------------------------------------------------------

fn tessellate_face_inner(
    topo: &ShapeTopology,
    fid: FaceId,
    polys: &[EdgePolyline],
    tol: &Tol,
) -> FaceResult {
    let mut diags: Vec<Diag> = Vec::new();
    let Some(face) = topo.faces.get(fid.idx()) else {
        return FaceResult { mesh: None, diags };
    };
    if let Surface::Unsupported(what) = &face.surface {
        diags.push(diag(topo, fid, DiagKind::UnsupportedSurface, 0, what));
        return FaceResult { mesh: None, diags };
    }
    let param = Param::new(&face.surface);
    let orient = if face.same_sense { 1.0 } else { -1.0 };

    // --- rings in 3-D ---------------------------------------------------------------------
    let mut kinds: Vec<(DiagKind, String)> = Vec::new();
    let mut rings3: Vec<Ring3> = Vec::new();
    for lid in &face.loops {
        if let Some(r) = build_ring(topo, *lid, polys, tol, &mut kinds) {
            rings3.push(r);
        } else if topo.loops.get(lid.idx()).map(|l| l.is_outer).unwrap_or(false) {
            kinds.push((DiagKind::DegenerateFace, "outer loop has fewer than 3 points".into()));
        }
    }
    let has_outer = rings3.iter().any(|r| r.is_outer);
    if rings3.is_empty() && !face.loops.is_empty() {
        let already_reported = kinds.iter().any(|(k, _)| *k == DiagKind::DegenerateFace);
        for (k, m) in kinds {
            diags.push(diag(topo, fid, k, 0, &m));
        }
        if !already_reported {
            diags.push(diag(topo, fid, DiagKind::DegenerateFace, 0, "no usable loops"));
        }
        return FaceResult { mesh: None, diags };
    }
    if !has_outer && !rings3.is_empty() {
        // No FACE_OUTER_BOUND: the largest ring becomes the outer one (decided in uv later).
    }

    // --- project to parameter space -----------------------------------------------------------
    let mut no_converge = 0usize;
    let mut rings: Vec<UvRing> = Vec::with_capacity(rings3.len());
    for r in &rings3 {
        let mut ur = UvRing::new(r.is_outer);
        let mut seed: Option<DVec2> = None;
        for (p, s) in r.pts.iter().zip(&r.src) {
            let (uv, ok) = param.inverse(*p, seed);
            if !ok {
                no_converge += 1;
            }
            seed = Some(uv);
            ur.push(uv, *s);
        }
        rings.push(ur);
    }
    if no_converge > 0 {
        kinds.push((
            DiagKind::InverseNoConverge,
            format!("{no_converge} point inversion(s) fell back to the seed"),
        ));
    }

    // --- fast path: closed strip on a v-ruled surface (Creo full cylinders / cones) -----------
    let uvmesh = if let Some(m) = strip_fast_path(&param, &rings) {
        Some(m)
    } else {
        // --- seams / poles --------------------------------------------------------------------
        let sr = seam::resolve(rings, &param, orient, tol);
        let rings = sr.rings;
        kinds.extend(sr.diags);
        if rings.is_empty() {
            for (k, m) in kinds {
                diags.push(diag(topo, fid, k, 0, &m));
            }
            diags.push(diag(
                topo,
                fid,
                DiagKind::DegenerateFace,
                0,
                "no rings after seam handling",
            ));
            return FaceResult { mesh: None, diags };
        }
        if matches!(face.surface, Surface::Plane { .. }) {
            planar_fast_path(&rings)
                .or_else(|| triangulate::triangulate(rings, &param, tol, true))
        } else {
            triangulate::triangulate(rings, &param, tol, true)
        }
    };

    let Some(uvmesh) = uvmesh else {
        for (k, m) in kinds {
            diags.push(diag(topo, fid, k, 0, &m));
        }
        diags.push(diag(topo, fid, DiagKind::DegenerateFace, 0, "triangulation produced nothing"));
        return FaceResult { mesh: None, diags };
    };
    for (k, m) in &uvmesh.diags {
        kinds.push((*k, m.clone()));
    }

    // --- lift to 3-D --------------------------------------------------------------------------
    let mesh = lift(&param, &uvmesh, polys, orient, &mut kinds);
    let fallback = uvmesh.fallback;
    for (k, m) in kinds {
        let level = if k == DiagKind::TriangulationFallback { fallback } else { 0 };
        diags.push(diag(topo, fid, k, level, &m));
    }
    match mesh {
        Some(mut m) => {
            m.fallback = fallback;
            FaceResult { mesh: Some(m), diags }
        }
        None => {
            diags.push(diag(topo, fid, DiagKind::DegenerateFace, 0, "no non-degenerate triangles"));
            FaceResult { mesh: None, diags }
        }
    }
}

/// Convert a uv triangulation into positions, normals and indices.
fn lift(
    param: &Param<'_>,
    m: &UvMesh,
    polys: &[EdgePolyline],
    orient: f64,
    kinds: &mut Vec<(DiagKind, String)>,
) -> Option<FaceMesh> {
    let n = m.uv.len();
    let mut positions = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut needs_avg = Vec::new();
    for i in 0..n {
        let uv = m.uv[i];
        let p = match m.src.get(i).copied().flatten() {
            Some((e, k)) => polys
                .get(e.idx())
                .and_then(|pl| pl.pts.get(k as usize).copied())
                .unwrap_or_else(|| param.eval(uv.x, uv.y)),
            None => param.eval(uv.x, uv.y),
        };
        let (_, du, dv) = param.derivs(uv.x, uv.y);
        let raw = du.cross(dv);
        let nrm = if raw.length() < 1e-14 {
            needs_avg.push(i);
            DVec3::ZERO
        } else {
            param.normal(uv.x, uv.y) * orient
        };
        positions.push(p);
        normals.push(nrm);
    }

    // Winding: the triangulators emit CCW in uv, which is the +(∂u × ∂v) side; flip for
    // `same_sense == false` faces so the normal points out of the solid.
    let flip = orient < 0.0;
    let mut indices: Vec<u32> = Vec::with_capacity(m.tris.len() * 3);
    for t in &m.tris {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if a >= n || b >= n || c >= n {
            continue;
        }
        let area = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        if area.length() < 1e-14 {
            continue; // degenerate: pole fan, duplicated seam point, …
        }
        if flip {
            indices.extend_from_slice(&[t[0], t[2], t[1]]);
        } else {
            indices.extend_from_slice(&[t[0], t[1], t[2]]);
        }
    }
    if indices.is_empty() {
        return None;
    }

    // Vertices where the analytic normal degenerates get the average of their incident faces.
    if !needs_avg.is_empty() {
        let mut acc = vec![DVec3::ZERO; n];
        for t in indices.as_chunks::<3>().0 {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let fnm = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
            acc[a] += fnm;
            acc[b] += fnm;
            acc[c] += fnm;
        }
        for i in needs_avg {
            normals[i] = safe_normalize(acc[i], DVec3::Z);
        }
    }

    // Consistency check against the analytic normals.
    let mut bad = 0usize;
    let mut tested = 0usize;
    for t in indices.as_chunks::<3>().0 {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let fnm = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        let vn = normals[a] + normals[b] + normals[c];
        if fnm.length_squared() > 1e-24 && vn.length_squared() > 1e-24 {
            tested += 1;
            if fnm.dot(vn) <= 0.0 {
                bad += 1;
            }
        }
    }
    if tested > 0 && bad * 10 >= tested * 9 {
        for t in indices.as_chunks_mut::<3>().0 {
            t.swap(1, 2);
        }
        kinds.push((
            DiagKind::WindingFlipped,
            format!("{bad}/{tested} triangles disagreed with the analytic normal"),
        ));
    }

    Some(FaceMesh { positions, normals, indices, fallback: 0 })
}

// ------------------------------------------------------------------------------------------
// fast paths
// ------------------------------------------------------------------------------------------

/// Planar faces: the parameter plane *is* the surface, so ear clipping with holes is exact.
fn planar_fast_path(rings: &[UvRing]) -> Option<UvMesh> {
    let prepared = triangulate::prepare_rings(rings.to_vec())?;
    if prepared.len() > 32 {
        return None;
    }
    let mut uv = Vec::new();
    let mut src = Vec::new();
    let mut contours: Vec<Vec<DVec2>> = Vec::new();
    for r in &prepared {
        contours.push(r.uv.clone());
        uv.extend_from_slice(&r.uv);
        src.extend_from_slice(&r.src);
    }
    let (tris, dev) = triangulate::earcut_planar(&contours)?;
    if !dev.is_finite() || dev.abs() >= 1e-3 {
        return None;
    }
    Some(UvMesh { uv, src, tris, fallback: 0, diags: Vec::new() })
}

/// Two rings that each wrap the u direction on a surface that is ruled in v (cylinder, cone,
/// extrusion): emit the quad strip between them directly.
fn strip_fast_path(param: &Param<'_>, rings: &[UvRing]) -> Option<UvMesh> {
    if !matches!(
        param.surf,
        Surface::Cylinder { .. } | Surface::Cone { .. } | Surface::Extrusion { .. }
    ) {
        return None;
    }
    if rings.len() != 2 {
        return None;
    }
    let period = param.period()[0]?;
    let mut a = rings[0].clone();
    let mut b = rings[1].clone();
    let wa = seam::unwrap_ring(&mut a.uv, [Some(period), None])[0];
    let wb = seam::unwrap_ring(&mut b.uv, [Some(period), None])[0];
    if wa.abs() != 1 || wb.abs() != 1 || a.len() != b.len() || a.len() < 3 {
        return None;
    }
    if wa < 0 {
        a.reverse();
        seam::unwrap_ring(&mut a.uv, [Some(period), None]);
    }
    if wb < 0 {
        b.reverse();
        seam::unwrap_ring(&mut b.uv, [Some(period), None]);
    }
    let n = a.len();
    let step = period / n as f64;
    // Align B's samples to A's.
    let j0 = (0..n).min_by(|x, y| {
        let dx = crate::param::wrap_half(b.uv[*x].x - a.uv[0].x, period).abs();
        let dy = crate::param::wrap_half(b.uv[*y].x - a.uv[0].x, period).abs();
        dx.partial_cmp(&dy).unwrap_or(std::cmp::Ordering::Equal)
    })?;
    for i in 0..n {
        let d = crate::param::wrap_half(b.uv[(j0 + i) % n].x - a.uv[i].x, period).abs();
        if d > 0.25 * step {
            return None;
        }
    }

    let mut uv = Vec::with_capacity(2 * n);
    let mut src = Vec::with_capacity(2 * n);
    uv.extend_from_slice(&a.uv);
    src.extend_from_slice(&a.src);
    for i in 0..n {
        let k = (j0 + i) % n;
        // Keep B's u on A's branch so the quad is not stretched across the seam.
        let mut p = b.uv[k];
        p.x = a.uv[i].x + crate::param::wrap_half(p.x - a.uv[i].x, period);
        uv.push(p);
        src.push(b.src[k]);
    }
    // v increases from A to B ⇒ (A_i, A_i+1, B_i+1, B_i) is counter-clockwise.
    let va: f64 = a.uv.iter().map(|p| p.y).sum::<f64>() / n as f64;
    let vb: f64 = uv[n..].iter().map(|p| p.y).sum::<f64>() / n as f64;
    let up = vb > va;
    let mut tris = Vec::with_capacity(2 * n);
    for i in 0..n {
        let i1 = (i + 1) % n;
        let (a0, a1) = (i as u32, i1 as u32);
        let (b0, b1) = ((n + i) as u32, (n + i1) as u32);
        if up {
            tris.push([a0, a1, b1]);
            tris.push([a0, b1, b0]);
        } else {
            tris.push([a0, b1, a1]);
            tris.push([a0, b0, b1]);
        }
    }
    Some(UvMesh {
        uv,
        src,
        tris,
        fallback: 0,
        diags: vec![(DiagKind::SeamSynthesized, "closed strip meshed directly".into())],
    })
}
