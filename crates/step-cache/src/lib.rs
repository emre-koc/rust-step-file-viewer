//! Mesh cache: `<cache_dir>/stepview/v1/<blake3(file)>-<params>-<tess version>.stpc`.
//!
//! Layout: `STPC` magic, u32 format version, u32 JSON length, JSON metadata (everything except the
//! big arrays), then a 16-byte-aligned binary blob holding positions / normals / face slots /
//! indices / edge indices per body. A read is one mmap plus memcpy into the `ShapeMesh` vectors.

use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use step_mesh::mesh::{Aabb, BodyMesh, Diag, FaceRange, ShapeMesh, TessParams, TessStats};
use step_mesh::topo::BodyId;
use thiserror::Error;

/// Bump when the tessellator's output changes in a way that should invalidate caches.
pub const TESS_VERSION: u32 = 1;
const FORMAT_VERSION: u32 = 1;
const MAGIC: &[u8; 4] = b"STPC";

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not a cache file")]
    BadMagic,
    #[error("cache format version mismatch")]
    Version,
    #[error("cache file truncated")]
    Truncated,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Default cache directory (`~/Library/Caches/stepview/v1` on macOS).
pub fn default_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("stepview").join("v1"))
}

/// blake3 of the whole file (parallel).
pub fn file_hash(bytes: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update_rayon(bytes);
    *h.finalize().as_bytes()
}

pub fn cache_path(dir: &Path, hash: &[u8; 32], params: &TessParams) -> PathBuf {
    let hex: String = hash[..8].iter().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{hex}-{:016x}-{TESS_VERSION}.stpc", params.key_bits()))
}

#[derive(Serialize, Deserialize)]
struct ArrayRef {
    off: u64,
    len: u64,
}

#[derive(Serialize, Deserialize)]
struct BodyMeta {
    body: BodyId,
    name: String,
    origin: [f64; 3],
    double_sided: bool,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
    face_ranges: Vec<FaceRange>,
    positions: ArrayRef,
    normals: ArrayRef,
    face_slot: ArrayRef,
    indices: ArrayRef,
    edge_indices: ArrayRef,
}

#[derive(Serialize, Deserialize)]
struct ShapeMeta {
    src_rep: u32,
    present: bool,
    bbox_min: [f64; 3],
    bbox_max: [f64; 3],
    stats: TessStats,
    diags: Vec<Diag>,
    bodies: Vec<BodyMeta>,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    file_hash: [u8; 32],
    params: TessParams,
    tess_version: u32,
    source_name: String,
    shapes: Vec<ShapeMeta>,
}

fn aabb_arrays(b: &Aabb) -> ([f64; 3], [f64; 3]) {
    (b.min.to_array(), b.max.to_array())
}

struct Blob(Vec<u8>);

impl Blob {
    fn push<T: bytemuck::Pod>(&mut self, data: &[T]) -> ArrayRef {
        while !self.0.len().is_multiple_of(16) {
            self.0.push(0);
        }
        let off = self.0.len() as u64;
        let bytes: &[u8] = bytemuck::cast_slice(data);
        self.0.extend_from_slice(bytes);
        ArrayRef { off, len: data.len() as u64 }
    }
}

/// Write meshes (indexed by shape) atomically. `src_reps[i]` is the STEP id of shape i's
/// representation, used to check alignment on read.
pub fn store(dir: &Path, hash: &[u8; 32], params: &TessParams, source_name: &str, meshes: &[Option<ShapeMesh>], src_reps: &[u32]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = cache_path(dir, hash, params);
    let mut blob = Blob(Vec::new());
    let mut shapes = Vec::with_capacity(meshes.len());
    for (i, m) in meshes.iter().enumerate() {
        let src_rep = src_reps.get(i).copied().unwrap_or(0);
        match m {
            None => shapes.push(ShapeMeta { src_rep, present: false, bbox_min: [0.0; 3], bbox_max: [0.0; 3], stats: TessStats::default(), diags: Vec::new(), bodies: Vec::new() }),
            Some(sm) => {
                let (bmin, bmax) = aabb_arrays(&sm.bbox);
                let mut bodies = Vec::with_capacity(sm.bodies.len());
                for b in &sm.bodies {
                    let (mn, mx) = aabb_arrays(&b.bbox);
                    bodies.push(BodyMeta {
                        body: b.body,
                        name: b.name.clone(),
                        origin: b.origin.to_array(),
                        double_sided: b.double_sided,
                        bbox_min: mn,
                        bbox_max: mx,
                        face_ranges: b.face_ranges.clone(),
                        positions: blob.push(&b.positions),
                        normals: blob.push(&b.normals),
                        face_slot: blob.push(&b.face_slot),
                        indices: blob.push(&b.indices),
                        edge_indices: blob.push(&b.edge_indices),
                    });
                }
                shapes.push(ShapeMeta { src_rep, present: true, bbox_min: bmin, bbox_max: bmax, stats: sm.stats, diags: sm.diags.clone(), bodies });
            }
        }
    }
    let meta = Meta { file_hash: *hash, params: *params, tess_version: TESS_VERSION, source_name: source_name.to_string(), shapes };
    let json = serde_json::to_vec(&meta)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        f.write_all(MAGIC)?;
        f.write_all(&FORMAT_VERSION.to_le_bytes())?;
        f.write_all(&(json.len() as u32).to_le_bytes())?;
        f.write_all(&json)?;
        let header_len = 12 + json.len();
        let pad = (16 - header_len % 16) % 16;
        f.write_all(&[0u8; 16][..pad])?;
        f.write_all(&blob.0)?;
        f.flush()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub struct Cached {
    pub meshes: Vec<Option<ShapeMesh>>,
    pub src_reps: Vec<u32>,
    pub params: TessParams,
    pub bytes: u64,
}

/// Load a cache file if present and valid.
pub fn lookup(dir: &Path, hash: &[u8; 32], params: &TessParams) -> Result<Option<Cached>> {
    let path = cache_path(dir, hash, params);
    if !path.exists() {
        return Ok(None);
    }
    let file = std::fs::File::open(&path)?;
    // SAFETY: read-only mapping of a file we own; content is validated before use.
    let map = unsafe { memmap2::Mmap::map(&file)? };
    let b: &[u8] = &map;
    if b.len() < 12 || &b[..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    let version = u32::from_le_bytes(b[4..8].try_into().unwrap());
    if version != FORMAT_VERSION {
        return Err(Error::Version);
    }
    let json_len = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let json = b.get(12..12 + json_len).ok_or(Error::Truncated)?;
    let meta: Meta = serde_json::from_slice(json)?;
    if meta.tess_version != TESS_VERSION || meta.file_hash != *hash {
        return Ok(None);
    }
    let header_len = 12 + json_len;
    let blob_start = header_len + (16 - header_len % 16) % 16;
    let blob = b.get(blob_start..).ok_or(Error::Truncated)?;

    fn read<T: bytemuck::Pod>(blob: &[u8], r: &ArrayRef) -> Result<Vec<T>> {
        let size = std::mem::size_of::<T>();
        let start = r.off as usize;
        let end = start + r.len as usize * size;
        let bytes = blob.get(start..end).ok_or(Error::Truncated)?;
        // Copy into an aligned Vec (the mmap offset may not be aligned for T).
        let mut out: Vec<T> = vec![T::zeroed(); r.len as usize];
        bytemuck::cast_slice_mut::<T, u8>(&mut out).copy_from_slice(bytes);
        Ok(out)
    }

    let mut meshes = Vec::with_capacity(meta.shapes.len());
    let mut src_reps = Vec::with_capacity(meta.shapes.len());
    for s in &meta.shapes {
        src_reps.push(s.src_rep);
        if !s.present {
            meshes.push(None);
            continue;
        }
        let mut bodies = Vec::with_capacity(s.bodies.len());
        for bm in &s.bodies {
            bodies.push(BodyMesh {
                body: bm.body,
                name: bm.name.clone(),
                origin: glam::DVec3::from_array(bm.origin),
                positions: read(blob, &bm.positions)?,
                normals: read(blob, &bm.normals)?,
                face_slot: read(blob, &bm.face_slot)?,
                indices: read(blob, &bm.indices)?,
                face_ranges: bm.face_ranges.clone(),
                edge_indices: read(blob, &bm.edge_indices)?,
                double_sided: bm.double_sided,
                bbox: Aabb { min: glam::DVec3::from_array(bm.bbox_min), max: glam::DVec3::from_array(bm.bbox_max) },
            });
        }
        meshes.push(Some(ShapeMesh {
            bodies,
            bbox: Aabb { min: glam::DVec3::from_array(s.bbox_min), max: glam::DVec3::from_array(s.bbox_max) },
            stats: s.stats,
            diags: s.diags.clone(),
        }));
    }
    Ok(Some(Cached { meshes, src_reps, params: meta.params, bytes: b.len() as u64 }))
}

#[derive(Debug, Default, Clone)]
pub struct CacheInfo {
    pub files: u32,
    pub bytes: u64,
    pub dir: PathBuf,
}

pub fn info(dir: &Path) -> CacheInfo {
    let mut ci = CacheInfo { dir: dir.to_path_buf(), ..Default::default() };
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "stpc") {
                ci.files += 1;
                ci.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    ci
}

pub fn clear(dir: &Path) -> Result<u32> {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "stpc") {
                std::fs::remove_file(e.path())?;
                n += 1;
            }
        }
    }
    Ok(n)
}

/// Remove least-recently-modified files until the directory is under `max_bytes`.
pub fn prune(dir: &Path, max_bytes: u64) -> Result<u32> {
    let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "stpc")
                && let Ok(m) = e.metadata() {
                    entries.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), p));
                }
        }
    }
    entries.sort_by_key(|e| e.0);
    let mut total: u64 = entries.iter().map(|e| e.1).sum();
    let mut removed = 0;
    for (_, len, p) in entries {
        if total <= max_bytes {
            break;
        }
        std::fs::remove_file(&p)?;
        total -= len;
        removed += 1;
    }
    Ok(removed)
}

#[allow(dead_code)]
fn _range_type_check(r: Range<u32>) -> u32 {
    r.end
}

#[cfg(test)]
mod tests {
    use super::*;
    use step_mesh::mesh::SurfaceKind;
    use step_mesh::topo::FaceId;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("stepview-cache-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let body = BodyMesh {
            body: BodyId(0),
            name: "b".into(),
            origin: glam::DVec3::new(1.0, 2.0, 3.0),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            face_slot: vec![0, 0, 0],
            indices: vec![0, 1, 2],
            face_ranges: vec![FaceRange { face: FaceId(7), src: 42, indices: 0..3, color: [1, 2, 3, 4], surface_kind: SurfaceKind::Plane }],
            edge_indices: vec![0, 1, 1, 2, 2, 0],
            double_sided: false,
            bbox: Aabb { min: glam::DVec3::ZERO, max: glam::DVec3::ONE },
        };
        let mesh = ShapeMesh { bodies: vec![body], bbox: Aabb { min: glam::DVec3::ZERO, max: glam::DVec3::ONE }, stats: TessStats { faces: 1, ..Default::default() }, diags: vec![] };
        let hash = [7u8; 32];
        let params = TessParams::PREVIEW;
        store(&dir, &hash, &params, "test", &[Some(mesh.clone()), None], &[100, 101]).unwrap();
        let got = lookup(&dir, &hash, &params).unwrap().unwrap();
        assert_eq!(got.src_reps, vec![100, 101]);
        assert!(got.meshes[1].is_none());
        let m = got.meshes[0].as_ref().unwrap();
        assert_eq!(m.bodies[0].positions, mesh.bodies[0].positions);
        assert_eq!(m.bodies[0].indices, mesh.bodies[0].indices);
        assert_eq!(m.bodies[0].edge_indices, mesh.bodies[0].edge_indices);
        assert_eq!(m.bodies[0].face_ranges, mesh.bodies[0].face_ranges);
        assert_eq!(m.bodies[0].origin, mesh.bodies[0].origin);
        assert_eq!(m.stats.faces, 1);
        // different params → miss
        assert!(lookup(&dir, &hash, &TessParams::FINE).unwrap().is_none());
        assert_eq!(info(&dir).files, 1);
        assert_eq!(clear(&dir).unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
