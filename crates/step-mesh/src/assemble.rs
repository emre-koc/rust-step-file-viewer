//! Parallel driver: edges → faces → bodies, plus feature edges, welding and statistics.

use std::time::Instant;

use glam::DVec3;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::discretize::{EdgePolyline, Tol, angle_between, discretize_all};
use crate::face::{FaceMesh, tessellate_face};
use crate::mesh::{Aabb, BodyMesh, Diag, FaceRange, ShapeMesh, TessParams};
use crate::param::{Param, cost_rank, surface_kind};
use crate::topo::{BodyId, EdgeId, FaceId, ShapeTopology};

/// Feature-edge threshold: adjacent faces whose normals differ by more than this are creases.
const FEATURE_ANGLE: f64 = 20.0;

pub fn tessellate(topo: &ShapeTopology, params: &TessParams) -> ShapeMesh {
    let mut out = ShapeMesh::default();
    let bbox = topo.vertex_bbox();
    let tol = Tol::new(params, bbox.diagonal(), topo.tol);
    out.stats.chord = tol.chord as f32;
    out.stats.faces = topo.faces.len() as u32;
    if topo.faces.is_empty() {
        return out;
    }

    // --- 1. edges -----------------------------------------------------------------------------
    let t0 = Instant::now();
    let polys = discretize_all(topo, &tol);
    out.stats.edge_ms = t0.elapsed().as_secs_f32() * 1e3;

    // --- 2. faces -----------------------------------------------------------------------------
    let t1 = Instant::now();
    // Start the expensive surfaces first so work stealing balances out.
    let mut order: Vec<u32> = (0..topo.faces.len() as u32).collect();
    order.sort_by_key(|i| cost_rank(&topo.faces[*i as usize].surface));
    let results: Vec<(u32, Option<FaceMesh>, Vec<Diag>)> = order
        .par_iter()
        .map(|i| {
            let r = tessellate_face(topo, FaceId(*i), &polys, &tol);
            (*i, r.mesh, r.diags)
        })
        .collect();
    out.stats.face_ms = t1.elapsed().as_secs_f32() * 1e3;

    let mut meshes: Vec<Option<FaceMesh>> = (0..topo.faces.len()).map(|_| None).collect();
    for (i, m, d) in results {
        if let Some(m) = &m {
            if m.fallback == 0 {
                out.stats.faces_ok += 1;
            } else {
                out.stats.faces_fallback += 1;
            }
        } else {
            out.stats.faces_skipped += 1;
        }
        meshes[i as usize] = m;
        out.diags.extend(d);
    }
    out.diags.sort_by_key(|d| (d.face.map(|f| f.0).unwrap_or(u32::MAX), d.kind));

    // --- 3. bodies ----------------------------------------------------------------------------
    let t2 = Instant::now();
    let edge_faces = if params.feature_edges { edge_face_map(topo) } else { FxHashMap::default() };
    out.bodies = (0..topo.bodies.len())
        .into_par_iter()
        .map(|bi| build_body(topo, BodyId(bi as u32), &meshes, &polys, &edge_faces, params))
        .collect();
    out.stats.assemble_ms = t2.elapsed().as_secs_f32() * 1e3;

    for b in &out.bodies {
        out.bbox.union(&b.bbox);
        out.stats.vertices += b.positions.len() as u32;
        out.stats.triangles += b.triangle_count() as u32;
    }
    out
}

fn edge_face_map(topo: &ShapeTopology) -> FxHashMap<EdgeId, Vec<FaceId>> {
    let mut m: FxHashMap<EdgeId, Vec<FaceId>> = FxHashMap::default();
    for (fi, f) in topo.faces.iter().enumerate() {
        for lid in &f.loops {
            let Some(lp) = topo.loops.get(lid.idx()) else { continue };
            for oe in &lp.edges {
                let e = m.entry(oe.edge).or_default();
                if !e.contains(&FaceId(fi as u32)) {
                    e.push(FaceId(fi as u32));
                }
            }
        }
    }
    m
}

#[inline]
fn key(p: DVec3) -> [u64; 3] {
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

#[inline]
fn key_f32(p: [f32; 3]) -> [u32; 3] {
    // Normalise -0.0 so it hashes with +0.0.
    [(p[0] + 0.0).to_bits(), (p[1] + 0.0).to_bits(), (p[2] + 0.0).to_bits()]
}

fn build_body(
    topo: &ShapeTopology,
    bid: BodyId,
    meshes: &[Option<FaceMesh>],
    polys: &[EdgePolyline],
    edge_faces: &FxHashMap<EdgeId, Vec<FaceId>>,
    params: &TessParams,
) -> BodyMesh {
    let Some(body) = topo.bodies.get(bid.idx()) else { return BodyMesh::default() };
    let mut faces: Vec<FaceId> = Vec::new();
    let mut all_closed = true;
    for sid in &body.shells {
        let Some(sh) = topo.shells.get(sid.idx()) else { continue };
        all_closed &= sh.closed;
        faces.extend(sh.faces.iter().copied());
    }

    let mut positions: Vec<DVec3> = Vec::new();
    let mut normals: Vec<DVec3> = Vec::new();
    let mut face_slot: Vec<u32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut face_ranges: Vec<FaceRange> = Vec::new();
    let mut bbox = Aabb::EMPTY;

    for fid in &faces {
        let Some(m) = meshes.get(fid.idx()).and_then(|x| x.as_ref()) else { continue };
        let Some(f) = topo.faces.get(fid.idx()) else { continue };
        let base = positions.len() as u32;
        let slot = face_ranges.len() as u32;
        let start = indices.len() as u32;
        for (p, n) in m.positions.iter().zip(&m.normals) {
            positions.push(*p);
            normals.push(*n);
            face_slot.push(slot);
            bbox.extend(*p);
        }
        indices.extend(m.indices.iter().map(|i| i + base));
        face_ranges.push(FaceRange {
            face: *fid,
            src: f.src,
            indices: start..indices.len() as u32,
            color: f.color,
            surface_kind: surface_kind(&f.surface),
        });
    }

    // --- feature edges ------------------------------------------------------------------------
    let mut edge_indices: Vec<u32> = Vec::new();
    if params.feature_edges && !positions.is_empty() {
        let mut lookup: FxHashMap<[u64; 3], u32> = FxHashMap::default();
        for (i, p) in positions.iter().enumerate() {
            lookup.entry(key(*p)).or_insert(i as u32);
        }
        let in_body: FxHashSet<FaceId> = faces.iter().copied().collect();
        let mut seen: FxHashSet<EdgeId> = FxHashSet::default();
        for fid in &faces {
            let Some(f) = topo.faces.get(fid.idx()) else { continue };
            for lid in &f.loops {
                let Some(lp) = topo.loops.get(lid.idx()) else { continue };
                for oe in &lp.edges {
                    if !seen.insert(oe.edge) {
                        continue;
                    }
                    let adj: Vec<FaceId> = edge_faces
                        .get(&oe.edge)
                        .map(|v| v.iter().copied().filter(|f| in_body.contains(f)).collect())
                        .unwrap_or_default();
                    if !is_feature(topo, oe.edge, &adj, polys) {
                        continue;
                    }
                    let Some(pl) = polys.get(oe.edge.idx()) else { continue };
                    for w in pl.pts.windows(2) {
                        if let (Some(a), Some(b)) = (lookup.get(&key(w[0])), lookup.get(&key(w[1])))
                        {
                            edge_indices.push(*a);
                            edge_indices.push(*b);
                        }
                    }
                }
            }
        }
    }

    // --- to f32, relative to the body origin --------------------------------------------------
    let origin = if bbox.is_empty() { DVec3::ZERO } else { bbox.center() };
    let mut pos32: Vec<[f32; 3]> = positions
        .iter()
        .map(|p| {
            let q = *p - origin;
            [q.x as f32, q.y as f32, q.z as f32]
        })
        .collect();
    let mut nrm32: Vec<[f32; 3]> =
        normals.iter().map(|n| [n.x as f32, n.y as f32, n.z as f32]).collect();

    if params.weld && !pos32.is_empty() {
        let mut map: FxHashMap<[u32; 3], u32> = FxHashMap::default();
        let mut remap = vec![0u32; pos32.len()];
        let mut np: Vec<[f32; 3]> = Vec::with_capacity(pos32.len());
        let mut nn: Vec<[f32; 3]> = Vec::with_capacity(pos32.len());
        let mut ns: Vec<u32> = Vec::with_capacity(pos32.len());
        for i in 0..pos32.len() {
            let k = key_f32(pos32[i]);
            match map.get(&k) {
                Some(j) => remap[i] = *j,
                None => {
                    let j = np.len() as u32;
                    map.insert(k, j);
                    remap[i] = j;
                    np.push(pos32[i]);
                    nn.push(nrm32[i]);
                    ns.push(face_slot[i]);
                }
            }
        }
        for i in indices.iter_mut().chain(edge_indices.iter_mut()) {
            *i = remap[*i as usize];
        }
        pos32 = np;
        nrm32 = nn;
        face_slot = ns;
    }

    BodyMesh {
        body: bid,
        name: body.name.clone(),
        origin,
        positions: pos32,
        normals: nrm32,
        face_slot,
        indices,
        face_ranges,
        edge_indices,
        double_sided: body.kind == crate::topo::BodyKind::Sheet || !all_closed,
        bbox,
    }
}

/// A boundary edge, or a crease between two faces whose normals differ by more than 20°.
fn is_feature(
    topo: &ShapeTopology,
    eid: EdgeId,
    adj: &[FaceId],
    polys: &[EdgePolyline],
) -> bool {
    if adj.len() != 2 {
        return true;
    }
    let Some(pl) = polys.get(eid.idx()) else { return false };
    if pl.pts.len() < 2 {
        return false;
    }
    let p = pl.pts[pl.pts.len() / 2];
    let mut ns = [DVec3::ZERO; 2];
    for (k, fid) in adj.iter().enumerate() {
        let Some(f) = topo.faces.get(fid.idx()) else { return true };
        let param = Param::new(&f.surface);
        let (uv, _) = param.inverse(p, None);
        let orient = if f.same_sense { 1.0 } else { -1.0 };
        ns[k] = param.normal(uv.x, uv.y) * orient;
    }
    angle_between(ns[0], ns[1]).to_degrees() > FEATURE_ANGLE
}
