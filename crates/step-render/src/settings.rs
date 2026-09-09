//! Per-frame render settings: draw mode, background, MSAA, clip plane, selection highlight.

use glam::DVec3;
use serde::{Deserialize, Serialize};

/// How geometry is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RenderMode {
    /// Shaded triangles only.
    Shaded,
    /// Shaded triangles plus feature edges (the CAD default).
    #[default]
    ShadedEdges,
    /// Feature edges only, no triangles (the shaded pass is skipped entirely).
    Wireframe,
    /// Semi-transparent triangles without depth write, plus edges.
    XRay,
}

impl RenderMode {
    /// Whether the shaded triangle pass runs.
    pub fn draws_triangles(self) -> bool {
        !matches!(self, RenderMode::Wireframe)
    }
    /// Whether the edge pass runs.
    pub fn draws_edges(self) -> bool {
        matches!(self, RenderMode::ShadedEdges | RenderMode::Wireframe | RenderMode::XRay)
    }
}

/// A half-space clip. Fragments with `dot(normal, p_world) > offset` are discarded.
///
/// Solids are drawn without back-face culling while a clip plane is active, and back-facing
/// fragments are shaded in `cap_color` — a cheap stand-in for a real capped section.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipPlane {
    /// Plane normal in world space (mm). Need not be normalised.
    pub normal: DVec3,
    /// Plane offset: the plane is `dot(normal.normalize(), p) == offset`.
    pub offset: f64,
    /// sRGB colour used for the faked cap (back faces of clipped solids).
    pub cap_color: [u8; 4],
}

impl ClipPlane {
    /// A plane through `point` with the given normal.
    pub fn through(point: DVec3, normal: DVec3, cap_color: [u8; 4]) -> Self {
        let n = normal.normalize_or_zero();
        ClipPlane { normal: n, offset: n.dot(point), cap_color }
    }
}

/// Everything the renderer needs beyond scene + camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderSettings {
    pub mode: RenderMode,
    /// Background clear colour, **linear** RGBA (not sRGB-encoded).
    pub background: [f32; 4],
    /// sRGB colour of feature edges.
    pub edge_color: [u8; 4],
    /// Multisample count: 1 (off) or 4. Anything else is clamped to those.
    pub msaa: u32,
    pub clip_plane: Option<ClipPlane>,
    /// Tint selected faces / instances in the shaded and edge passes.
    pub show_selection_highlight: bool,
    /// sRGB highlight colour; alpha is the blend weight against the face colour.
    pub selection_color: [u8; 4],
    /// Alpha used by [`RenderMode::XRay`].
    pub xray_alpha: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings {
            mode: RenderMode::ShadedEdges,
            background: [0.035601314, 0.035601314, 0.035601314, 1.0],
            edge_color: [38, 38, 38, 190],
            msaa: 4,
            clip_plane: None,
            show_selection_highlight: true,
            selection_color: [255, 150, 40, 200],
            xray_alpha: 0.35,
        }
    }
}

impl RenderSettings {
    /// MSAA count clamped to a value the renderer actually builds pipelines for.
    pub fn sample_count(&self) -> u32 {
        if self.msaa >= 4 { 4 } else { 1 }
    }
}
