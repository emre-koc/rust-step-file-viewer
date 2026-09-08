//! A tiny CPU rasteriser for box silhouettes.
//!
//! Used for the two degraded thumbnail paths: over the time budget (we only have cheap per-shape
//! bounds, not meshes) and no Metal device (a sandboxed extension that cannot reach the GPU).
//! It draws each instance's oriented bounding box as 12 shaded triangles with a z-buffer plus dark
//! feature edges, under the same isometric view and transparent background as the GPU renderer, so
//! the two paths look like the same family of image.
//!
//! Output is sRGB-encoded RGBA8, alpha 0 where nothing was drawn, rendered 2× and box-filtered down
//! for antialiasing.

use glam::{DAffine3, DVec3};
use step_mesh::Aabb;

/// Base body colour (linear).
const BASE: [f32; 3] = [0.62, 0.65, 0.70];
/// Edge colour (linear), matching `RenderSettings::default().edge_color` in spirit.
const EDGE: [f32; 3] = [0.02, 0.025, 0.035];
const SUPERSAMPLE: u32 = 2;

/// Camera basis for the Z-up isometric view, identical to `StandardView::Iso`:
/// forward = normalize(front - right - up) with front = +Y, right = +X, up = +Z.
fn iso_basis() -> (DVec3, DVec3, DVec3) {
    let up = DVec3::Z;
    let forward = (DVec3::Y - DVec3::X - DVec3::Z).normalize();
    let zc = -forward; // camera looks down its own -Z
    let xc = up.cross(zc).normalize();
    let yc = zc.cross(xc);
    (xc, yc, zc)
}

/// The 8 corners of `bb` after `world`, in the given corner order (bit 0 = x, 1 = y, 2 = z).
fn corners(bb: &Aabb, world: &DAffine3) -> [DVec3; 8] {
    let mut c = [DVec3::ZERO; 8];
    for (i, out) in c.iter_mut().enumerate() {
        let p = DVec3::new(
            if i & 1 == 0 { bb.min.x } else { bb.max.x },
            if i & 2 == 0 { bb.min.y } else { bb.max.y },
            if i & 4 == 0 { bb.min.z } else { bb.max.z },
        );
        *out = world.transform_point3(p);
    }
    c
}

/// Quads of the box, as corner indices forming a cycle around each face (winding is not\n/// guaranteed outward; see the normal flip in `render_boxes`).
const FACES: [[usize; 4]; 6] = [
    [0, 2, 6, 4], // -x
    [1, 5, 7, 3], // +x
    [0, 4, 5, 1], // -y
    [2, 3, 7, 6], // +y
    [0, 1, 3, 2], // -z
    [4, 6, 7, 5], // +z
];
const EDGES: [[usize; 2]; 12] =
    [[0, 1], [2, 3], [4, 5], [6, 7], [0, 2], [1, 3], [4, 6], [5, 7], [0, 4], [1, 5], [2, 6], [3, 7]];

struct Target {
    w: u32,
    h: u32,
    rgb: Vec<[f32; 3]>,
    cov: Vec<f32>,
    depth: Vec<f32>,
}

impl Target {
    fn new(w: u32, h: u32) -> Target {
        let n = (w as usize) * (h as usize);
        Target { w, h, rgb: vec![[0.0; 3]; n], cov: vec![0.0; n], depth: vec![f32::NEG_INFINITY; n] }
    }
    #[inline]
    fn put(&mut self, x: i32, y: i32, d: f32, c: [f32; 3]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let i = y as usize * self.w as usize + x as usize;
        if d >= self.depth[i] {
            self.depth[i] = d;
            self.rgb[i] = c;
            self.cov[i] = 1.0;
        }
    }
}

/// Projected vertex: screen x/y and camera-space depth (larger = closer to the eye).
#[derive(Clone, Copy)]
struct Pv {
    x: f32,
    y: f32,
    d: f32,
}

fn fill_triangle(t: &mut Target, a: Pv, b: Pv, c: Pv, color: [f32; 3]) {
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as i32;
    let max_x = a.x.max(b.x).max(c.x).ceil().min(t.w as f32 - 1.0) as i32;
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as i32;
    let max_y = a.y.max(b.y).max(c.y).ceil().min(t.h as f32 - 1.0) as i32;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-9 {
        return;
    }
    let inv = 1.0 / area;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((b.x - a.x) * (py - a.y) - (b.y - a.y) * (px - a.x)) * inv;
            let w1 = ((c.x - b.x) * (py - b.y) - (c.y - b.y) * (px - b.x)) * inv;
            let w2 = ((a.x - c.x) * (py - c.y) - (a.y - c.y) * (px - c.x)) * inv;
            // `w0` is the barycentric weight of c, `w1` of a, `w2` of b.
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let d = w1 * a.d + w2 * b.d + w0 * c.d;
            t.put(x, y, d, color);
        }
    }
}

fn draw_line(t: &mut Target, a: Pv, b: Pv, color: [f32; 3], bias: f32) {
    let steps = ((b.x - a.x).abs().max((b.y - a.y).abs()).ceil() as i32).max(1);
    for s in 0..=steps {
        let f = s as f32 / steps as f32;
        let x = a.x + (b.x - a.x) * f;
        let y = a.y + (b.y - a.y) * f;
        let d = a.d + (b.d - a.d) * f + bias;
        t.put(x.round() as i32, y.round() as i32, d, color);
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Render oriented boxes into a square RGBA image. Returns `None` when nothing is visible.
pub fn render_boxes(boxes: &[(Aabb, DAffine3)], size: u32) -> Option<image::RgbaImage> {
    let size = size.clamp(16, 4096);
    let ss = if size <= 1024 { SUPERSAMPLE } else { 1 };
    let (xc, yc, zc) = iso_basis();

    // Project every corner once so the fit and the draw see the same numbers.
    let mut all: Vec<[DVec3; 8]> = Vec::with_capacity(boxes.len());
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for (bb, world) in boxes {
        if bb.is_empty() {
            continue;
        }
        let c = corners(bb, world);
        for p in &c {
            let (u, v) = (p.dot(xc), p.dot(yc));
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
        }
        all.push(c);
    }
    if all.is_empty() || !umin.is_finite() {
        return None;
    }

    let dim = (size * ss) as f64;
    let margin = dim * 0.06;
    let span = (umax - umin).max(vmax - vmin).max(1e-9);
    let scale = (dim - 2.0 * margin) / span;
    let ox = margin + ((umax - umin).max(0.0).mul_add(-scale, dim - 2.0 * margin)) * 0.5;
    let oy = margin + ((vmax - vmin).max(0.0).mul_add(-scale, dim - 2.0 * margin)) * 0.5;
    let project = |p: DVec3| -> Pv {
        let u = (p.dot(xc) - umin) * scale + ox;
        let v = (p.dot(yc) - vmin) * scale + oy;
        Pv { x: u as f32, y: (dim - v) as f32, d: p.dot(zc) as f32 }
    };

    let mut t = Target::new(size * ss, size * ss);
    let light = DVec3::new(0.35, 0.45, 0.82).normalize();
    for c in &all {
        let pv: Vec<Pv> = c.iter().map(|p| project(*p)).collect();
        for q in FACES {
            let (p0, p1, p2) = (c[q[0]], c[q[1]], c[q[2]]);
            let n = (p1 - p0).cross(p2 - p0);
            if n.length_squared() < 1e-18 {
                continue;
            }
            // Winding is not guaranteed outward for every face of the corner-bit ordering, so
            // flip towards the eye and let the z-buffer decide which quad is actually visible.
            let mut n = n.normalize();
            if n.dot(zc) < 0.0 {
                n = -n;
            }
            let lam = n.dot(light).max(0.0);
            let hemi = 0.5 + 0.5 * n.dot(DVec3::Z);
            let k = (0.30 + 0.35 * hemi + 0.55 * lam) as f32;
            let col = [BASE[0] * k, BASE[1] * k, BASE[2] * k];
            fill_triangle(&mut t, pv[q[0]], pv[q[1]], pv[q[2]], col);
            fill_triangle(&mut t, pv[q[0]], pv[q[2]], pv[q[3]], col);
        }
        let bias = (scale as f32) * 1e-4 + 1e-3;
        for e in EDGES {
            draw_line(&mut t, pv[e[0]], pv[e[1]], EDGE, bias);
        }
    }

    // Box-filter down: coverage becomes the alpha, colour is averaged over covered samples only so
    // silhouette pixels are not darkened towards black.
    let mut img = image::RgbaImage::new(size, size);
    let s = ss as usize;
    for y in 0..size as usize {
        for x in 0..size as usize {
            let (mut acc, mut cov) = ([0.0f32; 3], 0.0f32);
            for dy in 0..s {
                for dx in 0..s {
                    let i = (y * s + dy) * t.w as usize + (x * s + dx);
                    if t.cov[i] > 0.0 {
                        acc[0] += t.rgb[i][0];
                        acc[1] += t.rgb[i][1];
                        acc[2] += t.rgb[i][2];
                        cov += 1.0;
                    }
                }
            }
            let px = if cov > 0.0 {
                let inv = 1.0 / cov;
                let a = cov / (s * s) as f32;
                [
                    linear_to_srgb(acc[0] * inv),
                    linear_to_srgb(acc[1] * inv),
                    linear_to_srgb(acc[2] * inv),
                    (a * 255.0 + 0.5) as u8,
                ]
            } else {
                [0, 0, 0, 0]
            };
            img.put_pixel(x as u32, y as u32, image::Rgba(px));
        }
    }
    Some(img)
}
