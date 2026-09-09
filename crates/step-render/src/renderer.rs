//! Pipelines, attachments and the three draw passes.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Vec3};

use crate::camera::Camera;
use crate::error::RenderError;
use crate::scene::{Scene, vertex_layout};
use crate::settings::{RenderMode, RenderSettings};
use crate::targets::Targets;
use crate::{DEPTH_FORMAT, ID_FORMAT};

/// Colour views the caller supplies to [`Renderer::render`].
///
/// Only the *resolved*, single-sampled colour target is external; the MSAA colour, depth and ID
/// attachments are owned by the renderer.
#[derive(Clone, Copy, Debug)]
pub struct TargetViews<'a> {
    pub color: &'a wgpu::TextureView,
}

/// Raw contents of the ID attachment at one pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawPick {
    /// Zero-based instance index (the attachment stores this + 1).
    pub instance_index: u32,
    /// Shape-global face index.
    pub face_index: u32,
    /// Depth buffer value at that pixel, `0..=1`.
    pub depth: f32,
}

/// A resolved pick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickResult {
    pub instance: crate::InstanceHandle,
    /// The `node_id` the instance was created with.
    pub node_id: u32,
    /// Index into the body's `face_ranges` (the raw `face_slot`).
    pub face_slot: u32,
    /// STEP `#id` of the picked `ADVANCED_FACE`.
    pub face_src: u32,
    /// Index into [`step_mesh::ShapeMesh::bodies`].
    pub body: u32,
    /// Shape-global face index (`body_face_base + face_slot`).
    pub face_index: u32,
    /// Depth buffer value, `0..=1`.
    pub depth: f32,
}

/// The bind-group layouts a [`Scene`] needs in order to build its own bind groups.
///
/// Cheap to clone (every wgpu handle is refcounted).
#[derive(Clone, Debug)]
pub struct Layouts {
    globals: wgpu::BindGroupLayout,
    instances: wgpu::BindGroupLayout,
    faces: wgpu::BindGroupLayout,
    pipeline: wgpu::PipelineLayout,
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Layouts {
        let storage = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("step-render globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let instances = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("step-render instances"),
            entries: &[storage(0), storage(1)],
        });
        let faces = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("step-render face colours"),
            entries: &[storage(0)],
        });
        let pipeline = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("step-render"),
            bind_group_layouts: &[Some(&globals), Some(&instances), Some(&faces)],
            immediate_size: 0,
        });
        Layouts { globals, instances, faces, pipeline }
    }

    pub(crate) fn instance_bind_group(
        &self,
        device: &wgpu::Device,
        instances: &wgpu::Buffer,
        face_sel: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("step-render instances"),
            layout: &self.instances,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instances.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: face_sel.as_entire_binding() },
            ],
        })
    }

    pub(crate) fn face_bind_group(&self, device: &wgpu::Device, colors: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("step-render face colours"),
            layout: &self.faces,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: colors.as_entire_binding() }],
        })
    }
}

/// Uniform block shared by all three shaders; mirrors `Globals` in `shaders/common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuGlobals {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    clip_plane: [f32; 4],
    cap_color: [f32; 4],
    edge_color: [f32; 4],
    sel_color: [f32; 4],
    light_dir: [f32; 4],
    up_axis: [f32; 4],
    fill_dir: [f32; 4],
    view_dir: [f32; 4],
    params: [f32; 4],
    modes: [u32; 4],
}

/// The multisample-count-dependent pipelines.
struct MsaaPipelines {
    shaded_cull: wgpu::RenderPipeline,
    shaded_nocull: wgpu::RenderPipeline,
    transparent_cull: wgpu::RenderPipeline,
    transparent_nocull: wgpu::RenderPipeline,
    transparent_edge: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    edge: wgpu::RenderPipeline,
}

/// Draws [`Scene`]s. One renderer per colour format; it owns its pipelines and attachments.
pub struct Renderer {
    color_format: wgpu::TextureFormat,
    layouts: Layouts,
    shaded_module: wgpu::ShaderModule,
    edge_module: wgpu::ShaderModule,
    pipelines: rustc_hash::FxHashMap<u32, MsaaPipelines>,
    id_pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    targets: Option<Targets>,
}

const COMMON_WGSL: &str = include_str!("shaders/common.wgsl");
const SHADED_WGSL: &str = include_str!("shaders/shaded.wgsl");
const EDGE_WGSL: &str = include_str!("shaders/edge.wgsl");
const ID_WGSL: &str = include_str!("shaders/id.wgsl");

/// NDC depth the edge pass is nudged towards the viewer.
const EDGE_DEPTH_BIAS: f32 = 2.0e-4;

impl Renderer {
    /// Build the renderer for a given colour target format (e.g. the surface format, or
    /// `Rgba8UnormSrgb` for offscreen rendering).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, color_format: wgpu::TextureFormat) -> Renderer {
        let _ = queue;
        let layouts = Layouts::new(device);
        let module = |label: &str, body: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(format!("{COMMON_WGSL}\n{body}").into()),
            })
        };
        let shaded_module = module("step-render shaded", SHADED_WGSL);
        let edge_module = module("step-render edge", EDGE_WGSL);
        let id_module = module("step-render id", ID_WGSL);

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("step-render globals"),
            size: core::mem::size_of::<GpuGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("step-render globals"),
            layout: &layouts.globals,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buffer.as_entire_binding() }],
        });

        let id_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("step-render id"),
            layout: Some(&layouts.pipeline),
            vertex: wgpu::VertexState {
                module: &id_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::Less)),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &id_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: ID_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Renderer {
            color_format,
            layouts,
            shaded_module,
            edge_module,
            pipelines: rustc_hash::FxHashMap::default(),
            id_pipeline,
            globals_buffer,
            globals_bind_group,
            targets: None,
        }
    }

    /// Create an instance/adapter/device on this machine and a renderer for `Rgba8UnormSrgb`.
    ///
    /// Intended for CLI tools and tests; returns [`RenderError::NoAdapter`] when there is no GPU.
    pub fn headless() -> Result<(wgpu::Device, wgpu::Queue, Renderer), RenderError> {
        Self::headless_with_format(wgpu::TextureFormat::Rgba8UnormSrgb)
    }

    /// [`Renderer::headless`] with an explicit colour format.
    pub fn headless_with_format(
        format: wgpu::TextureFormat,
    ) -> Result<(wgpu::Device, wgpu::Queue, Renderer), RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("step-render headless"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| RenderError::NoDevice(e.to_string()))?;

        let renderer = Renderer::new(&device, &queue, format);
        Ok((device, queue, renderer))
    }

    /// An empty scene wired to this renderer's bind-group layouts.
    pub fn new_scene(&self, device: &wgpu::Device) -> Scene {
        Scene::new(device, &self.layouts)
    }

    /// The bind-group layouts, for callers that build [`Scene`]s themselves.
    pub fn layouts(&self) -> &Layouts {
        &self.layouts
    }

    /// The colour format this renderer's pipelines were built for.
    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    /// Size of the internal attachments, or `None` before the first [`Renderer::render`].
    pub fn target_size(&self) -> Option<(u32, u32)> {
        self.targets.as_ref().map(|t| (t.width, t.height))
    }

    // --------------------------------------------------------------- drawing

    /// Draw `scene` into `target.color`.
    ///
    /// `scene` is taken mutably because the instance storage buffer is rebuilt lazily; call
    /// [`Scene::flush`] yourself if you need an immutable borrow here.
    ///
    /// The caller owns `encoder` and must submit it. [`Renderer::pick`] reads the ID attachment
    /// written by this call, so submit before picking.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &mut Scene,
        camera: &Camera,
        settings: &RenderSettings,
        target: &TargetViews<'_>,
        size: (u32, u32),
    ) {
        let (width, height) = (size.0.max(1), size.1.max(1));
        let samples = settings.sample_count();
        self.ensure_targets(device, width, height, samples);
        self.ensure_pipelines(device, samples);
        scene.flush(device, queue);

        let aspect = width as f64 / height as f64;
        let transparent = settings.mode == RenderMode::XRay || scene.draws().iter().any(|d| d.has_transparency);
        if transparent {
            self.targets.as_mut().unwrap().ensure_transparency(device);
        }
        let mut globals = self.build_globals(scene, camera, settings, aspect);
        globals.modes[3] = u32::from(transparent);
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let targets = self.targets.as_ref().expect("targets created above");
        let pipes = self.pipelines.get(&samples).expect("pipelines created above");
        let clip_on = settings.clip_plane.is_some();
        let bg = settings.background;
        let clear = wgpu::Color { r: (bg[0] * bg[3]) as f64, g: (bg[1] * bg[3]) as f64, b: (bg[2] * bg[3]) as f64, a: bg[3] as f64 };
        let color_view = &targets.color;
        let resolve_target = None;

        // ---------------------------------------------------------- shaded
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("step-render shaded"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, scene.instance_bind_group(), &[]);
            for item in scene.draws().iter().filter(|_| settings.mode.draws_triangles()) {
                let shape = scene.shape_at(item.shape);
                let body = &shape.bodies[item.body as usize];
                if body.index_count == 0 {
                    continue;
                }
                let pipe = if body.double_sided || clip_on {
                    &pipes.shaded_nocull
                } else {
                    &pipes.shaded_cull
                };
                pass.set_pipeline(pipe);
                pass.set_bind_group(2, &shape.face_bind_group, &[]);
                pass.set_vertex_buffer(0, body.vertices.slice(..));
                pass.set_index_buffer(body.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    0..body.index_count,
                    0,
                    item.first_instance..item.first_instance + item.instance_count,
                );
            }
        }

        // ----------------------------------------------------------- edges
        if settings.mode.draws_edges() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("step-render edges"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color_view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&pipes.edge);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, scene.instance_bind_group(), &[]);
            for item in scene.draws() {
                let shape = scene.shape_at(item.shape);
                let body = &shape.bodies[item.body as usize];
                if body.edge_count == 0 {
                    continue;
                }
                let Some(edges) = &body.edges else { continue };
                pass.set_bind_group(2, &shape.face_bind_group, &[]);
                pass.set_vertex_buffer(0, body.vertices.slice(..));
                pass.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    0..body.edge_count,
                    0,
                    item.first_instance..item.first_instance + item.instance_count,
                );
            }
        }

        // OIT keeps instanced batches intact: each fragment chooses its pass by effective alpha.
        if let Some((accum, reveal)) = targets.transparency.as_ref().filter(|_| transparent) {
            let attachment = |view, clear| Some(wgpu::RenderPassColorAttachment {
                view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("step-render transparency"),
                color_attachments: &[attachment(accum, wgpu::Color::TRANSPARENT), attachment(reveal, wgpu::Color::WHITE)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, scene.instance_bind_group(), &[]);
            for item in scene.draws().iter().filter(|d| d.has_transparency || settings.mode == RenderMode::XRay) {
                let shape = scene.shape_at(item.shape);
                let body = &shape.bodies[item.body as usize];
                pass.set_bind_group(2, &shape.face_bind_group, &[]);
                pass.set_vertex_buffer(0, body.vertices.slice(..));
                if settings.mode.draws_triangles() && body.index_count > 0 {
                    let nocull = body.double_sided || clip_on || settings.mode == RenderMode::XRay;
                    pass.set_pipeline(if nocull { &pipes.transparent_nocull } else { &pipes.transparent_cull });
                    pass.set_index_buffer(body.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..body.index_count, 0, item.first_instance..item.first_instance + item.instance_count);
                }
                if settings.mode.draws_edges() && let Some(edges) = &body.edges {
                    pass.set_pipeline(&pipes.transparent_edge);
                    pass.set_index_buffer(edges.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..body.edge_count, 0, item.first_instance..item.first_instance + item.instance_count);
                }
            }
        }

        // Composite per sample before resolving, then encode exactly once for the external target.
        {
            let (accum, reveal) = targets.transparency.as_ref().map(|(a, r)| (a, r)).unwrap_or((color_view, color_view));
            let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("step-render composite textures"), layout: &pipes.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(color_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(accum) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(reveal) },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("step-render composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.color, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipes.composite);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, &textures, &[]);
            pass.draw(0..3, 0..1);
        }

        // -------------------------------------------------------------- ID
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("step-render id"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.id_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.id_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.id_pipeline);
            pass.set_bind_group(0, &self.globals_bind_group, &[]);
            pass.set_bind_group(1, scene.instance_bind_group(), &[]);
            for item in scene.draws() {
                let shape = scene.shape_at(item.shape);
                let body = &shape.bodies[item.body as usize];
                if body.index_count == 0 {
                    continue;
                }
                pass.set_bind_group(2, &shape.face_bind_group, &[]);
                pass.set_vertex_buffer(0, body.vertices.slice(..));
                pass.set_index_buffer(body.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    0..body.index_count,
                    0,
                    item.first_instance..item.first_instance + item.instance_count,
                );
            }
        }
    }

    /// Render into an internally created texture and read it back as RGBA8.
    ///
    /// The colour format is [`Renderer::color_format`]; only 8-bit RGBA/BGRA formats can be read
    /// back (BGRA is swizzled on the CPU).
    #[allow(clippy::too_many_arguments)]
    pub fn render_offscreen(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &mut Scene,
        camera: &Camera,
        settings: &RenderSettings,
        width: u32,
        height: u32,
    ) -> Result<image::RgbaImage, RenderError> {
        let swizzle = match self.color_format {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            other => return Err(RenderError::UnsupportedFormat(other)),
        };
        let (width, height) = (width.max(1), height.max(1));

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("step-render offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let unpadded = width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("step-render offscreen readback"),
            size: (padded as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("step-render offscreen") });
        self.render(device, queue, &mut encoder, scene, camera, settings, &TargetViews { color: &view }, (width, height));
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));

        let bytes = map_read(device, &readback)?;
        let mut out = image::RgbaImage::new(width, height);
        for y in 0..height {
            let row = &bytes[(y as usize) * (padded as usize)..][..unpadded as usize];
            for x in 0..width {
                let p = &row[(x as usize) * 4..][..4];
                let px = if swizzle { [p[2], p[1], p[0], p[3]] } else { [p[0], p[1], p[2], p[3]] };
                out.put_pixel(x, y, image::Rgba(px));
            }
        }
        Ok(out)
    }

    // --------------------------------------------------------------- picking

    /// Read the ID attachment at one pixel and resolve it against `scene`.
    ///
    /// Synchronous: it submits a 1×1 copy and blocks on `Device::poll`. Returns `None` for
    /// background pixels, out-of-range coordinates, or before the first [`Renderer::render`].
    pub fn pick(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        x: u32,
        y: u32,
    ) -> Option<PickResult> {
        let raw = self.pick_raw(device, queue, x, y)?;
        let (instance, node_id, face) = scene.resolve_pick(raw.instance_index + 1, raw.face_index)?;
        Some(PickResult {
            instance,
            node_id,
            face_slot: face.face_slot,
            face_src: face.face_src,
            body: face.body,
            face_index: raw.face_index,
            depth: raw.depth,
        })
    }

    /// The unresolved contents of the ID and depth attachments at one pixel.
    pub fn pick_raw(&self, device: &wgpu::Device, queue: &wgpu::Queue, x: u32, y: u32) -> Option<RawPick> {
        let targets = self.targets.as_ref()?;
        if x >= targets.width || y >= targets.height {
            return None;
        }
        let id_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("step-render pick"),
            size: wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("step-render pick") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.id_texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &id_buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        queue.submit(Some(encoder.finish()));

        let bytes = map_read(device, &id_buf).ok()?;
        let pick_id = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let face_index = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let depth = f32::from_le_bytes(bytes[8..12].try_into().ok()?);

        (pick_id != 0).then(|| RawPick { instance_index: pick_id - 1, face_index, depth })
    }

    // -------------------------------------------------------------- internal

    fn ensure_targets(&mut self, device: &wgpu::Device, width: u32, height: u32, samples: u32) {
        let ok = self.targets.as_ref().is_some_and(|t| t.matches(width, height, samples, self.color_format));
        if !ok {
            self.targets = Some(Targets::new(device, width, height, samples, self.color_format));
        }
    }

    fn ensure_pipelines(&mut self, device: &wgpu::Device, samples: u32) {
        if self.pipelines.contains_key(&samples) {
            return;
        }
        let ms = wgpu::MultisampleState { count: samples, ..Default::default() };
        let opaque_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba16Float, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL,
        })];
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let reveal = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::OneMinusSrc, operation: wgpu::BlendOperation::Add };
        let transparent_targets = [
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: Some(wgpu::BlendState { color: add, alpha: add }), write_mask: wgpu::ColorWrites::ALL }),
            Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: Some(wgpu::BlendState { color: reveal, alpha: reveal }), write_mask: wgpu::ColorWrites::RED }),
        ];
        let geometry = |label: &str, edges: bool, cull: Option<wgpu::Face>, transparent: bool| {
            let module = if edges { &self.edge_module } else { &self.shaded_module };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label), layout: Some(&self.layouts.pipeline),
                vertex: wgpu::VertexState { module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[Some(vertex_layout())] },
                primitive: wgpu::PrimitiveState { topology: if edges { wgpu::PrimitiveTopology::LineList } else { wgpu::PrimitiveTopology::TriangleList }, cull_mode: cull, ..Default::default() },
                depth_stencil: Some(depth_state(!transparent && !edges, if edges { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Less })),
                multisample: ms,
                fragment: Some(wgpu::FragmentState {
                    module, entry_point: Some(if transparent { "fs_transparent" } else { "fs_main" }), compilation_options: Default::default(),
                    targets: if transparent { &transparent_targets } else { &opaque_targets },
                }),
                multiview_mask: None, cache: None,
            })
        };
        let entries: Vec<_> = (0..3).map(|binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: samples > 1 },
            count: None,
        }).collect();
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("composite textures"), entries: &entries });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite"), bind_group_layouts: &[Some(&self.layouts.globals), Some(&composite_layout)], immediate_size: 0,
        });
        let shader = include_str!("shaders/composite.wgsl")
            .replace("TEXTURE_TYPE", if samples > 1 { "texture_multisampled_2d" } else { "texture_2d" })
            .replace("SAMPLE_COUNT", &samples.to_string());
        let globals = COMMON_WGSL.split("struct Instance").next().unwrap();
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite"), source: wgpu::ShaderSource::Wgsl(format!("{globals}\n{shader}").into()),
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite"), layout: Some(&layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: self.color_format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None, cache: None,
        });
        let pipes = MsaaPipelines {
            shaded_cull: geometry("opaque cull", false, Some(wgpu::Face::Back), false),
            shaded_nocull: geometry("opaque double sided", false, None, false),
            transparent_cull: geometry("transparent cull", false, Some(wgpu::Face::Back), true),
            transparent_nocull: geometry("transparent double sided", false, None, true),
            transparent_edge: geometry("transparent edges", true, None, true),
            edge: geometry("opaque edges", true, None, false),
            composite, composite_layout,
        };
        self.pipelines.insert(samples, pipes);
    }

    fn build_globals(&self, scene: &Scene, camera: &Camera, settings: &RenderSettings, aspect: f64) -> GpuGlobals {
        let origin = scene.render_origin();
        let view_proj = camera.view_proj_relative(aspect, origin);
        let eye = (camera.eye() - origin).as_vec3();

        // Key light: roughly from the camera, lifted and pushed left so curvature reads.
        let l = (-camera.forward() + camera.up() * 0.55 - camera.right() * 0.35).normalize_or(DVec3::Z);
        let light = l.as_vec3();

        let (clip, cap, clip_on) = match settings.clip_plane {
            Some(cp) => {
                let n = cp.normal.normalize_or(DVec3::Z);
                // Rebase the plane onto the render origin: dot(n, p_render) <= offset - dot(n, o).
                let w = -(cp.offset - n.dot(origin));
                (
                    [n.x as f32, n.y as f32, n.z as f32, w as f32],
                    srgb_u8_to_linear(cp.cap_color),
                    1u32,
                )
            }
            None => ([0.0, 0.0, 1.0, 0.0], [0.0; 4], 0u32),
        };

        let sel = srgb_u8_to_linear(settings.selection_color);
        GpuGlobals {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: [eye.x, eye.y, eye.z, 0.0],
            clip_plane: clip,
            cap_color: cap,
            edge_color: srgb_u8_to_linear(settings.edge_color),
            sel_color: sel,
            light_dir: [light.x, light.y, light.z, 0.0],
            up_axis: {
                let u: Vec3 = camera.up_axis.up().as_vec3();
                [u.x, u.y, u.z, 0.0]
            },
            fill_dir: {
                let f = (-camera.forward() - camera.up() * 0.55 + camera.right() * 0.75).normalize().as_vec3();
                [f.x, f.y, f.z, 0.0]
            },
            view_dir: {
                let f = camera.forward().as_vec3();
                [f.x, f.y, f.z, f32::from(camera.ortho)]
            },
            params: [
                if settings.mode == RenderMode::XRay { settings.xray_alpha.clamp(0.0, 1.0) } else { 1.0 },
                EDGE_DEPTH_BIAS,
                0.0,
                0.0,
            ],
            modes: [
                clip_on,
                u32::from(self.color_format.is_srgb()),
                u32::from(settings.show_selection_highlight),
                0,
            ],
        }
    }
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn srgb_u8_to_linear(c: [u8; 4]) -> [f32; 4] {
    let f = |v: u8| {
        let x = v as f32 / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3] as f32 / 255.0]
}

/// Map a `MAP_READ` buffer synchronously and copy it out. The buffer is unmapped before returning,
/// so the caller owns plain bytes and no mapping stays alive.
fn map_read(device: &wgpu::Device, buffer: &wgpu::Buffer) -> Result<Vec<u8>, RenderError> {
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| RenderError::Poll(e.to_string()))?;
    match rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(RenderError::Readback(e.to_string())),
        Err(e) => return Err(RenderError::Readback(e.to_string())),
    }
    let out = {
        let view = slice.get_mapped_range().map_err(|e| RenderError::Readback(e.to_string()))?;
        view.to_vec()
    };
    buffer.unmap();
    Ok(out)
}
