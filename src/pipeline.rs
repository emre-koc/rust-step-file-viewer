//! Shared load pipeline used by the CLI commands and the GUI loader:
//! index → structure → (cache | extract + tessellate) → meshes per shape.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use rayon::prelude::*;
use step_brep::{DiagSink, Diagnostics, ShapeId, Structure};
use step_mesh::{ShapeMesh, TessParams};
use step_p21::StepFile;

#[derive(Clone, Debug)]
pub struct LoadOpts {
    pub params: TessParams,
    pub use_cache: bool,
    pub colors: bool,
}

impl Default for LoadOpts {
    fn default() -> Self {
        LoadOpts { params: TessParams::PREVIEW, use_cache: true, colors: true }
    }
}

impl LoadOpts {
    pub fn from_cli(tol: Option<f64>, no_cache: bool, no_colors: bool) -> Self {
        let mut params = TessParams::PREVIEW;
        if let Some(t) = tol {
            params.chord = t;
        }
        LoadOpts { params, use_cache: !no_cache, colors: !no_colors }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub index_ms: f32,
    pub hash_ms: f32,
    pub structure_ms: f32,
    pub mesh_ms: f32,
    pub cache_write_ms: f32,
    pub total_ms: f32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheOutcome {
    Disabled,
    Miss,
    Hit,
    Written(PathBuf),
    Error(String),
}

#[allow(dead_code)]
pub struct Loaded {
    pub path: PathBuf,
    pub file: Arc<StepFile>,
    pub structure: Arc<Structure>,
    pub meshes: Vec<Option<ShapeMesh>>,
    pub diags: Diagnostics,
    pub timings: Timings,
    pub cache: CacheOutcome,
}

pub fn index(path: &Path) -> anyhow::Result<StepFile> {
    StepFile::open(path).with_context(|| format!("opening {}", path.display()))
}

/// Shape processing order: most instances × most bodies first (best work-stealing balance and the
/// most visible geometry earliest).
pub fn shape_order(structure: &Structure) -> Vec<usize> {
    let asm = &structure.assembly;
    let mut order: Vec<usize> = (0..asm.shapes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(asm.by_shape[i].len() as u64 * (1 + asm.shapes[i].body_items as u64)));
    order
}

/// Extract + tessellate every shape in parallel. `on_shape` is called from worker threads as each
/// shape finishes (in completion order). Returns meshes indexed by shape id; cancelled or failed
/// shapes are `None`.
pub fn tessellate_shapes(
    file: &StepFile,
    structure: &Structure,
    params: &TessParams,
    cancel: &AtomicBool,
    sink: &DiagSink,
    on_shape: &(dyn Fn(ShapeId, &ShapeMesh, usize, usize) + Sync),
) -> Vec<Option<ShapeMesh>> {
    let n = structure.assembly.shapes.len();
    let order = shape_order(structure);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<Option<ShapeMesh>>> = Mutex::new((0..n).map(|_| None).collect());
    order.par_iter().for_each(|&i| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let (topo, diags) = structure.extract(file, ShapeId(i as u32));
        sink.merge(diags);
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mesh = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step_mesh::tessellate(&topo, params)));
        let mesh = match mesh {
            Ok(m) => m,
            Err(_) => {
                sink.push(step_brep::DiagKind::DegenerateGeometry, Some(step_p21::EntityId(topo.src_rep)), "tessellator panicked; shape skipped");
                return;
            }
        };
        let d = done.fetch_add(1, Ordering::Relaxed) + 1;
        on_shape(ShapeId(i as u32), &mesh, d, n);
        results.lock().unwrap()[i] = Some(mesh);
    });
    results.into_inner().unwrap()
}

/// Full synchronous load (CLI).
pub fn load_all(path: &Path, opts: &LoadOpts) -> anyhow::Result<Loaded> {
    let t_start = Instant::now();
    let mut timings = Timings::default();
    let file = Arc::new(index(path)?);
    timings.index_ms = file.index_ms();

    let t = Instant::now();
    let structure = Arc::new(step_brep::load_structure(&file, opts.colors));
    timings.structure_ms = t.elapsed().as_secs_f32() * 1000.0;
    let mut diags = structure.diags.clone();

    let cache_dir = if opts.use_cache { step_cache::default_dir() } else { None };
    let mut cache = if opts.use_cache { CacheOutcome::Miss } else { CacheOutcome::Disabled };
    let mut hash: Option<[u8; 32]> = None;
    let mut meshes: Option<Vec<Option<ShapeMesh>>> = None;
    if let Some(dir) = &cache_dir {
        let t = Instant::now();
        let h = step_cache::file_hash(file.bytes());
        timings.hash_ms = t.elapsed().as_secs_f32() * 1000.0;
        hash = Some(h);
        match step_cache::lookup(dir, &h, &opts.params) {
            Ok(Some(c)) if c.src_reps.len() == structure.assembly.shapes.len() && c.src_reps.iter().zip(&structure.assembly.shapes).all(|(r, s)| *r == s.rep.0) => {
                meshes = Some(c.meshes);
                cache = CacheOutcome::Hit;
            }
            Ok(_) => {}
            Err(e) => cache = CacheOutcome::Error(e.to_string()),
        }
    }

    let meshes = match meshes {
        Some(m) => m,
        None => {
            let t = Instant::now();
            let sink = DiagSink::default();
            let cancel = AtomicBool::new(false);
            let m = tessellate_shapes(&file, &structure, &opts.params, &cancel, &sink, &|_, _, _, _| {});
            timings.mesh_ms = t.elapsed().as_secs_f32() * 1000.0;
            diags.merge(sink.into_inner());
            if let (Some(dir), Some(h)) = (&cache_dir, hash) {
                let t = Instant::now();
                let src: Vec<u32> = structure.assembly.shapes.iter().map(|s| s.rep.0).collect();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                match step_cache::store(dir, &h, &opts.params, &name, &m, &src) {
                    Ok(p) => cache = CacheOutcome::Written(p),
                    Err(e) => cache = CacheOutcome::Error(e.to_string()),
                }
                timings.cache_write_ms = t.elapsed().as_secs_f32() * 1000.0;
            }
            m
        }
    };
    timings.total_ms = t_start.elapsed().as_secs_f32() * 1000.0;
    Ok(Loaded { path: path.to_path_buf(), file, structure, meshes, diags, timings, cache })
}

/// Total triangle count over all instances (what the GPU will draw).
pub fn instanced_triangles(structure: &Structure, meshes: &[Option<ShapeMesh>]) -> u64 {
    structure
        .assembly
        .instances
        .iter()
        .map(|i| meshes.get(i.shape.0 as usize).and_then(|m| m.as_ref()).map_or(0, |m| m.triangle_count() as u64))
        .sum()
}
