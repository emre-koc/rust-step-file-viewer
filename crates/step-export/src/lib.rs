//! Export tessellated models. STL and OBJ are flattened into world space; glTF keeps the assembly
//! hierarchy and instancing (one glTF mesh per body, referenced by every instance node).

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::Path;

use glam::{DAffine3, DMat3, DVec3, Vec3};
use step_brep::{Assembly, NodeId, ShapeId};
use step_mesh::{BodyMesh, ShapeMesh};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported extension '{0}' (use stl, obj, glb or gltf)")]
    Extension(String),
    #[error("nothing to export")]
    Empty,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Where to find the tessellation of shape `i` (index = `ShapeId.0`).
pub trait MeshLookup {
    fn mesh(&self, shape: u32) -> Option<&ShapeMesh>;
}

impl MeshLookup for [Option<ShapeMesh>] {
    fn mesh(&self, shape: u32) -> Option<&ShapeMesh> {
        self.get(shape as usize).and_then(|m| m.as_ref())
    }
}
impl MeshLookup for Vec<Option<ShapeMesh>> {
    fn mesh(&self, shape: u32) -> Option<&ShapeMesh> {
        self.as_slice().mesh(shape)
    }
}
impl MeshLookup for [Option<std::sync::Arc<ShapeMesh>>] {
    fn mesh(&self, shape: u32) -> Option<&ShapeMesh> {
        self.get(shape as usize).and_then(|m| m.as_deref())
    }
}
impl MeshLookup for Vec<Option<std::sync::Arc<ShapeMesh>>> {
    fn mesh(&self, shape: u32) -> Option<&ShapeMesh> {
        self.as_slice().mesh(shape)
    }
}

/// What to export.
pub struct Scene<'a> {
    pub assembly: &'a Assembly,
    pub meshes: &'a dyn MeshLookup,
    /// Restrict to these instance indices (into `assembly.instances`); `None` = all.
    pub only_instances: Option<&'a [u32]>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ExportStats {
    pub bodies: u32,
    pub triangles: u64,
    pub vertices: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOpts {
    /// ASCII STL / text glTF.
    pub ascii: bool,
    /// glTF: convert Z-up mm to Y-up metres (glTF convention). Off = raw mm, Z-up.
    pub gltf_convention: bool,
}

impl Default for ExportOpts {
    fn default() -> Self {
        ExportOpts { ascii: false, gltf_convention: true }
    }
}

/// Dispatch on the output extension.
pub fn export(path: &Path, scene: &Scene<'_>, opts: ExportOpts) -> Result<ExportStats> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "stl" => write_stl(path, scene, opts.ascii),
        "obj" => write_obj(path, scene),
        "glb" => write_gltf(path, scene, opts.gltf_convention, true),
        "gltf" => write_gltf(path, scene, opts.gltf_convention, false),
        other => Err(Error::Extension(other.to_string())),
    }
}

/// Iterate (instance index, body mesh, world transform including the body origin).
fn placed_bodies<'a>(scene: &Scene<'a>) -> impl Iterator<Item = (u32, &'a BodyMesh, DAffine3)> + 'a {
    let asm: &'a Assembly = scene.assembly;
    let meshes: &'a dyn MeshLookup = scene.meshes;
    let indices: Vec<u32> = match scene.only_instances {
        Some(v) => v.to_vec(),
        None => (0..asm.instances.len() as u32).collect(),
    };
    indices.into_iter().flat_map(move |i| {
        let inst = &asm.instances[i as usize];
        let mesh = meshes.mesh(inst.shape.0);
        let world = inst.world;
        mesh.into_iter().flat_map(move |m| m.bodies.iter().map(move |b| (i, b, world * DAffine3::from_translation(b.origin))))
    })
}

fn transform_point(m: &DAffine3, p: [f32; 3]) -> DVec3 {
    m.transform_point3(DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64))
}

fn transform_normal(rot: &DMat3, n: [f32; 3]) -> DVec3 {
    (*rot * DVec3::new(n[0] as f64, n[1] as f64, n[2] as f64)).normalize_or_zero()
}

// ---------------------------------------------------------------- STL

pub fn write_stl(path: &Path, scene: &Scene<'_>, ascii: bool) -> Result<ExportStats> {
    let mut stats = ExportStats::default();
    let items: Vec<_> = placed_bodies(scene).collect();
    if items.is_empty() {
        return Err(Error::Empty);
    }
    let total: u64 = items.iter().map(|(_, b, _)| b.triangle_count() as u64).sum();
    let mut w = BufWriter::new(std::fs::File::create(path)?);
    if ascii {
        writeln!(w, "solid stepview")?;
    } else {
        let mut header = [0u8; 80];
        let tag = b"stepview binary STL (mm)";
        header[..tag.len()].copy_from_slice(tag);
        w.write_all(&header)?;
        w.write_all(&(total as u32).to_le_bytes())?;
    }
    for (_, b, world) in &items {
        stats.bodies += 1;
        stats.vertices += b.positions.len() as u64;
        let flip = world.matrix3.determinant() < 0.0;
        for t in b.indices.as_chunks::<3>().0 {
            let (i0, i1, i2) = if flip { (t[0], t[2], t[1]) } else { (t[0], t[1], t[2]) };
            let p0 = transform_point(world, b.positions[i0 as usize]);
            let p1 = transform_point(world, b.positions[i1 as usize]);
            let p2 = transform_point(world, b.positions[i2 as usize]);
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            if ascii {
                writeln!(w, " facet normal {} {} {}", n.x as f32, n.y as f32, n.z as f32)?;
                writeln!(w, "  outer loop")?;
                for p in [p0, p1, p2] {
                    writeln!(w, "   vertex {} {} {}", p.x as f32, p.y as f32, p.z as f32)?;
                }
                writeln!(w, "  endloop\n endfacet")?;
            } else {
                let mut rec = [0u8; 50];
                let mut o = 0;
                for v in [n, p0, p1, p2] {
                    for c in [v.x, v.y, v.z] {
                        rec[o..o + 4].copy_from_slice(&(c as f32).to_le_bytes());
                        o += 4;
                    }
                }
                w.write_all(&rec)?;
            }
            stats.triangles += 1;
        }
    }
    if ascii {
        writeln!(w, "endsolid stepview")?;
    }
    w.flush()?;
    stats.bytes = std::fs::metadata(path)?.len();
    Ok(stats)
}

// ---------------------------------------------------------------- OBJ

pub fn write_obj(path: &Path, scene: &Scene<'_>) -> Result<ExportStats> {
    let mut stats = ExportStats::default();
    let items: Vec<_> = placed_bodies(scene).collect();
    if items.is_empty() {
        return Err(Error::Empty);
    }
    let mtl_path = path.with_extension("mtl");
    let mtl_name = mtl_path.file_name().and_then(|s| s.to_str()).unwrap_or("stepview.mtl").to_string();
    let mut w = BufWriter::new(std::fs::File::create(path)?);
    writeln!(w, "# stepview OBJ export (mm, Z-up)")?;
    writeln!(w, "mtllib {mtl_name}")?;
    let mut materials: HashMap<[u8; 4], String> = HashMap::new();
    let mut base: u64 = 1;
    for (inst, b, world) in &items {
        let node = &scene.assembly.nodes[scene.assembly.instances[*inst as usize].node.0 as usize];
        let name = sanitize(&format!("{}__{}", node.path, b.name));
        writeln!(w, "o {name}")?;
        let rot = world.matrix3;
        for p in &b.positions {
            let q = transform_point(world, *p);
            writeln!(w, "v {} {} {}", q.x as f32, q.y as f32, q.z as f32)?;
        }
        for n in &b.normals {
            let q = transform_normal(&rot, *n);
            writeln!(w, "vn {} {} {}", q.x as f32, q.y as f32, q.z as f32)?;
        }
        let flip = rot.determinant() < 0.0;
        for fr in &b.face_ranges {
            let mat = materials.entry(fr.color).or_insert_with(|| format!("c_{:02x}{:02x}{:02x}{:02x}", fr.color[0], fr.color[1], fr.color[2], fr.color[3])).clone();
            writeln!(w, "usemtl {mat}")?;
            let s = fr.indices.start as usize;
            let e = (fr.indices.end as usize).min(b.indices.len());
            for t in b.indices[s..e].as_chunks::<3>().0 {
                let (a, bb, c) = if flip { (t[0], t[2], t[1]) } else { (t[0], t[1], t[2]) };
                writeln!(w, "f {0}//{0} {1}//{1} {2}//{2}", base + a as u64, base + bb as u64, base + c as u64)?;
                stats.triangles += 1;
            }
        }
        base += b.positions.len() as u64;
        stats.bodies += 1;
        stats.vertices += b.positions.len() as u64;
    }
    w.flush()?;
    let mut m = BufWriter::new(std::fs::File::create(&mtl_path)?);
    let mut mats: Vec<_> = materials.iter().collect();
    mats.sort_by(|a, b| a.1.cmp(b.1));
    for (rgba, name) in mats {
        writeln!(m, "newmtl {name}")?;
        writeln!(m, "Kd {:.4} {:.4} {:.4}", rgba[0] as f32 / 255.0, rgba[1] as f32 / 255.0, rgba[2] as f32 / 255.0)?;
        writeln!(m, "d {:.4}", rgba[3] as f32 / 255.0)?;
    }
    m.flush()?;
    stats.bytes = std::fs::metadata(path)?.len();
    Ok(stats)
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect()
}

// ---------------------------------------------------------------- glTF

struct GltfBuilder {
    bin: Vec<u8>,
    buffer_views: Vec<serde_json::Value>,
    accessors: Vec<serde_json::Value>,
}

impl GltfBuilder {
    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn push_view(&mut self, data: &[u8], target: Option<u32>, stride: Option<u32>) -> usize {
        self.align();
        let offset = self.bin.len();
        self.bin.extend_from_slice(data);
        let mut v = serde_json::json!({ "buffer": 0, "byteOffset": offset, "byteLength": data.len() });
        if let Some(t) = target {
            v["target"] = t.into();
        }
        if let Some(s) = stride {
            v["byteStride"] = s.into();
        }
        self.buffer_views.push(v);
        self.buffer_views.len() - 1
    }

    fn push_accessor(&mut self, view: usize, component: u32, count: usize, ty: &str, normalized: bool, minmax: Option<([f32; 3], [f32; 3])>) -> usize {
        let mut a = serde_json::json!({ "bufferView": view, "componentType": component, "count": count, "type": ty });
        if normalized {
            a["normalized"] = true.into();
        }
        if let Some((mn, mx)) = minmax {
            a["min"] = serde_json::json!(mn);
            a["max"] = serde_json::json!(mx);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }
}

const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const UNSIGNED_BYTE: u32 = 5121;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

pub fn write_gltf(path: &Path, scene: &Scene<'_>, convention: bool, binary: bool) -> Result<ExportStats> {
    let asm = scene.assembly;
    let mut stats = ExportStats::default();
    let mut g = GltfBuilder { bin: Vec::new(), buffer_views: Vec::new(), accessors: Vec::new() };
    let mut meshes: Vec<serde_json::Value> = Vec::new();
    // mesh index per (shape, body)
    let mut mesh_of: HashMap<(u32, usize), usize> = HashMap::new();
    let unit = if convention { 0.001f32 } else { 1.0 };

    // Which shapes are needed?
    let inst_filter: Option<std::collections::HashSet<u32>> = scene.only_instances.map(|v| v.iter().copied().collect());
    let mut used_shapes: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for (i, inst) in asm.instances.iter().enumerate() {
        if inst_filter.as_ref().is_none_or(|f| f.contains(&(i as u32))) {
            used_shapes.insert(inst.shape.0);
        }
    }

    for shape in &used_shapes {
        let Some(sm) = scene.meshes.mesh(*shape) else { continue };
        for (bi, b) in sm.bodies.iter().enumerate() {
            if b.indices.is_empty() {
                continue;
            }
            // positions (relative to origin, scaled)
            let mut pos = Vec::with_capacity(b.positions.len() * 12);
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &b.positions {
                let q = [(p[0] + b.origin.x as f32) * unit, (p[1] + b.origin.y as f32) * unit, (p[2] + b.origin.z as f32) * unit];
                for k in 0..3 {
                    mn[k] = mn[k].min(q[k]);
                    mx[k] = mx[k].max(q[k]);
                    pos.extend_from_slice(&q[k].to_le_bytes());
                }
            }
            let pv = g.push_view(&pos, Some(ARRAY_BUFFER), Some(12));
            let pa = g.push_accessor(pv, FLOAT, b.positions.len(), "VEC3", false, Some((mn, mx)));
            let mut nrm = Vec::with_capacity(b.normals.len() * 12);
            for n in &b.normals {
                let v = Vec3::from(*n).normalize_or_zero();
                for c in v.to_array() {
                    nrm.extend_from_slice(&c.to_le_bytes());
                }
            }
            let nv = g.push_view(&nrm, Some(ARRAY_BUFFER), Some(12));
            let na = g.push_accessor(nv, FLOAT, b.normals.len(), "VEC3", false, None);
            let mut col = Vec::with_capacity(b.face_slot.len() * 4);
            for slot in &b.face_slot {
                let c = b.face_ranges.get(*slot as usize).map(|f| f.color).unwrap_or([200, 200, 200, 255]);
                col.extend_from_slice(&c);
            }
            let cv = g.push_view(&col, Some(ARRAY_BUFFER), Some(4));
            let ca = g.push_accessor(cv, UNSIGNED_BYTE, b.face_slot.len(), "VEC4", true, None);
            let mut idx = Vec::with_capacity(b.indices.len() * 4);
            for i in &b.indices {
                idx.extend_from_slice(&i.to_le_bytes());
            }
            let iv = g.push_view(&idx, Some(ELEMENT_ARRAY_BUFFER), None);
            let ia = g.push_accessor(iv, UNSIGNED_INT, b.indices.len(), "SCALAR", false, None);
            let mesh = serde_json::json!({
                "name": b.name,
                "primitives": [{
                    "attributes": { "POSITION": pa, "NORMAL": na, "COLOR_0": ca },
                    "indices": ia,
                    "material": if b.double_sided { 1 } else { 0 },
                    "mode": 4
                }]
            });
            meshes.push(mesh);
            mesh_of.insert((*shape, bi), meshes.len() - 1);
            stats.triangles += b.triangle_count() as u64;
            stats.vertices += b.positions.len() as u64;
            stats.bodies += 1;
        }
    }
    if meshes.is_empty() {
        return Err(Error::Empty);
    }

    // nodes: mirror the assembly tree; body meshes as child nodes of the instance node
    let mut nodes: Vec<serde_json::Value> = Vec::new();
    let mut node_index: HashMap<NodeId, usize> = HashMap::new();
    fn matrix_json(m: &DAffine3, unit: f64) -> serde_json::Value {
        let c = m.to_cols_array_2d(); // 4 columns of 3
        let t = m.translation * unit;
        serde_json::json!([
            c[0][0], c[0][1], c[0][2], 0.0,
            c[1][0], c[1][1], c[1][2], 0.0,
            c[2][0], c[2][1], c[2][2], 0.0,
            t.x, t.y, t.z, 1.0
        ])
    }
    // create nodes for every assembly node in tree order
    for (i, n) in asm.nodes.iter().enumerate() {
        let mut j = serde_json::json!({ "name": n.name });
        if n.parent.is_some() {
            j["matrix"] = matrix_json(&n.local, unit as f64);
        }
        nodes.push(j);
        node_index.insert(NodeId(i as u32), nodes.len() - 1);
    }
    // children + meshes
    for (i, n) in asm.nodes.iter().enumerate() {
        let mut children: Vec<usize> = n.children.iter().map(|c| node_index[c]).collect();
        for shape in n.shapes.iter() {
            let ShapeId(s) = *shape;
            let Some(sm) = scene.meshes.mesh(s) else { continue };
            for bi in 0..sm.bodies.len() {
                if let Some(&mi) = mesh_of.get(&(s, bi)) {
                    nodes.push(serde_json::json!({ "name": sm.bodies[bi].name, "mesh": mi }));
                    children.push(nodes.len() - 1);
                }
            }
        }
        if !children.is_empty() {
            nodes[node_index[&NodeId(i as u32)]]["children"] = serde_json::json!(children);
        }
    }
    // root: Z-up mm → Y-up m rotation node
    let root_children: Vec<usize> = asm.roots.iter().map(|r| node_index[r]).collect();
    let scene_roots: Vec<usize> = if convention {
        // rotate -90° about X: (x, y, z) -> (x, z, -y)
        nodes.push(serde_json::json!({
            "name": "Z-up to Y-up",
            "matrix": [1,0,0,0, 0,0,-1,0, 0,1,0,0, 0,0,0,1],
            "children": root_children
        }));
        vec![nodes.len() - 1]
    } else {
        root_children
    };

    g.align();
    let mut root = serde_json::json!({
        "asset": { "version": "2.0", "generator": "stepview" },
        "scene": 0,
        "scenes": [{ "nodes": scene_roots }],
        "nodes": nodes,
        "meshes": meshes,
        "materials": [
            { "name": "solid", "pbrMetallicRoughness": { "baseColorFactor": [1,1,1,1], "metallicFactor": 0.0, "roughnessFactor": 0.6 } },
            { "name": "sheet", "doubleSided": true, "pbrMetallicRoughness": { "baseColorFactor": [1,1,1,1], "metallicFactor": 0.0, "roughnessFactor": 0.6 } }
        ],
        "bufferViews": g.buffer_views,
        "accessors": g.accessors,
    });
    if binary {
        root["buffers"] = serde_json::json!([{ "byteLength": g.bin.len() }]);
        let mut json = serde_json::to_vec(&root)?;
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + g.bin.len();
        let mut w = BufWriter::new(std::fs::File::create(path)?);
        w.write_all(b"glTF")?;
        w.write_all(&2u32.to_le_bytes())?;
        w.write_all(&(total as u32).to_le_bytes())?;
        w.write_all(&(json.len() as u32).to_le_bytes())?;
        w.write_all(b"JSON")?;
        w.write_all(&json)?;
        w.write_all(&(g.bin.len() as u32).to_le_bytes())?;
        w.write_all(b"BIN\0")?;
        w.write_all(&g.bin)?;
        w.flush()?;
    } else {
        let bin_name = path.with_extension("bin");
        std::fs::write(&bin_name, &g.bin)?;
        root["buffers"] = serde_json::json!([{ "byteLength": g.bin.len(), "uri": bin_name.file_name().and_then(|s| s.to_str()).unwrap_or("model.bin") }]);
        std::fs::write(path, serde_json::to_vec_pretty(&root)?)?;
    }
    stats.bytes = std::fs::metadata(path)?.len();
    Ok(stats)
}
