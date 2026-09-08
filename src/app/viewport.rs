//! The 3D viewport: an offscreen wgpu texture rendered by `step-render` and shown through egui as a
//! native texture. Handles camera input and picking.

use std::sync::Arc;

use egui::mutex::RwLock;

use egui::{Pos2, Rect, Sense, Vec2};
use glam::{DAffine3, DVec2, DVec3};
use step_mesh::{Aabb, ShapeMesh};
use step_render::{Camera, InstanceHandle, PickResult, RenderSettings, Renderer, Scene, ShapeHandle, TargetViews};

use super::App;

pub struct Viewport {
    device: wgpu::Device,
    queue: wgpu::Queue,
    egui_renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    renderer: Renderer,
    scene: Scene,
    texture: Option<(wgpu::Texture, wgpu::TextureView, egui::TextureId, (u32, u32))>,
    /// Screen rect (points) of the last drawn image, for picking/overlays.
    pub last_rect: Rect,
    pub last_ppp: f32,
}

const VIEW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Viewport {
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let renderer = Renderer::new(&rs.device, &rs.queue, VIEW_FORMAT);
        let scene = renderer.new_scene(&rs.device);
        Viewport {
            device: rs.device.clone(),
            queue: rs.queue.clone(),
            egui_renderer: rs.renderer.clone(),
            renderer,
            scene,
            texture: None,
            last_rect: Rect::NOTHING,
            last_ppp: 1.0,
        }
    }

    pub fn clear_scene(&mut self) {
        self.scene = self.renderer.new_scene(&self.device);
    }

    pub fn upload(&mut self, mesh: &ShapeMesh) -> ShapeHandle {
        self.scene.upload_shape(&self.device, &self.queue, mesh)
    }
    pub fn add_instance(&mut self, shape: ShapeHandle, world: DAffine3, node_id: u32) -> InstanceHandle {
        self.scene.add_instance(shape, world, node_id)
    }
    pub fn set_visible(&mut self, inst: InstanceHandle, visible: bool) {
        self.scene.set_visible(inst, visible);
    }
    pub fn set_selected_instances(&mut self, sel: &[InstanceHandle]) {
        self.scene.set_selected_instances(sel);
    }
    pub fn set_selected_faces(&mut self, inst: InstanceHandle, faces: &[u32]) {
        self.scene.set_selected_faces(inst, faces);
    }
    pub fn scene_bbox(&self) -> Aabb {
        self.scene.bbox()
    }

    fn ensure_texture(&mut self, size: (u32, u32)) -> egui::TextureId {
        if let Some((_, _, id, s)) = &self.texture
            && *s == size {
                return *id;
            }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport color"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIEW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut er = self.egui_renderer.write();
        let id = match self.texture.take() {
            Some((_, _, id, _)) => {
                er.update_egui_texture_from_wgpu_texture(&self.device, &view, wgpu::FilterMode::Linear, id);
                id
            }
            None => er.register_native_texture(&self.device, &view, wgpu::FilterMode::Linear),
        };
        drop(er);
        self.texture = Some((tex, view, id, size));
        id
    }

    /// Returns the CPU time spent encoding + submitting the frame, in ms.
    fn render(&mut self, camera: &Camera, settings: &RenderSettings, size: (u32, u32)) -> f32 {
        let Some((_, view, _, _)) = &self.texture else { return 0.0 };
        let t = std::time::Instant::now();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("viewport") });
        self.renderer.render(&self.device, &self.queue, &mut encoder, &mut self.scene, camera, settings, &TargetViews { color: view }, size);
        self.queue.submit([encoder.finish()]);
        t.elapsed().as_secs_f32() * 1000.0
    }

    pub fn pick(&self, x: u32, y: u32) -> Option<PickResult> {
        self.renderer.pick(&self.device, &self.queue, &self.scene, x, y)
    }

    pub fn screenshot(&mut self, camera: &Camera, settings: &RenderSettings, scale: u32) -> Result<image::RgbaImage, step_render::RenderError> {
        let size = self.texture.as_ref().map(|t| t.3).unwrap_or((1600, 1200));
        self.renderer.render_offscreen(&self.device, &self.queue, &mut self.scene, camera, settings, size.0 * scale, size.1 * scale)
    }

    /// Unproject a pixel (physical, relative to the image) with a depth value to world space.
    pub fn unproject(&self, camera: &Camera, px: f64, py: f64, depth: f64, size: (u32, u32)) -> DVec3 {
        let ndc = DVec3::new(px / size.0 as f64 * 2.0 - 1.0, 1.0 - py / size.1 as f64 * 2.0, depth);
        let aspect = size.0 as f64 / size.1.max(1) as f64;
        let inv = camera.view_proj(aspect).inverse();
        
        inv.project_point3(ndc)
    }

    /// Project a world point to screen points (egui coordinates) if in front of the camera.
    pub fn project_to_screen(&self, camera: &Camera, p: DVec3) -> Option<Pos2> {
        let size = self.texture.as_ref()?.3;
        let aspect = size.0 as f64 / size.1.max(1) as f64;
        let ndc = camera.project(p, aspect);
        if !(-1.5..=1.5).contains(&ndc.x) || !(-1.5..=1.5).contains(&ndc.y) || ndc.z < 0.0 || ndc.z > 1.0 {
            return None;
        }
        let r = self.last_rect;
        Some(Pos2::new(r.min.x + (ndc.x as f32 + 1.0) * 0.5 * r.width(), r.min.y + (1.0 - ndc.y as f32) * 0.5 * r.height()))
    }
}

/// Draw the viewport into the central panel and process its input.
pub fn show(app: &mut App, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
    let avail = ui.available_rect_before_wrap();
    let ppp = ui.ctx().pixels_per_point();
    let size_px = ((avail.width() * ppp).round().max(1.0) as u32, (avail.height() * ppp).round().max(1.0) as u32);

    let Some(vp) = &mut app.viewport else {
        ui.centered_and_justified(|ui| ui.label("No GPU available"));
        return;
    };
    let tex_id = vp.ensure_texture(size_px);
    vp.last_ppp = ppp;
    let response = ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(tex_id, avail.size())).sense(Sense::click_and_drag()));
    vp.last_rect = response.rect;
    let rect = response.rect;
    let aspect = size_px.0 as f64 / size_px.1.max(1) as f64;

    // --- camera input ---
    let (zoom_delta, scroll, rotate, modifiers, hover) = ui.input(|i| {
        let mut rot = 0.0f32;
        for e in &i.events {
            if let egui::Event::Rotate(r) = e {
                rot += r;
            }
        }
        (i.zoom_delta(), i.smooth_scroll_delta, rot, i.modifiers, i.pointer.hover_pos())
    });
    let hovered = response.hovered() || response.dragged();
    let cursor_ndc = |pos: Pos2| -> Option<glam::Vec2> {
        if !rect.contains(pos) {
            return None;
        }
        Some(glam::Vec2::new((pos.x - rect.min.x) / rect.width() * 2.0 - 1.0, 1.0 - (pos.y - rect.min.y) / rect.height() * 2.0))
    };
    if hovered {
        if (zoom_delta - 1.0).abs() > 1e-4 {
            app.camera.zoom(zoom_delta as f64, hover.and_then(cursor_ndc), aspect);
            app.camera_touched = true;
        }
        if scroll != Vec2::ZERO {
            if modifiers.shift {
                app.camera.pan(-scroll.x as f64 * ppp as f64, -scroll.y as f64 * ppp as f64, size_px);
            } else {
                let factor = (scroll.y as f64 * 0.0035).exp();
                app.camera.zoom(factor, hover.and_then(cursor_ndc), aspect);
            }
            app.camera_touched = true;
        }
        if rotate.abs() > 1e-5 {
            app.camera.roll(rotate as f64);
            app.camera_touched = true;
        }
    }
    let drag = response.drag_motion();
    if response.dragged_by(egui::PointerButton::Primary) && drag != Vec2::ZERO {
        if modifiers.shift || modifiers.alt {
            app.camera.pan(drag.x as f64 * ppp as f64, drag.y as f64 * ppp as f64, size_px);
        } else {
            let s = 0.008;
            app.camera.orbit(-drag.x as f64 * s, -drag.y as f64 * s);
        }
        app.camera_touched = true;
    }
    if (response.dragged_by(egui::PointerButton::Middle) || response.dragged_by(egui::PointerButton::Secondary)) && drag != Vec2::ZERO {
        app.camera.pan(drag.x as f64 * ppp as f64, drag.y as f64 * ppp as f64, size_px);
        app.camera_touched = true;
    }

    // --- picking ---
    if response.clicked_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos() {
            let px = ((pos.x - rect.min.x) * ppp).round().max(0.0) as u32;
            let py = ((pos.y - rect.min.y) * ppp).round().max(0.0) as u32;
            let pick = vp.pick(px.min(size_px.0.saturating_sub(1)), py.min(size_px.1.saturating_sub(1)));
            match pick {
                Some(p) => {
                    let point = vp.unproject(&app.camera, px as f64, py as f64, p.depth as f64, size_px);
                    if let Some(m) = &app.model {
                        let sel = m.selection_from_pick(&p, point);
                        if app.tools.measure_active {
                            app.tools.add_measure_point(point);
                        } else {
                            m.apply_selection(sel.as_ref(), vp);
                            app.selection = sel;
                        }
                    }
                }
                None => {
                    if !app.tools.measure_active {
                        if let Some(m) = &app.model {
                            m.apply_selection(None, vp);
                        }
                        app.selection = None;
                    }
                }
            }
        }
    if response.double_clicked()
        && let Some(sel) = &app.selection
            && let Some(m) = &app.model {
                let bb = m.node_bbox[sel.node.0 as usize];
                if !bb.is_empty() {
                    app.camera.fit(bb);
                    app.camera_touched = true;
                }
            }

    // --- render ---
    if let Some(m) = &app.model {
        let bb = vp.scene_bbox();
        if !bb.is_empty() {
            app.camera.auto_clip(bb);
        }
        let _ = m;
    }
    let render_ms = vp.render(&app.camera, &app.settings, size_px);
    if app.frame_times.len() >= 60 {
        app.frame_times.pop_front();
    }
    app.frame_times.push_back(render_ms);
    // keep animating while the pointer interacts so the fps figure is meaningful
    if response.dragged() || hovered && (zoom_delta - 1.0).abs() > 1e-4 {
        ui.ctx().request_repaint();
    }

    // --- overlays ---
    super::tools::draw_overlays(app, ui, rect);

    // hint when empty
    if app.model.is_none() && !app.load.loading {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "Drop a .step / .stp file here", egui::FontId::proportional(18.0), egui::Color32::from_gray(180));
    }
    let _ = DVec2::ZERO;
}
