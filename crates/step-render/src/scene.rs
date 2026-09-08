//! GPU-resident geometry and the instances that place it in the world.
//!
//! # Buffer layout
//!
//! One [`ShapeMesh`] becomes one [`GpuShape`]:
//!
//! * per body: a **vertex buffer** of interleaved `{ position: f32x3, normal: f32x3, face_slot: u32 }`
//!   (28-byte stride) taken verbatim from [`step_mesh::BodyMesh`] — positions stay relative to the
//!   body `origin`, which is folded into the instance model matrix,
//! * per body: a **triangle index buffer** (`u32`) and an **edge index buffer** (`u32`, line list),
//! * per shape: a **face colour storage buffer** of packed `RGBA8` (`[u8;4]` little-endian in a
//!   `u32`), indexed by the *shape-global* face index `body_face_base + face_slot`.
//!
//! Instances live in one scene-wide storage buffer. Because `face_base` differs per body but must be
//! constant across an instanced draw, entries are flattened per `(body, instance)` and grouped
//! body-major, so every body is still a single `draw_indexed(.., first..first+instance_count)`.
//!
//! Face selection is a scene-wide bitset; an instance without face selection stores
//! `sel_base == u32::MAX` and the shader skips the lookup.

use bytemuck::{Pod, Zeroable};
use glam::{DAffine3, DVec3, Mat4, Vec4};
use step_mesh::{Aabb, ShapeMesh};
use wgpu::util::DeviceExt;

use crate::renderer::Layouts;

/// Handle to geometry uploaded with [`Scene::upload_shape`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShapeHandle(pub u32);

/// Handle to one placement of a shape, from [`Scene::add_instance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceHandle(pub u32);

/// Where a shape-global face index came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaceRef {
    /// Index into [`ShapeMesh::bodies`].
    pub body: u32,
    /// Index into that body's `face_ranges` (i.e. the raw `face_slot` value).
    pub face_slot: u32,
    /// STEP `#id` of the `ADVANCED_FACE` (`FaceRange::src`).
    pub face_src: u32,
}

/// Vertex as it is laid out in the GPU buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub face_slot: u32,
}

pub(crate) const VERTEX_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];

pub(crate) fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: core::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRS,
    }
}

/// Per-`(body, instance)` record consumed by every vertex shader.
///
/// 96 bytes, 16-byte aligned — matches `Instance` in `shaders/common.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct GpuInstance {
    /// Render-space model matrix: `translate(-render_origin) * world * translate(body.origin)`.
    pub model: [[f32; 4]; 4],
    pub flags: u32,
    /// Packed RGBA8 colour override, only read when [`FLAG_COLOR_OVERRIDE`] is set.
    pub color_override: u32,
    /// Shape-global face index of this body's first face.
    pub face_base: u32,
    pub node_id: u32,
    /// Word offset into the scene face-selection bitset, or [`NO_FACE_SELECTION`].
    pub sel_base: u32,
    /// Logical instance index + 1, written to the ID attachment (0 = background).
    pub pick_id: u32,
    pub _pad: [u32; 2],
}

pub(crate) const FLAG_SELECTED: u32 = 1;
pub(crate) const FLAG_COLOR_OVERRIDE: u32 = 2;
pub(crate) const FLAG_DOUBLE_SIDED: u32 = 4;
pub(crate) const NO_FACE_SELECTION: u32 = u32::MAX;

pub(crate) struct GpuBody {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
    pub edges: Option<wgpu::Buffer>,
    pub edge_count: u32,
    /// Shape-global index of this body's first face.
    pub face_base: u32,
    pub double_sided: bool,
    pub origin: DVec3,
}

pub(crate) struct GpuShape {
    pub bodies: Vec<GpuBody>,
    /// Bind group 2: the shape's packed face colours.
    pub face_bind_group: wgpu::BindGroup,
    /// Total number of faces across all bodies (size of the face colour buffer).
    pub face_count: u32,
    /// Shape-global face index → source face, for pick resolution.
    pub face_refs: Vec<FaceRef>,
    pub bbox: Aabb,
}

struct InstanceRec {
    shape: ShapeHandle,
    world: DAffine3,
    node_id: u32,
    visible: bool,
    selected: bool,
    color_override: Option<[u8; 4]>,
    /// Shape-global face indices, sorted and deduplicated. `None` == nothing selected.
    selected_faces: Option<Vec<u32>>,
}

/// One instanced draw: `body` of `shape` for instance entries `first..first + count`.
pub(crate) struct DrawItem {
    pub shape: u32,
    pub body: u32,
    pub first_instance: u32,
    pub instance_count: u32,
}

/// Uploaded geometry plus the instances that place it.
///
/// Mutating the scene only touches CPU state; the GPU-side instance buffer is rebuilt lazily by
/// [`Scene::flush`] (which [`crate::Renderer::render`] calls for you).
pub struct Scene {
    layouts: Layouts,
    shapes: Vec<GpuShape>,
    instances: Vec<InstanceRec>,
    dirty: bool,

    // --- derived, rebuilt by `flush` ---
    render_origin: DVec3,
    instance_buffer: wgpu::Buffer,
    face_sel_buffer: wgpu::Buffer,
    instance_bind_group: wgpu::BindGroup,
    draws: Vec<DrawItem>,
    /// Flattened entry index → logical instance index (for debugging / stats).
    instance_entry_count: u32,
}

impl Scene {
    /// Create an empty scene bound to a renderer's bind-group layouts.
    ///
    /// Prefer [`crate::Renderer::new_scene`].
    pub fn new(device: &wgpu::Device, layouts: &Layouts) -> Scene {
        let instance_buffer = empty_storage(device, "step-render instances", core::mem::size_of::<GpuInstance>() as u64);
        let face_sel_buffer = empty_storage(device, "step-render face selection", 4);
        let instance_bind_group =
            layouts.instance_bind_group(device, &instance_buffer, &face_sel_buffer);
        Scene {
            layouts: layouts.clone(),
            shapes: Vec::new(),
            instances: Vec::new(),
            dirty: false,
            render_origin: DVec3::ZERO,
            instance_buffer,
            face_sel_buffer,
            instance_bind_group,
            draws: Vec::new(),
            instance_entry_count: 0,
        }
    }

    // ------------------------------------------------------------- geometry

    /// Upload a tessellated shape. The mesh is not retained; only GPU buffers are kept.
    pub fn upload_shape(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &ShapeMesh) -> ShapeHandle {
        let _ = queue; // buffers are created with `create_buffer_init`; no staging writes needed.
        let mut bodies = Vec::with_capacity(mesh.bodies.len());
        let mut colors: Vec<u32> = Vec::new();
        let mut face_refs: Vec<FaceRef> = Vec::new();
        let mut bbox = Aabb::EMPTY;

        for (bi, body) in mesh.bodies.iter().enumerate() {
            let face_base = colors.len() as u32;
            for (slot, fr) in body.face_ranges.iter().enumerate() {
                colors.push(pack_rgba(fr.color));
                face_refs.push(FaceRef { body: bi as u32, face_slot: slot as u32, face_src: fr.src });
            }

            let n = body.positions.len();
            let mut verts = Vec::with_capacity(n);
            for i in 0..n {
                verts.push(GpuVertex {
                    position: body.positions[i],
                    normal: *body.normals.get(i).unwrap_or(&[0.0, 0.0, 1.0]),
                    face_slot: *body.face_slot.get(i).unwrap_or(&0),
                });
            }

            if verts.is_empty() {
                verts.push(GpuVertex::zeroed());
            }
            let tri_indices = pad_indices(&body.indices);
            let line_indices = pad_indices(&body.edge_indices);

            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("step-render vertices"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("step-render indices"),
                contents: bytemuck::cast_slice(&tri_indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let edges = if body.edge_indices.is_empty() {
                None
            } else {
                Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("step-render edge indices"),
                    contents: bytemuck::cast_slice(&line_indices),
                    usage: wgpu::BufferUsages::INDEX,
                }))
            };

            bbox.union(&body.bbox);
            bodies.push(GpuBody {
                vertices,
                indices,
                index_count: body.indices.len() as u32,
                edges,
                edge_count: body.edge_indices.len() as u32,
                face_base,
                double_sided: body.double_sided,
                origin: body.origin,
            });
        }

        if colors.is_empty() {
            colors.push(0xffff_ffff);
        }
        let face_colors = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("step-render face colours"),
            contents: bytemuck::cast_slice(&colors),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let face_bind_group = self.layouts.face_bind_group(device, &face_colors);

        if bbox.is_empty() {
            bbox = mesh.bbox;
        }
        let handle = ShapeHandle(self.shapes.len() as u32);
        self.shapes.push(GpuShape {
            bodies,
            face_bind_group,
            face_count: face_refs.len() as u32,
            face_refs,
            bbox,
        });
        self.dirty = true;
        handle
    }

    /// Number of uploaded shapes.
    pub fn shape_count(&self) -> usize {
        self.shapes.len()
    }

    /// Total face count of a shape (the domain of its shape-global face indices).
    pub fn shape_face_count(&self, shape: ShapeHandle) -> u32 {
        self.shapes.get(shape.0 as usize).map_or(0, |s| s.face_count)
    }

    /// Resolve a shape-global face index back to `(body, face_slot, face_src)`.
    pub fn face_ref(&self, shape: ShapeHandle, face_index: u32) -> Option<FaceRef> {
        self.shapes.get(shape.0 as usize)?.face_refs.get(face_index as usize).copied()
    }

    /// Shape-global face index of `face_slot` inside `body`, the inverse of [`Scene::face_ref`].
    pub fn face_index(&self, shape: ShapeHandle, body: u32, face_slot: u32) -> Option<u32> {
        let s = self.shapes.get(shape.0 as usize)?;
        let b = s.bodies.get(body as usize)?;
        let idx = b.face_base + face_slot;
        (idx < s.face_count).then_some(idx)
    }

    // ------------------------------------------------------------ instances

    /// Place `shape` in the world. `node_id` is opaque to the renderer and comes back from picking.
    pub fn add_instance(&mut self, shape: ShapeHandle, world: DAffine3, node_id: u32) -> InstanceHandle {
        let handle = InstanceHandle(self.instances.len() as u32);
        self.instances.push(InstanceRec {
            shape,
            world,
            node_id,
            visible: true,
            selected: false,
            color_override: None,
            selected_faces: None,
        });
        self.dirty = true;
        handle
    }

    /// Number of instances (including hidden ones).
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Replace an instance's world transform.
    pub fn set_instance_transform(&mut self, instance: InstanceHandle, world: DAffine3) {
        if let Some(i) = self.instances.get_mut(instance.0 as usize) {
            i.world = world;
            self.dirty = true;
        }
    }

    /// Show or hide an instance. Hidden instances are skipped by every pass and by [`Scene::bbox`].
    pub fn set_visible(&mut self, instance: InstanceHandle, visible: bool) {
        if let Some(i) = self.instances.get_mut(instance.0 as usize) {
            i.visible = visible;
            self.dirty = true;
        }
    }

    /// Whether an instance is visible.
    pub fn is_visible(&self, instance: InstanceHandle) -> bool {
        self.instances.get(instance.0 as usize).is_some_and(|i| i.visible)
    }

    /// Replace the whole-instance selection set.
    pub fn set_selected_instances(&mut self, selected: &[InstanceHandle]) {
        for i in &mut self.instances {
            i.selected = false;
        }
        for h in selected {
            if let Some(i) = self.instances.get_mut(h.0 as usize) {
                i.selected = true;
            }
        }
        self.dirty = true;
    }

    /// Replace the per-face selection of one instance. `faces` are *shape-global* face indices
    /// (see [`Scene::face_index`]); an empty slice clears the selection.
    pub fn set_selected_faces(&mut self, instance: InstanceHandle, faces: &[u32]) {
        let Some(i) = self.instances.get_mut(instance.0 as usize) else { return };
        if faces.is_empty() {
            i.selected_faces = None;
        } else {
            let mut v = faces.to_vec();
            v.sort_unstable();
            v.dedup();
            i.selected_faces = Some(v);
        }
        self.dirty = true;
    }

    /// Force an instance to a single sRGB colour, or `None` to restore per-face colours.
    pub fn set_instance_color_override(&mut self, instance: InstanceHandle, color: Option<[u8; 4]>) {
        if let Some(i) = self.instances.get_mut(instance.0 as usize) {
            i.color_override = color;
            self.dirty = true;
        }
    }

    /// The `node_id` an instance was created with.
    pub fn node_id(&self, instance: InstanceHandle) -> Option<u32> {
        self.instances.get(instance.0 as usize).map(|i| i.node_id)
    }

    /// The shape an instance draws.
    pub fn instance_shape(&self, instance: InstanceHandle) -> Option<ShapeHandle> {
        self.instances.get(instance.0 as usize).map(|i| i.shape)
    }

    /// Drop all shapes and instances (GPU buffers are released with them).
    pub fn clear(&mut self) {
        self.shapes.clear();
        self.instances.clear();
        self.draws.clear();
        self.dirty = true;
    }

    /// World bounding box of all *visible* instances.
    pub fn bbox(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for inst in &self.instances {
            if !inst.visible {
                continue;
            }
            let Some(shape) = self.shapes.get(inst.shape.0 as usize) else { continue };
            if shape.bbox.is_empty() {
                continue;
            }
            b.union(&shape.bbox.transformed(&inst.world));
        }
        b
    }

    // ------------------------------------------------------------- internal

    /// Rebuild the instance storage buffer if anything changed. Idempotent and cheap when clean.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let visible_bbox = self.bbox();
        self.render_origin = if visible_bbox.is_empty() { DVec3::ZERO } else { visible_bbox.center() };

        // Face-selection bitset: only instances that actually have one get words allocated.
        let mut sel_words: Vec<u32> = Vec::new();
        let mut sel_base = vec![NO_FACE_SELECTION; self.instances.len()];
        for (idx, inst) in self.instances.iter().enumerate() {
            let (Some(faces), Some(shape)) = (&inst.selected_faces, self.shapes.get(inst.shape.0 as usize)) else {
                continue;
            };
            if !inst.visible || shape.face_count == 0 {
                continue;
            }
            let words = shape.face_count.div_ceil(32) as usize;
            let base = sel_words.len();
            sel_words.resize(base + words, 0);
            for &f in faces {
                if f < shape.face_count {
                    sel_words[base + (f >> 5) as usize] |= 1 << (f & 31);
                }
            }
            sel_base[idx] = base as u32;
        }
        if sel_words.is_empty() {
            sel_words.push(0);
        }

        // Group visible instances by shape so each body becomes one instanced draw.
        let mut by_shape: Vec<Vec<u32>> = vec![Vec::new(); self.shapes.len()];
        for (idx, inst) in self.instances.iter().enumerate() {
            if inst.visible && (inst.shape.0 as usize) < self.shapes.len() {
                by_shape[inst.shape.0 as usize].push(idx as u32);
            }
        }

        let mut entries: Vec<GpuInstance> = Vec::new();
        let mut draws: Vec<DrawItem> = Vec::new();
        for (si, list) in by_shape.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            let shape = &self.shapes[si];
            for (bi, body) in shape.bodies.iter().enumerate() {
                if body.index_count == 0 && body.edge_count == 0 {
                    continue;
                }
                let first_instance = entries.len() as u32;
                for &ii in list {
                    let inst = &self.instances[ii as usize];
                    let mut flags = 0;
                    if inst.selected {
                        flags |= FLAG_SELECTED;
                    }
                    if inst.color_override.is_some() {
                        flags |= FLAG_COLOR_OVERRIDE;
                    }
                    if body.double_sided {
                        flags |= FLAG_DOUBLE_SIDED;
                    }
                    entries.push(GpuInstance {
                        model: model_matrix(&inst.world, body.origin, self.render_origin).to_cols_array_2d(),
                        flags,
                        color_override: pack_rgba(inst.color_override.unwrap_or([255, 255, 255, 255])),
                        face_base: body.face_base,
                        node_id: inst.node_id,
                        sel_base: sel_base[ii as usize],
                        pick_id: ii + 1,
                        _pad: [0; 2],
                    });
                }
                draws.push(DrawItem {
                    shape: si as u32,
                    body: bi as u32,
                    first_instance,
                    instance_count: list.len() as u32,
                });
            }
        }
        if entries.is_empty() {
            entries.push(GpuInstance::zeroed());
        }

        self.instance_entry_count = entries.len() as u32;
        self.draws = draws;

        let inst_bytes: &[u8] = bytemuck::cast_slice(&entries);
        if (inst_bytes.len() as u64) > self.instance_buffer.size() {
            self.instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("step-render instances"),
                contents: inst_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        } else {
            queue.write_buffer(&self.instance_buffer, 0, inst_bytes);
        }

        let sel_bytes: &[u8] = bytemuck::cast_slice(&sel_words);
        if (sel_bytes.len() as u64) > self.face_sel_buffer.size() {
            self.face_sel_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("step-render face selection"),
                contents: sel_bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            });
        } else {
            queue.write_buffer(&self.face_sel_buffer, 0, sel_bytes);
        }

        self.instance_bind_group =
            self.layouts.instance_bind_group(device, &self.instance_buffer, &self.face_sel_buffer);
    }

    /// Number of `(body, instance)` entries in the instance storage buffer after the last flush.
    pub fn instance_entry_count(&self) -> u32 {
        self.instance_entry_count
    }

    /// Origin the `f32` model / view matrices are expressed relative to.
    pub(crate) fn render_origin(&self) -> DVec3 {
        self.render_origin
    }
    pub(crate) fn instance_bind_group(&self) -> &wgpu::BindGroup {
        &self.instance_bind_group
    }
    pub(crate) fn draws(&self) -> &[DrawItem] {
        &self.draws
    }
    pub(crate) fn shape_at(&self, i: u32) -> &GpuShape {
        &self.shapes[i as usize]
    }
    /// Resolve a raw pick (instance id + 1, shape-global face index) to logical scene data.
    pub(crate) fn resolve_pick(&self, pick_id: u32, face_index: u32) -> Option<(InstanceHandle, u32, FaceRef)> {
        let idx = pick_id.checked_sub(1)?;
        let inst = self.instances.get(idx as usize)?;
        let face = self.face_ref(inst.shape, face_index)?;
        Some((InstanceHandle(idx), inst.node_id, face))
    }
}

/// `translate(-render_origin) * world * translate(body_origin)`, narrowed to `f32`.
fn model_matrix(world: &DAffine3, body_origin: DVec3, render_origin: DVec3) -> Mat4 {
    let m = *world * DAffine3::from_translation(body_origin);
    let t = m.translation - render_origin;
    Mat4::from_cols(
        m.matrix3.x_axis.as_vec3().extend(0.0),
        m.matrix3.y_axis.as_vec3().extend(0.0),
        m.matrix3.z_axis.as_vec3().extend(0.0),
        Vec4::new(t.x as f32, t.y as f32, t.z as f32, 1.0),
    )
}

fn pack_rgba(c: [u8; 4]) -> u32 {
    u32::from_le_bytes(c)
}

/// Index buffers must be a multiple of `COPY_BUFFER_ALIGNMENT` (4) — `u32` indices always are, but
/// an empty slice would create a zero-sized buffer, which wgpu rejects.
fn pad_indices(src: &[u32]) -> Vec<u32> {
    if src.is_empty() { vec![0] } else { src.to_vec() }
}

fn empty_storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
