//! Shared load path for both FFI entry points: index → structure → (cache | extract + tessellate).
//!
//! This mirrors `stepview`'s `src/pipeline.rs` but lives here because the binary crate is not a
//! library. Two differences matter for Quick Look:
//!
//! * the mesh cache is **read-only** — a Quick Look extension must never race the GUI's writer, and
//!   its `COARSE` meshes would pollute a key the GUI does not use;
//! * tessellation honours a wall-clock deadline; shapes not started before it are left `None` and
//!   the caller reports a degraded result.
//!
//! Work is spread over `std::thread::scope` workers rather than rayon so the crate keeps the
//! dependency set the FFI actually needs.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use step_brep::{DiagSink, ShapeId, Structure};
use step_mesh::{ShapeMesh, TessParams};
use step_p21::StepFile;

pub struct Loaded {
    pub file: StepFile,
    pub structure: Structure,
    pub meshes: Vec<Option<ShapeMesh>>,
    /// True when every shape produced a mesh (no deadline cut, no cache mismatch).
    pub complete: bool,
    /// True when the meshes came out of the on-disk cache.
    pub from_cache: bool,
}

/// Open and index a STEP file.
pub fn index(path: &Path) -> Result<StepFile, String> {
    StepFile::open(path).map_err(|e| format!("opening {}: {e}", path.display()))
}

/// Look the file up in the GUI's mesh cache. Read-only and best-effort: any error is a miss.
pub fn cache_lookup(file: &StepFile, structure: &Structure, params: &TessParams) -> Option<Vec<Option<ShapeMesh>>> {
    let dir = step_cache::default_dir()?;
    let hash = step_cache::file_hash(file.bytes());
    let cached = step_cache::lookup(&dir, &hash, params).ok()??;
    // Same validation `pipeline.rs` does: the cache must describe exactly this assembly.
    let shapes = &structure.assembly.shapes;
    if cached.src_reps.len() != shapes.len() || !cached.src_reps.iter().zip(shapes).all(|(r, s)| *r == s.rep.0) {
        return None;
    }
    Some(cached.meshes)
}

/// Shape processing order: most instances × most bodies first.
fn shape_order(structure: &Structure) -> Vec<usize> {
    let asm = &structure.assembly;
    let mut order: Vec<usize> = (0..asm.shapes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(asm.by_shape[i].len() as u64 * (1 + asm.shapes[i].body_items as u64)));
    order
}

fn worker_count(jobs: usize) -> usize {
    let hw = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    hw.min(jobs).max(1)
}

/// Extract + tessellate every shape. Returns the meshes and whether all of them were produced.
/// A worker stops pulling new shapes once `deadline` has passed.
pub fn tessellate_all(
    file: &StepFile,
    structure: &Structure,
    params: &TessParams,
    deadline: Option<Instant>,
) -> (Vec<Option<ShapeMesh>>, bool) {
    let n = structure.assembly.shapes.len();
    if n == 0 {
        return (Vec::new(), true);
    }
    let order = shape_order(structure);
    let cursor = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<ShapeMesh>>> = Mutex::new((0..n).map(|_| None).collect());
    let sink = DiagSink::default();

    std::thread::scope(|scope| {
        for _ in 0..worker_count(n) {
            scope.spawn(|| {
                loop {
                    let k = cursor.fetch_add(1, Ordering::Relaxed);
                    if k >= order.len() {
                        return;
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return;
                    }
                    let i = order[k];
                    let (topo, diags) = structure.extract(file, ShapeId(i as u32));
                    sink.merge(diags);
                    // The tessellator documents that it never panics; belt and braces anyway,
                    // because a panic here would unwind across the C ABI.
                    let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step_mesh::tessellate(&topo, params)));
                    if let Ok(m) = mesh {
                        results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(m);
                    }
                }
            });
        }
    });

    let meshes = results.into_inner().unwrap_or_else(|e| e.into_inner());
    let complete = meshes.iter().all(|m| m.is_some());
    (meshes, complete)
}

/// Per-shape bounds taken from the B-rep vertex points only — no tessellation. Used for the
/// degraded thumbnail: extraction is roughly an order of magnitude cheaper than tessellating.
pub fn shape_bounds(file: &StepFile, structure: &Structure, deadline: Option<Instant>) -> Vec<Option<step_mesh::Aabb>> {
    let n = structure.assembly.shapes.len();
    if n == 0 {
        return Vec::new();
    }
    let order = shape_order(structure);
    let cursor = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<step_mesh::Aabb>>> = Mutex::new((0..n).map(|_| None).collect());
    let sink = DiagSink::default();

    std::thread::scope(|scope| {
        for _ in 0..worker_count(n) {
            scope.spawn(|| {
                loop {
                    let k = cursor.fetch_add(1, Ordering::Relaxed);
                    if k >= order.len() {
                        return;
                    }
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return;
                    }
                    let i = order[k];
                    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| structure.extract(file, ShapeId(i as u32))));
                    let Ok((topo, diags)) = extracted else { continue };
                    sink.merge(diags);
                    let mut bb = step_mesh::Aabb::EMPTY;
                    for v in &topo.vertices {
                        bb.extend(v.p);
                    }
                    if !bb.is_empty() {
                        out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(bb);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Full load with an optional deadline for the tessellation stage.
pub fn load(path: &Path, params: &TessParams, deadline: Option<Instant>) -> Result<Loaded, String> {
    let file = index(path)?;
    let structure = step_brep::load_structure(&file, true);
    if let Some(meshes) = cache_lookup(&file, &structure, params) {
        let complete = meshes.iter().any(|m| m.is_some());
        return Ok(Loaded { file, structure, meshes, complete, from_cache: true });
    }
    let (meshes, complete) = tessellate_all(&file, &structure, params, deadline);
    Ok(Loaded { file, structure, meshes, complete, from_cache: false })
}
