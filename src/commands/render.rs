//! `stepview render`: headless PNG.

use std::path::Path;

use anyhow::Context;
use glam::DAffine3;
use step_render::{Camera, RenderSettings, Renderer, StandardView};

use crate::pipeline::{LoadOpts, load_all};

pub struct RenderOpts {
    pub size: (u32, u32),
    pub view: String,
    pub background: [f32; 4],
    pub only: Option<String>,
}

pub fn parse_size(s: &str) -> anyhow::Result<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X']).context("size must be WIDTHxHEIGHT")?;
    Ok((w.trim().parse()?, h.trim().parse()?))
}

/// `#rrggbb` (sRGB) → linear RGBA.
pub fn parse_color(s: &str) -> anyhow::Result<[f32; 4]> {
    let h = s.trim().trim_start_matches('#');
    anyhow::ensure!(h.len() == 6, "colour must be #rrggbb");
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map(|v| srgb_to_linear(v as f32 / 255.0));
    Ok([c(0)?, c(2)?, c(4)?, 1.0])
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn apply_view(camera: &mut Camera, view: &str) -> anyhow::Result<()> {
    let v = match view.to_ascii_lowercase().as_str() {
        "iso" => Some(StandardView::Iso),
        "top" => Some(StandardView::Top),
        "bottom" => Some(StandardView::Bottom),
        "front" => Some(StandardView::Front),
        "back" => Some(StandardView::Back),
        "left" => Some(StandardView::Left),
        "right" => Some(StandardView::Right),
        _ => None,
    };
    match v {
        Some(v) => camera.standard_view(v),
        None => {
            let (yaw, pitch) = view.split_once(',').context("view must be a name or yaw,pitch in degrees")?;
            let yaw: f64 = yaw.trim().parse()?;
            let pitch: f64 = pitch.trim().parse()?;
            camera.standard_view(StandardView::Front);
            camera.orbit(yaw.to_radians(), pitch.to_radians());
        }
    }
    Ok(())
}

pub fn run(file: &Path, out: &Path, ropts: &RenderOpts, lopts: &LoadOpts) -> anyhow::Result<()> {
    let loaded = load_all(file, lopts)?;
    let (device, queue, mut renderer) = Renderer::headless().context("creating a Metal device")?;
    let mut scene = renderer.new_scene(&device);
    let asm = &loaded.structure.assembly;
    let mut handles = vec![None; asm.shapes.len()];
    for (i, m) in loaded.meshes.iter().enumerate() {
        if let Some(m) = m {
            handles[i] = Some(scene.upload_shape(&device, &queue, m));
        }
    }
    let filter = ropts.only.as_ref().map(|f| f.to_ascii_lowercase());
    let mut shown = 0usize;
    for inst in &asm.instances {
        if let Some(h) = handles[inst.shape.0 as usize] {
            let node = &asm.nodes[inst.node.0 as usize];
            let visible = filter.as_ref().is_none_or(|f| node.path.to_ascii_lowercase().contains(f));
            let ih = scene.add_instance(h, inst.world * DAffine3::IDENTITY, inst.node.0);
            if !visible {
                scene.set_visible(ih, false);
            } else {
                shown += 1;
            }
        }
    }
    anyhow::ensure!(shown > 0, "no assembly node matches --only {:?}", ropts.only);
    let mut camera = Camera::default();
    apply_view(&mut camera, &ropts.view)?;
    let aspect = ropts.size.0 as f64 / ropts.size.1 as f64;
    camera.fit_aspect(scene.bbox(), aspect);
    let settings = RenderSettings { background: ropts.background, ..RenderSettings::default() };
    let t = std::time::Instant::now();
    let img = renderer.render_offscreen(&device, &queue, &mut scene, &camera, &settings, ropts.size.0, ropts.size.1)?;
    let render_ms = t.elapsed().as_secs_f32() * 1000.0;
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    img.save(out).with_context(|| format!("writing {}", out.display()))?;
    let tris = crate::pipeline::instanced_triangles(&loaded.structure, &loaded.meshes);
    let t = &loaded.timings;
    println!(
        "wrote {} ({}x{}): {} of {} instances, {} triangles drawn; index {:.0} ms, structure {:.0} ms, mesh {:.0} ms, render {:.0} ms, cache {:?}",
        out.display(),
        ropts.size.0,
        ropts.size.1,
        shown,
        asm.instances.len(),
        tris,
        t.index_ms,
        t.structure_ms,
        t.mesh_ms,
        render_ms,
        loaded.cache
    );
    Ok(())
}
