//! `sv_thumbnail_png`: a square isometric thumbnail of a STEP file.
//!
//! # Image
//!
//! Square, `size_px` on a side, RGBA8 PNG with a **fully transparent background** (alpha 0 outside
//! the model). Finder and Quick Look composite thumbnails over their own backdrop, which differs
//! between light and dark mode, so a transparent plate is the only choice that looks right in both;
//! it also keeps the icon reading as an object rather than as a tile. The model itself is drawn with
//! the viewer's normal shading and dark feature edges, framed with a small margin, from the standard
//! CAD isometric (`StandardView::Iso`).
//!
//! # Budget
//!
//! `budget_ms` is a soft wall-clock budget checked after indexing and after the product structure.
//! When either check is already over, the geometry stage is skipped and per-shape bounds — taken
//! from the B-rep vertex points, which is far cheaper than tessellating — are drawn as shaded boxes
//! by the CPU rasteriser. Tessellation itself also stops pulling new shapes at the deadline; a
//! partial result is rendered rather than discarded. Anything short of a full render returns 1.

use std::path::Path;
use std::time::{Duration, Instant};

use glam::DAffine3;
use image::ImageEncoder;
use step_brep::Structure;
use step_mesh::{ShapeMesh, TessParams};
use step_render::{Camera, RenderMode, RenderSettings, Renderer, StandardView};

use crate::load;
use crate::model::Model;
use crate::raster;

/// Transparent background, linear RGBA — see the module docs.
const BACKGROUND: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
/// Below this pixel size feature edges only muddy the image.
const EDGES_MIN_PX: u32 = 48;
/// Share of the remaining budget the degraded path may spend collecting B-rep bounds.
const DEGRADED_SLICE: f32 = 0.5;

pub struct Thumbnail {
    pub png: Vec<u8>,
    /// [`crate::SV_OK`] for a full render, [`crate::SV_DEGRADED`] otherwise.
    pub code: i32,
}

/// Failures carry the status code the C API should report alongside the message.
pub type Failure = (i32, String);

fn encode_png(img: &image::RgbaImage) -> Result<Vec<u8>, Failure> {
    let mut buf = Vec::new();
    image::codecs::png::PngEncoder::new(&mut buf)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| (crate::SV_ERR_RENDER, format!("encoding PNG: {e}")))?;
    Ok(buf)
}

/// Headless Metal render of the tessellated model. Mirrors `stepview render`.
fn gpu_render(structure: &Structure, meshes: &[Option<ShapeMesh>], size: u32) -> Result<image::RgbaImage, String> {
    let run = || -> Result<image::RgbaImage, String> {
        let (device, queue, mut renderer) = Renderer::headless().map_err(|e| format!("no Metal device: {e}"))?;
        let mut scene = renderer.new_scene(&device);
        let asm = &structure.assembly;
        let mut handles = vec![None; asm.shapes.len()];
        for (i, m) in meshes.iter().enumerate() {
            if let Some(m) = m {
                if !m.bodies.is_empty() {
                    handles[i] = Some(scene.upload_shape(&device, &queue, m));
                }
            }
        }
        let mut drawn = 0usize;
        for inst in &asm.instances {
            if let Some(Some(h)) = handles.get(inst.shape.0 as usize) {
                scene.add_instance(*h, inst.world * DAffine3::IDENTITY, inst.node.0);
                drawn += 1;
            }
        }
        if drawn == 0 || scene.bbox().is_empty() {
            return Err("no drawable geometry".into());
        }
        let mut camera = Camera::default();
        camera.standard_view(StandardView::Iso);
        camera.fit_aspect(scene.bbox(), 1.0);
        let settings = RenderSettings {
            background: BACKGROUND,
            mode: if size >= EDGES_MIN_PX { RenderMode::ShadedEdges } else { RenderMode::Shaded },
            ..RenderSettings::default()
        };
        renderer
            .render_offscreen(&device, &queue, &mut scene, &camera, &settings, size, size)
            .map_err(|e| format!("offscreen render failed: {e}"))
    };
    // wgpu is allowed to panic on device loss / validation; never let that cross the C ABI.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| Err("renderer panicked".into()))
}

pub fn thumbnail(path: &Path, size_px: u32, budget_ms: u32) -> Result<Thumbnail, Failure> {
    let size = size_px.clamp(16, 4096);
    let t0 = Instant::now();
    let budget = (budget_ms > 0).then(|| Duration::from_millis(budget_ms as u64));
    let deadline = budget.map(|b| t0 + b);
    let over = |d: Option<Instant>| d.is_some_and(|d| Instant::now() >= d);

    let file = load::index(path).map_err(|e| (crate::SV_ERR_OPEN, e))?;
    let mut degraded = over(deadline);

    let structure = step_brep::load_structure(&file, true);
    degraded |= over(deadline);
    if structure.assembly.instances.is_empty() {
        return Err((crate::SV_ERR_EMPTY, "file has no shape instances".to_string()));
    }

    if degraded {
        // Over budget already: bounds only, no tessellation, and only for as long as half of what
        // is left of a second budget's worth of time.
        let slice = budget.map(|b| Instant::now() + b.mul_f32(DEGRADED_SLICE));
        let bounds = load::shape_bounds(&file, &structure, slice);
        let boxes: Vec<(step_mesh::Aabb, DAffine3)> = structure
            .assembly
            .instances
            .iter()
            .filter_map(|i| bounds.get(i.shape.0 as usize).and_then(|b| *b).map(|b| (b, i.world)))
            .collect();
        let img = raster::render_boxes(&boxes, size)
            .ok_or((crate::SV_ERR_EMPTY, "over budget and no B-rep bounds to draw".to_string()))?;
        crate::set_error(format!("over the {budget_ms} ms budget after the product structure; drew bounding boxes"));
        return Ok(Thumbnail { png: encode_png(&img)?, code: crate::SV_DEGRADED });
    }

    let cached = load::cache_lookup(&file, &structure, &TessParams::PREVIEW);
    let (meshes, complete) = match cached {
        Some(m) => (m, true),
        None => load::tessellate_all(&file, &structure, &TessParams::COARSE, deadline),
    };
    let mut code = if complete { crate::SV_OK } else { crate::SV_DEGRADED };

    match gpu_render(&structure, &meshes, size) {
        Ok(img) => Ok(Thumbnail { png: encode_png(&img)?, code }),
        Err(gpu_err) => {
            // No Metal device (or it failed): fall back to CPU box silhouettes of the meshes we do
            // have, so the extension still returns an image instead of a blank tile.
            let boxes = Model::instance_boxes(&structure, &meshes);
            let img = raster::render_boxes(&boxes, size)
                .ok_or_else(|| (crate::SV_ERR_RENDER, format!("{gpu_err}; no bounding boxes to fall back to")))?;
            code = crate::SV_DEGRADED;
            crate::set_error(format!("{gpu_err}; drew bounding boxes on the CPU"));
            Ok(Thumbnail { png: encode_png(&img)?, code })
        }
    }
}
