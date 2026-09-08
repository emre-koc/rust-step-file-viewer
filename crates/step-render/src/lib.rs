//! `step-render`: a self-contained wgpu 30 renderer for [`step_mesh::ShapeMesh`].
//!
//! The crate owns no window, surface, event loop or UI — it renders into texture views the caller
//! supplies (or, for CLI/tests, into its own offscreen texture) and is therefore usable from a
//! winit/egui app, a CLI exporter and the test-suite alike.
//!
//! # Pieces
//!
//! * [`Renderer`] — pipelines, internal attachments (MSAA colour, depth, ID) and the draw passes.
//! * [`Scene`] — GPU-resident geometry ([`Scene::upload_shape`]) plus instances of it.
//! * [`Camera`] — f64 orbit camera (target / distance / orientation) with `fit`, `orbit`, `pan`,
//!   zoom-to-cursor, `roll`, standard views and `ray` for picking.
//! * [`RenderSettings`] — draw mode, background, MSAA, clip plane, selection highlight.
//!
//! # Coordinate systems and precision
//!
//! All world maths is done in `f64` (`glam::DVec3` / `DAffine3` / `DMat4`), matching `step-mesh`'s
//! millimetre convention. Nothing in `f32` ever holds a raw world coordinate:
//!
//! * every [`step_mesh::BodyMesh`] stores positions relative to its own `origin`; the renderer folds
//!   `origin` into the instance model matrix,
//! * the scene picks a **render origin** (the centre of the visible bounding box) and instance model
//!   matrices are emitted *relative to it*,
//! * the camera's view matrix is likewise rebased onto the render origin in `f64` before the single
//!   `f32` cast, so shaders only ever see small numbers even for models placed at 10^6 mm.
//!
//! # Passes
//!
//! 1. **shaded** — MSAA colour + depth, per-face colour looked up in a storage buffer, hemisphere
//!    ambient + one key light, optional clip plane (`discard`, back faces recoloured to fake a cap),
//!    X-ray as alpha blending without depth write.
//! 2. **edges** — line list from `BodyMesh::edge_indices`, depth-biased towards the viewer so lines
//!    win against the coplanar triangles they belong to.
//! 3. **ID** — a single-sampled `Rgba32Uint` attachment (`r` = instance id + 1 with 0 meaning
//!    background, `g` = shape-global face index, `b` = the NDC depth bit-cast to `u32`) with its own
//!    single-sampled depth buffer, read back one texel at a time by [`Renderer::pick`]. Depth rides
//!    along in the colour attachment because partial copies out of a depth texture are not portable.
//!
//! Selection highlighting needs no pass of its own: the shaded vertex shader tints the face colour
//! when the instance flag or the per-face selection bitset says so.
//!
//! # Example
//!
//! ```no_run
//! use step_render::{Camera, RenderSettings, Renderer, Scene};
//! # fn demo(mesh: &step_mesh::ShapeMesh) -> Result<(), step_render::RenderError> {
//! let (device, queue, mut renderer) = Renderer::headless()?;
//! let mut scene = renderer.new_scene(&device);
//! let shape = scene.upload_shape(&device, &queue, mesh);
//! scene.add_instance(shape, glam::DAffine3::IDENTITY, 0);
//!
//! let mut camera = Camera::default();
//! camera.fit(scene.bbox());
//!
//! let img = renderer.render_offscreen(
//!     &device, &queue, &mut scene, &camera, &RenderSettings::default(), 800, 600,
//! )?;
//! img.save("preview.png").unwrap();
//! # Ok(())
//! # }
//! ```

mod camera;
mod error;
mod renderer;
mod scene;
mod settings;
mod targets;

pub use camera::{Camera, StandardView, UpAxis};
pub use error::RenderError;
pub use renderer::{Layouts, PickResult, RawPick, Renderer, TargetViews};
pub use scene::{FaceRef, InstanceHandle, Scene, ShapeHandle};
pub use settings::{ClipPlane, RenderMode, RenderSettings};

/// Depth attachment format used by every pass.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Format of the picking attachment: `r` = instance id + 1 (0 = background), `g` = shape-global face
/// index, `b` = NDC depth bit-cast to `u32`, `a` unused.
pub const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Uint;
