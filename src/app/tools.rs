//! Measurement and section-plane tools.

use egui::{Color32, Pos2, Rect, Stroke};
use glam::DVec3;
use step_render::ClipPlane;

use super::App;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionAxis {
    X,
    Y,
    Z,
    Face,
}

#[derive(Debug)]
pub struct Tools {
    pub measure_active: bool,
    pub measure_points: Vec<DVec3>,
    pub section_enabled: bool,
    pub section_axis: SectionAxis,
    /// 0..1 along the bbox extent.
    pub section_t: f32,
    pub section_flip: bool,
    pub section_normal: DVec3,
    pub export_selection_only: bool,
}

impl Default for Tools {
    fn default() -> Self {
        Tools {
            measure_active: false,
            measure_points: Vec::new(),
            section_enabled: false,
            section_axis: SectionAxis::Z,
            section_t: 0.5,
            section_flip: false,
            section_normal: DVec3::Z,
            export_selection_only: false,
        }
    }
}

impl Tools {
    pub fn reset(&mut self) {
        self.measure_points.clear();
        self.section_enabled = false;
    }

    pub fn add_measure_point(&mut self, p: DVec3) {
        if self.measure_points.len() >= 2 {
            self.measure_points.clear();
        }
        self.measure_points.push(p);
    }

    pub fn distance(&self) -> Option<f64> {
        if self.measure_points.len() == 2 { Some(self.measure_points[0].distance(self.measure_points[1])) } else { None }
    }

    /// Recompute the clip plane from the current settings and scene bbox.
    pub fn clip_plane(&self, bbox: &step_mesh::Aabb) -> Option<ClipPlane> {
        if !self.section_enabled || bbox.is_empty() {
            return None;
        }
        let n = match self.section_axis {
            SectionAxis::X => DVec3::X,
            SectionAxis::Y => DVec3::Y,
            SectionAxis::Z => DVec3::Z,
            SectionAxis::Face => self.section_normal.normalize_or_zero(),
        };
        let n = if self.section_flip { -n } else { n };
        // sweep between the bbox extremes along n
        let c = bbox.center();
        let half = bbox.size() * 0.5;
        let extent = half.x * n.x.abs() + half.y * n.y.abs() + half.z * n.z.abs();
        let point = c + n * ((self.section_t as f64 * 2.0 - 1.0) * extent);
        Some(ClipPlane::through(point, n, [235, 120, 60, 255]))
    }
}

pub fn draw_overlays(app: &App, ui: &egui::Ui, rect: Rect) {
    let Some(vp) = &app.viewport else { return };
    let painter = ui.painter_at(rect);
    // measurement points/line
    let pts: Vec<Option<Pos2>> = app.tools.measure_points.iter().map(|p| vp.project_to_screen(&app.camera, *p)).collect();
    for p in pts.iter().flatten() {
        painter.circle_stroke(*p, 6.0, Stroke::new(2.0, Color32::from_rgb(255, 200, 60)));
        painter.circle_filled(*p, 2.5, Color32::from_rgb(255, 200, 60));
    }
    if let [Some(a), Some(b)] = pts.as_slice() {
        painter.line_segment([*a, *b], Stroke::new(2.0, Color32::from_rgb(255, 200, 60)));
        if let Some(d) = app.tools.distance() {
            let mid = Pos2::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            let text = format_len(d, app.prefs.units_inch);
            painter.text(mid + egui::vec2(8.0, -8.0), egui::Align2::LEFT_BOTTOM, text, egui::FontId::proportional(14.0), Color32::WHITE);
        }
    }
    // selection point marker
    if let Some(sel) = &app.selection
        && let Some(p) = vp.project_to_screen(&app.camera, sel.point) {
            painter.circle_stroke(p, 4.0, Stroke::new(1.5, Color32::from_rgb(255, 150, 40)));
        }
    if app.tools.measure_active {
        painter.text(rect.left_top() + egui::vec2(10.0, 10.0), egui::Align2::LEFT_TOP, "Measure: click two points (Esc to exit)", egui::FontId::proportional(13.0), Color32::from_rgb(255, 200, 60));
    }
}

pub fn format_len(mm: f64, inch: bool) -> String {
    if inch { format!("{:.4} in", mm / 25.4) } else if mm.abs() >= 1000.0 { format!("{:.3} m", mm / 1000.0) } else { format!("{:.3} mm", mm) }
}
