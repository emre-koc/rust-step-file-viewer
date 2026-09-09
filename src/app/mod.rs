//! The eframe application: state, message handling and layout. Panels live in `panels.rs`, the
//! 3D viewport in `viewport.rs`, model bookkeeping in `model.rs`, measure/section in `tools.rs`.

mod view_cube;
mod navigation;
mod orientation;
mod model;
mod panels;
mod tools;
mod viewport;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use step_brep::Diagnostics;
use step_render::{Camera, RenderMode, RenderSettings, StandardView, UpAxis};

use crate::loader::{LoadMsg, Loader};
use crate::pipeline::{CacheOutcome, LoadOpts, Timings};
pub use model::{LoadedModel, Selection};
use viewport::Viewport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    Coarse,
    Preview,
    Fine,
}

impl Quality {
    pub fn params(self) -> step_mesh::TessParams {
        match self {
            Quality::Coarse => step_mesh::TessParams::COARSE,
            Quality::Preview => step_mesh::TessParams::PREVIEW,
            Quality::Fine => step_mesh::TessParams::FINE,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Quality::Coarse => "Coarse",
            Quality::Preview => "Normal",
            Quality::Fine => "Fine",
        }
    }
}

/// Persisted preferences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prefs {
    pub mode: RenderMode,
    pub quality: Quality,
    pub msaa: u32,
    pub background_srgb: [u8; 3],
    pub edge_color: [u8; 4],
    pub show_stats: bool,
    pub show_tree: bool,
    pub show_props: bool,
    pub ortho: bool,
    pub units_inch: bool,
    pub recent: VecDeque<PathBuf>,
    pub use_cache: bool,
    pub colors: bool,
    pub turntable: bool,
}

impl Prefs {
    fn migrate_appearance(&mut self) {
        if self.background_srgb == [58, 62, 70] {
            self.background_srgb = [53, 53, 53];
        }
        if self.edge_color == [26, 28, 32, 255] {
            self.edge_color = [38, 38, 38, 190];
        }
    }
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            mode: RenderMode::ShadedEdges,
            quality: Quality::Preview,
            msaa: 4,
            background_srgb: [53, 53, 53],
            edge_color: [38, 38, 38, 190],
            show_stats: true,
            show_tree: true,
            show_props: true,
            ortho: false,
            units_inch: false,
            recent: VecDeque::new(),
            use_cache: true,
            colors: true,
            turntable: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct LoadState {
    pub generation: u64,
    pub path: Option<PathBuf>,
    pub loading: bool,
    pub done: usize,
    pub total: usize,
    pub entities: u32,
    pub timings: Timings,
    pub cache: Option<CacheOutcome>,
    pub error: Option<String>,
    pub diags: Diagnostics,
    pub started: Option<Instant>,
    pub first_shape_at: Option<f32>,
    pub finished_at: Option<f32>,
}

pub struct App {
    pub prefs: Prefs,
    pub camera: Camera,
    pub up_axis_override: Option<UpAxis>,
    pub view_animation: Option<navigation::ViewAnimation>,
    pub settings: RenderSettings,
    pub loader: Loader,
    pub load: LoadState,
    pub model: Option<LoadedModel>,
    pub viewport: Option<Viewport>,
    pub selection: Option<Selection>,
    pub tools: tools::Tools,
    pub tree_filter: String,
    pub status: String,
    pub frame_times: VecDeque<f32>,
    pub last_frame: Instant,
    pub camera_touched: bool,
    pub pending_fit: bool,
    pub load_opts: LoadOpts,
    pub about_open: bool,
    pub diag_open: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>, opts: LoadOpts) -> Self {
        let mut prefs: Prefs = cc.storage.and_then(|s| eframe::get_value(s, "prefs")).unwrap_or_default();
        prefs.migrate_appearance();
        let ctx = cc.egui_ctx.clone();
        let loader = Loader::new(Arc::new(move || ctx.request_repaint()));
        let viewport = cc.wgpu_render_state.as_ref().map(Viewport::new);
        let camera = Camera { ortho: prefs.ortho, ..Default::default() };
        let mut settings = RenderSettings::default();
        apply_prefs_to_settings(&prefs, &mut settings);
        let mut load_opts = opts.clone();
        load_opts.params = prefs.quality.params();
        if opts_overrides_quality(&opts) {
            load_opts.params = opts.params;
        }
        let mut app = App {
            prefs,
            camera,
            up_axis_override: None,
            view_animation: None,
            settings,
            loader,
            load: LoadState::default(),
            model: None,
            viewport,
            selection: None,
            tools: tools::Tools::default(),
            tree_filter: String::new(),
            status: "Open a STEP file (⌘O) or drop one here".into(),
            frame_times: VecDeque::with_capacity(120),
            last_frame: Instant::now(),
            camera_touched: false,
            pending_fit: false,
            load_opts,
            about_open: false,
            diag_open: false,
        };
        #[cfg(target_os = "macos")]
        crate::macos::set_wake(cc.egui_ctx.clone());
        if let Some(p) = initial {
            app.open_path(p);
        }
        app
    }

    pub fn open_path(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if self.load.path.as_ref() != Some(&path) {
            self.up_axis_override = None;
        }
        self.camera.up_axis = self.up_axis_override.unwrap_or_default();
        self.camera.standard_view(StandardView::Iso);
        self.load = LoadState { started: Some(Instant::now()), loading: true, path: Some(path.clone()), ..Default::default() };
        self.model = None;
        self.selection = None;
        self.tools.reset();
        if let Some(v) = &mut self.viewport {
            v.clear_scene();
        }
        self.view_animation = None;
        self.camera_touched = false;
        self.pending_fit = true;
        let mut opts = self.load_opts.clone();
        opts.use_cache = self.prefs.use_cache;
        opts.colors = self.prefs.colors;
        self.load.generation = self.loader.open(path.clone(), opts);
        self.status = format!("Loading {}…", path.display());
        self.prefs.recent.retain(|p| p != &path);
        self.prefs.recent.push_front(path);
        while self.prefs.recent.len() > 10 {
            self.prefs.recent.pop_back();
        }
    }

    pub fn reload(&mut self) {
        if let Some(p) = self.load.path.clone() {
            self.open_path(p);
        }
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.loader.rx.try_recv() {
            let generation = match &msg {
                LoadMsg::Started { generation, .. }
                | LoadMsg::Indexed { generation, .. }
                | LoadMsg::Structure { generation, .. }
                | LoadMsg::Shape { generation, .. }
                | LoadMsg::Finished { generation, .. }
                | LoadMsg::Failed { generation, .. } => *generation,
            };
            if generation != self.load.generation {
                continue;
            }
            match msg {
                LoadMsg::Started { .. } => {}
                LoadMsg::Indexed { entities, index_ms, .. } => {
                    self.load.entities = entities;
                    self.load.timings.index_ms = index_ms;
                    self.status = format!("Indexed {entities} entities in {index_ms:.0} ms");
                }
                LoadMsg::Structure { file, structure, .. } => {
                    self.camera.up_axis = self.up_axis_override.unwrap_or_else(|| orientation::suggested_up_axis(file.header()));
                    if !self.camera_touched {
                        self.camera.standard_view(StandardView::Iso);
                    } else {
                        self.camera.view_from_direction(-self.camera.forward());
                    }
                    self.load.total = structure.assembly.shapes.len();
                    self.status = format!("{} parts, {} instances — tessellating…", structure.assembly.product_count, structure.assembly.instances.len());
                    self.model = Some(LoadedModel::new(file, structure));
                }
                LoadMsg::Shape { shape, mesh, done, total, .. } => {
                    self.load.done = done;
                    self.load.total = total;
                    if self.load.first_shape_at.is_none() {
                        self.load.first_shape_at = self.load.started.map(|t| t.elapsed().as_secs_f32() * 1000.0);
                    }
                    if let (Some(m), Some(v)) = (&mut self.model, &mut self.viewport) {
                        m.add_shape(shape, mesh, v);
                        if self.pending_fit && !self.camera_touched {
                            self.camera.fit(v.scene_bbox());
                        }
                    }
                }
                LoadMsg::Finished { timings, diags, cache, .. } => {
                    self.load.loading = false;
                    self.load.timings = timings;
                    self.load.cache = Some(cache);
                    self.load.diags = diags;
                    self.load.finished_at = self.load.started.map(|t| t.elapsed().as_secs_f32() * 1000.0);
                    if let (Some(m), Some(v)) = (&mut self.model, &mut self.viewport) {
                        m.finish(v);
                        if !self.camera_touched {
                            self.camera.fit(v.scene_bbox());
                        }
                    }
                    self.pending_fit = false;
                    let tris = self.model.as_ref().map_or(0, |m| m.instanced_triangles());
                    self.status = format!(
                        "Loaded in {:.0} ms — {} triangles{}",
                        self.load.finished_at.unwrap_or(timings.total_ms),
                        group_digits(tris),
                        match &self.load.cache {
                            Some(CacheOutcome::Hit) => " (from cache)",
                            _ => "",
                        }
                    );
                }
                LoadMsg::Failed { error, .. } => {
                    self.load.loading = false;
                    self.load.error = Some(error.clone());
                    self.status = format!("Failed: {error}");
                }
            }
        }
    }

    pub fn fit_all(&mut self) {
        self.direct_camera_input();
        if let Some(v) = &self.viewport {
            self.camera.fit_aspect(v.scene_bbox(), v.last_rect.aspect_ratio() as f64);
        }
    }

    pub fn direct_camera_input(&mut self) {
        self.view_animation = None;
        self.camera_touched = true;
    }

    pub fn set_projection(&mut self, ortho: bool) {
        self.direct_camera_input();
        self.camera.ortho = ortho;
        self.prefs.ortho = ortho;
    }

    pub fn set_up_axis(&mut self, axis: Option<UpAxis>) {
        self.up_axis_override = axis;
        let up = axis.unwrap_or_else(|| self.model.as_ref().map_or(UpAxis::Z, |m| orientation::suggested_up_axis(m.file.header())));
        self.direct_camera_input();
        self.camera.up_axis = up;
        // Keep the same eye direction, target, distance and projection; level the horizon.
        self.camera.view_from_direction(-self.camera.forward());
    }

    pub fn new_window(&mut self, path: Option<PathBuf>) {
        if let Err(error) = crate::gui::open_window(path.as_deref()) {
            self.status = format!("Could not open a new window: {error:#}");
        }
    }

    pub fn open_document(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if self.load.path.is_none() {
            self.open_path(path);
        } else if self.load.path.as_ref() != Some(&path) {
            self.new_window(Some(path));
        }
    }

    pub fn orbit_camera(&mut self, yaw: f64, pitch: f64) {
        self.set_projection(false);
        self.camera.orbit(yaw, pitch);
    }

    pub fn drag_orbit(&mut self, delta: egui::Vec2) {
        // Camera::orbit already applies the camera/model sign reversal.
        self.orbit_camera(delta.x as f64 * 0.008, delta.y as f64 * 0.008);
    }

    pub fn snap_direction(&mut self, direction: glam::DVec3, ortho: bool) {
        self.set_projection(ortho);
        let mut destination = self.camera;
        destination.view_from_direction(direction);
        self.view_animation = Some(navigation::ViewAnimation::new(self.camera.orientation, destination.orientation));
    }

    pub fn set_view(&mut self, view: StandardView) {
        self.snap_direction(-self.camera.standard_view_forward(view), view != StandardView::Iso);
    }

    pub fn export_dialog(&mut self) {
        let Some(m) = &self.model else { return };
        let default_name = self.load.path.as_ref().and_then(|p| p.file_stem()).and_then(|s| s.to_str()).map(|s| format!("{s}.glb")).unwrap_or_else(|| "model.glb".into());
        let Some(out) = rfd::FileDialog::new()
            .add_filter("glTF binary", &["glb"])
            .add_filter("glTF", &["gltf"])
            .add_filter("STL", &["stl"])
            .add_filter("OBJ", &["obj"])
            .set_file_name(default_name)
            .save_file()
        else {
            return;
        };
        let only: Option<Vec<u32>> = self.selection.as_ref().and_then(|s| if self.tools.export_selection_only { Some(m.instances_under_node(s.node)) } else { None });
        let scene = step_export::Scene { assembly: &m.structure.assembly, meshes: &m.meshes, only_instances: only.as_deref() };
        match step_export::export(&out, &scene, step_export::ExportOpts::default()) {
            Ok(st) => self.status = format!("Exported {} ({} triangles, {:.1} MB)", out.display(), group_digits(st.triangles), st.bytes as f64 / 1e6),
            Err(e) => self.status = format!("Export failed: {e}"),
        }
    }

    pub fn screenshot_dialog(&mut self) {
        let Some(v) = &mut self.viewport else { return };
        let default_name = self.load.path.as_ref().and_then(|p| p.file_stem()).and_then(|s| s.to_str()).map(|s| format!("{s}.png")).unwrap_or_else(|| "StepView.png".into());
        let Some(out) = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name(default_name).save_file() else { return };
        match v.screenshot(&self.camera, &self.settings, 2) {
            Ok(img) => match img.save(&out) {
                Ok(()) => self.status = format!("Saved {} ({}x{})", out.display(), img.width(), img.height()),
                Err(e) => self.status = format!("Screenshot failed: {e}"),
            },
            Err(e) => self.status = format!("Screenshot failed: {e}"),
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(paths) = rfd::FileDialog::new().add_filter("STEP", &["step", "stp", "STEP", "STP", "p21"]).pick_files() {
            for p in paths {
                self.open_document(p);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut open = false;
        let mut export = false;
        let mut shot = false;
        let (fit, views, mode, hide, unhide, isolate, reload, esc) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            open = cmd && i.key_pressed(egui::Key::O);
            export = cmd && i.key_pressed(egui::Key::E);
            shot = cmd && i.key_pressed(egui::Key::S);
            let fit = i.key_pressed(egui::Key::F) && !cmd;
            let mut view = None;
            for (k, v) in [
                (egui::Key::Num1, StandardView::Front),
                (egui::Key::Num2, StandardView::Back),
                (egui::Key::Num3, StandardView::Left),
                (egui::Key::Num4, StandardView::Right),
                (egui::Key::Num5, StandardView::Top),
                (egui::Key::Num6, StandardView::Bottom),
                (egui::Key::Num7, StandardView::Iso),
            ] {
                if i.key_pressed(k) && !cmd {
                    view = Some(v);
                }
            }
            let mut mode = None;
            if !cmd {
                if i.key_pressed(egui::Key::W) {
                    mode = Some(RenderMode::Wireframe);
                }
                if i.key_pressed(egui::Key::S) {
                    mode = Some(RenderMode::ShadedEdges);
                }
                if i.key_pressed(egui::Key::X) {
                    mode = Some(RenderMode::XRay);
                }
            }
            let hide = i.key_pressed(egui::Key::H) && !i.modifiers.shift && !cmd;
            let unhide = i.key_pressed(egui::Key::H) && i.modifiers.shift;
            let isolate = i.key_pressed(egui::Key::I) && !cmd;
            let reload = cmd && i.key_pressed(egui::Key::R);
            let esc = i.key_pressed(egui::Key::Escape);
            (fit, view, mode, hide, unhide, isolate, reload, esc)
        });
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::N)) {
            self.new_window(None);
        }
        if open {
            self.open_dialog();
        }
        if export {
            self.export_dialog();
        }
        if shot {
            self.screenshot_dialog();
        }
        if fit {
            self.fit_all();
        }
        if let Some(v) = views {
            self.set_view(v);
        }
        if let Some(m) = mode {
            self.prefs.mode = m;
            self.settings.mode = m;
        }
        if hide {
            self.hide_selected();
        }
        if unhide {
            self.unhide_all();
        }
        if isolate {
            self.isolate_selected();
        }
        if reload {
            self.reload();
        }
        if esc {
            self.selection = None;
            if let (Some(m), Some(v)) = (&mut self.model, &mut self.viewport) {
                m.apply_selection(None, v);
            }
            self.tools.measure_active = false;
        }
    }

    pub fn hide_selected(&mut self) {
        if let (Some(sel), Some(m), Some(v)) = (&self.selection, &mut self.model, &mut self.viewport) {
            m.set_node_hidden(sel.node, true, v);
        }
    }
    pub fn unhide_all(&mut self) {
        if let (Some(m), Some(v)) = (&mut self.model, &mut self.viewport) {
            m.unhide_all(v);
        }
    }
    pub fn isolate_selected(&mut self) {
        if let (Some(sel), Some(m), Some(v)) = (&self.selection, &mut self.model, &mut self.viewport) {
            m.isolate_node(sel.node, v);
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for p in dropped.into_iter().filter(|p| is_step(p)) {
            self.open_document(p);
        }
    }
}

fn is_step(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "step" | "stp" | "p21"))
}

fn opts_overrides_quality(opts: &LoadOpts) -> bool {
    opts.params.chord > 0.0
}

pub fn apply_prefs_to_settings(p: &Prefs, s: &mut RenderSettings) {
    s.mode = p.mode;
    s.msaa = p.msaa;
    s.edge_color = p.edge_color;
    let lin = |c: u8| crate::commands::render::srgb_to_linear(c as f32 / 255.0);
    s.background = [lin(p.background_srgb[0]), lin(p.background_srgb[1]), lin(p.background_srgb[2]), 1.0];
}

pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.prefs.ortho = self.camera.ortho;
        eframe::set_value(storage, "prefs", &self.prefs);
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.last_frame = Instant::now();
        self.drain_messages();
        #[cfg(target_os = "macos")]
        for p in crate::macos::take_pending().into_iter().filter(|p| is_step(p)) {
            self.open_document(p);
        }
        self.handle_drops(ctx);
        self.handle_shortcuts(ctx);
        if self.load.loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let title = self.load.path.as_ref().and_then(|p| p.file_name()).map_or_else(|| "StepView".to_owned(), |name| format!("{} — StepView", name.to_string_lossy()));
        if ui.input(|i| i.viewport().title.as_deref() != Some(title.as_str())) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        panels::top_bar(self, ui);
        panels::status_bar(self, ui);
        if self.prefs.show_tree {
            panels::tree_panel(self, ui);
        }
        if self.prefs.show_props {
            panels::props_panel(self, ui);
        }
        panels::dialogs(self, &ui.ctx().clone());

        egui::CentralPanel::no_frame().show(ui, |ui| {
            viewport::show(self, ui, frame);
        });
    }
}
