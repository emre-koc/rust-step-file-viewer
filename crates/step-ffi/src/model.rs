//! `sv_model`: a loaded STEP file flattened into render-ready buffers for the preview extension.
//!
//! One *mesh* is one `(shape, body)` pair — the unit the tessellator produces and the unit a
//! `SCNGeometry` maps onto. One *instance* is one `(assembly instance, body)` pair, carrying the
//! assembly's world transform in millimetres so identical parts share a single geometry on the
//! Swift side. Positions have the body origin folded back in (the tessellator stores them relative
//! to it), so `world * position` is the millimetre world point.

use std::ffi::CString;

use glam::{DAffine3, DMat4};
use step_mesh::{Aabb, ShapeMesh, TessParams};

pub struct Mesh {
    pub vertex_count: u32,
    /// 3 × `vertex_count`, millimetres, body origin already added.
    pub positions: Vec<f32>,
    /// 3 × `vertex_count`.
    pub normals: Vec<f32>,
    /// 4 × `vertex_count`, from `face_ranges[face_slot[v]].color`.
    pub colors: Vec<u8>,
    pub indices: Vec<u32>,
    pub edge_indices: Vec<u32>,
    pub double_sided: bool,
}

pub struct Instance {
    pub mesh: u32,
    /// Column-major 4×4, millimetres.
    pub matrix: [f32; 16],
    pub name: CString,
}

pub struct Model {
    pub meshes: Vec<Mesh>,
    pub instances: Vec<Instance>,
    pub bbox: Aabb,
}

/// Colour used when a vertex has no usable face slot.
const FALLBACK_COLOR: [u8; 4] = [180, 182, 188, 255];

fn build_mesh(body: &step_mesh::BodyMesh) -> Mesh {
    let vc = body.positions.len();
    let o = body.origin;
    let mut positions = Vec::with_capacity(vc * 3);
    for p in &body.positions {
        positions.push((p[0] as f64 + o.x) as f32);
        positions.push((p[1] as f64 + o.y) as f32);
        positions.push((p[2] as f64 + o.z) as f32);
    }
    let mut normals = Vec::with_capacity(vc * 3);
    for n in &body.normals {
        normals.extend_from_slice(n);
    }
    // A body without normals still has to hand SceneKit a full-length source.
    normals.resize(vc * 3, 0.0);

    let mut colors = Vec::with_capacity(vc * 4);
    for v in 0..vc {
        let c = body
            .face_slot
            .get(v)
            .and_then(|&s| body.face_ranges.get(s as usize))
            .map(|f| f.color)
            .unwrap_or(FALLBACK_COLOR);
        colors.extend_from_slice(&c);
    }

    // Never hand out an index that would read past the vertex arrays.
    let vc32 = vc as u32;
    let indices: Vec<u32> = if body.indices.iter().all(|&i| i < vc32) {
        body.indices.clone()
    } else {
        body.indices.chunks_exact(3).filter(|t| t.iter().all(|&i| i < vc32)).flatten().copied().collect()
    };
    let edge_indices: Vec<u32> = if body.edge_indices.iter().all(|&i| i < vc32) {
        body.edge_indices.clone()
    } else {
        body.edge_indices.chunks_exact(2).filter(|t| t.iter().all(|&i| i < vc32)).flatten().copied().collect()
    };

    Mesh { vertex_count: vc32, positions, normals, colors, indices, edge_indices, double_sided: body.double_sided }
}

fn matrix_of(world: &DAffine3) -> [f32; 16] {
    let cols = DMat4::from(*world).to_cols_array();
    std::array::from_fn(|i| cols[i] as f32)
}

impl Model {
    pub fn build(structure: &step_brep::Structure, meshes: &[Option<ShapeMesh>]) -> Model {
        let asm = &structure.assembly;
        // Meshes are laid out shape-major so an instance of shape `s` maps onto the contiguous
        // range `start[s] .. start[s] + count[s]`.
        let mut start = vec![0u32; meshes.len()];
        let mut count = vec![0u32; meshes.len()];
        let mut out_meshes: Vec<Mesh> = Vec::new();
        for (s, m) in meshes.iter().enumerate() {
            start[s] = out_meshes.len() as u32;
            if let Some(m) = m {
                for body in &m.bodies {
                    if body.positions.is_empty() || body.indices.is_empty() {
                        continue;
                    }
                    out_meshes.push(build_mesh(body));
                    count[s] += 1;
                }
            }
        }

        let mut out_instances = Vec::new();
        let mut bbox = Aabb::EMPTY;
        for inst in &asm.instances {
            let s = inst.shape.0 as usize;
            if s >= meshes.len() || count[s] == 0 {
                continue;
            }
            let name = asm.nodes.get(inst.node.0 as usize).map(|n| n.name.as_str()).unwrap_or("");
            let cname = CString::new(name.replace('\0', " ")).unwrap_or_else(|_| CString::new("").unwrap());
            let matrix = matrix_of(&inst.world);
            if let Some(sm) = meshes[s].as_ref() {
                bbox.union(&sm.bbox.transformed(&inst.world));
            }
            for k in 0..count[s] {
                out_instances.push(Instance { mesh: start[s] + k, matrix, name: cname.clone() });
            }
        }
        Model { meshes: out_meshes, instances: out_instances, bbox }
    }

    /// Per-instance oriented boxes, for the degraded thumbnail paths.
    pub fn instance_boxes(structure: &step_brep::Structure, meshes: &[Option<ShapeMesh>]) -> Vec<(Aabb, DAffine3)> {
        let asm = &structure.assembly;
        let mut out = Vec::new();
        for inst in &asm.instances {
            if let Some(Some(m)) = meshes.get(inst.shape.0 as usize) {
                for body in &m.bodies {
                    if !body.bbox.is_empty() {
                        out.push((body.bbox, inst.world));
                    }
                }
                if m.bodies.is_empty() && !m.bbox.is_empty() {
                    out.push((m.bbox, inst.world));
                }
            }
        }
        out
    }
}

/// `quality` code from the C API → tessellation parameters.
pub fn params_for(quality: u32) -> TessParams {
    match quality {
        0 => TessParams::COARSE,
        2 => TessParams::FINE,
        _ => TessParams::PREVIEW,
    }
}
