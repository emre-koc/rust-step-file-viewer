//! Background, cancellable, progressive loader for the GUI. Messages arrive on a channel; each
//! carries the generation of the load it belongs to so stale results are dropped.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use step_brep::{DiagSink, Diagnostics, ShapeId, Structure};
use step_mesh::ShapeMesh;
use step_p21::StepFile;

use crate::pipeline::{CacheOutcome, LoadOpts, Timings};

pub enum LoadMsg {
    #[allow(dead_code)]
    Started { generation: u64, path: PathBuf },
    Indexed { generation: u64, entities: u32, index_ms: f32 },
    Structure { generation: u64, file: Arc<StepFile>, structure: Arc<Structure> },
    Shape { generation: u64, shape: ShapeId, mesh: Arc<ShapeMesh>, done: usize, total: usize },
    Finished { generation: u64, timings: Timings, diags: Diagnostics, cache: CacheOutcome },
    Failed { generation: u64, error: String },
}

pub struct LoadHandle {
    #[allow(dead_code)]
    pub generation: u64,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for LoadHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

pub struct Loader {
    tx: Sender<LoadMsg>,
    pub rx: Receiver<LoadMsg>,
    current: Option<LoadHandle>,
    generation: u64,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Loader {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = channel();
        Loader { tx, rx, current: None, generation: 0, wake }
    }

    #[allow(dead_code)]
    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    pub fn cancel(&mut self) {
        if let Some(h) = self.current.take() {
            h.cancel.store(true, Ordering::Relaxed);
            // do not join here: let it finish its current shape in the background
            std::mem::forget(h);
        }
    }

    /// Start loading `path`; any in-flight load is cancelled.
    pub fn open(&mut self, path: PathBuf, opts: LoadOpts) -> u64 {
        self.cancel();
        self.generation += 1;
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        let c = cancel.clone();
        let join = std::thread::Builder::new()
            .name(format!("stepview-load-{generation}"))
            .spawn(move || run_load(generation, path, opts, c, tx, wake))
            .ok();
        self.current = Some(LoadHandle { generation, cancel, join });
        generation
    }
}

fn run_load(generation: u64, path: PathBuf, opts: LoadOpts, cancel: Arc<AtomicBool>, tx: Sender<LoadMsg>, wake: Arc<dyn Fn() + Send + Sync>) {
    let send = |m: LoadMsg| {
        let _ = tx.send(m);
        wake();
    };
    let t_start = Instant::now();
    let mut timings = Timings::default();
    send(LoadMsg::Started { generation, path: path.clone() });

    let file = match crate::pipeline::index(&path) {
        Ok(f) => Arc::new(f),
        Err(e) => {
            send(LoadMsg::Failed { generation, error: format!("{e:#}") });
            return;
        }
    };
    timings.index_ms = file.index_ms();
    send(LoadMsg::Indexed { generation, entities: file.stats().entities, index_ms: file.index_ms() });
    if cancel.load(Ordering::Relaxed) {
        return;
    }

    let t = Instant::now();
    let structure = Arc::new(step_brep::load_structure(&file, opts.colors));
    timings.structure_ms = t.elapsed().as_secs_f32() * 1000.0;
    let mut diags = structure.diags.clone();
    send(LoadMsg::Structure { generation, file: file.clone(), structure: structure.clone() });
    if cancel.load(Ordering::Relaxed) {
        return;
    }

    let cache_dir = if opts.use_cache { step_cache::default_dir() } else { None };
    let mut cache = if opts.use_cache { CacheOutcome::Miss } else { CacheOutcome::Disabled };
    let mut hash = None;
    let total = structure.assembly.shapes.len();
    let mut served_from_cache = false;
    if let Some(dir) = &cache_dir {
        let t = Instant::now();
        let h = step_cache::file_hash(file.bytes());
        timings.hash_ms = t.elapsed().as_secs_f32() * 1000.0;
        hash = Some(h);
        match step_cache::lookup(dir, &h, &opts.params) {
            Ok(Some(c)) if c.src_reps.len() == total && c.src_reps.iter().zip(&structure.assembly.shapes).all(|(r, s)| *r == s.rep.0) => {
                let order = crate::pipeline::shape_order(&structure);
                let mut done = 0;
                let mut meshes = c.meshes;
                for i in order {
                    if let Some(m) = meshes[i].take() {
                        done += 1;
                        send(LoadMsg::Shape { generation, shape: ShapeId(i as u32), mesh: Arc::new(m), done, total });
                    }
                }
                cache = CacheOutcome::Hit;
                served_from_cache = true;
            }
            Ok(_) => {}
            Err(e) => cache = CacheOutcome::Error(e.to_string()),
        }
    }

    if !served_from_cache {
        let t = Instant::now();
        let sink = DiagSink::default();
        let collected: Mutex<Vec<Option<ShapeMesh>>> = Mutex::new((0..total).map(|_| None).collect());
        let want_cache = cache_dir.is_some();
        let meshes = crate::pipeline::tessellate_shapes(&file, &structure, &opts.params, &cancel, &sink, &|shape, mesh, done, total| {
            let arc = Arc::new(mesh.clone());
            if want_cache {
                collected.lock().unwrap()[shape.0 as usize] = Some(mesh.clone());
            }
            send(LoadMsg::Shape { generation, shape, mesh: arc, done, total });
        });
        timings.mesh_ms = t.elapsed().as_secs_f32() * 1000.0;
        diags.merge(sink.into_inner());
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if let (Some(dir), Some(h)) = (&cache_dir, hash) {
            let t = Instant::now();
            let src: Vec<u32> = structure.assembly.shapes.iter().map(|s| s.rep.0).collect();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let all = meshes.iter().all(Option::is_some);
            if all {
                match step_cache::store(dir, &h, &opts.params, &name, &meshes, &src) {
                    Ok(p) => cache = CacheOutcome::Written(p),
                    Err(e) => cache = CacheOutcome::Error(e.to_string()),
                }
            }
            timings.cache_write_ms = t.elapsed().as_secs_f32() * 1000.0;
        }
        drop(collected);
    }
    timings.total_ms = t_start.elapsed().as_secs_f32() * 1000.0;
    send(LoadMsg::Finished { generation, timings, diags, cache });
}
