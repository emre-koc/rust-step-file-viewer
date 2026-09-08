//! Loaded model: structure, meshes and their GPU handles, visibility and selection bookkeeping.

use std::collections::HashSet;
use std::sync::Arc;

use step_brep::{NodeId, ShapeId, Structure};
use step_mesh::{Aabb, ShapeMesh};
use step_p21::StepFile;
use step_render::{InstanceHandle, PickResult, ShapeHandle};

use super::viewport::Viewport;

#[derive(Clone, Debug)]
pub struct Selection {
    pub instance: InstanceHandle,
    pub instance_index: u32,
    pub node: NodeId,
    pub shape: ShapeId,
    pub body: u32,
    pub face_slot: u32,
    pub face_src: u32,
    pub face_index: u32,
    /// World-space hit point (mm).
    pub point: glam::DVec3,
}

pub struct LoadedModel {
    pub file: Arc<StepFile>,
    pub structure: Arc<Structure>,
    pub assembly: Arc<step_brep::Assembly>,
    pub meshes: Vec<Option<Arc<ShapeMesh>>>,
    pub shape_handles: Vec<Option<ShapeHandle>>,
    /// Per assembly instance index.
    pub instance_handles: Vec<Option<InstanceHandle>>,
    /// Instance handle → assembly instance index.
    pub handle_to_instance: Vec<u32>,
    pub hidden: HashSet<NodeId>,
    pub node_bbox: Vec<Aabb>,
    pub triangles: u64,
}

impl LoadedModel {
    pub fn new(file: Arc<StepFile>, structure: Arc<Structure>) -> Self {
        let n_shapes = structure.assembly.shapes.len();
        let n_inst = structure.assembly.instances.len();
        let n_nodes = structure.assembly.nodes.len();
        let assembly = Arc::new(structure.assembly.clone());
        LoadedModel {
            file,
            assembly,
            meshes: vec![None; n_shapes],
            shape_handles: vec![None; n_shapes],
            instance_handles: vec![None; n_inst],
            handle_to_instance: Vec::new(),
            hidden: HashSet::new(),
            node_bbox: vec![Aabb::EMPTY; n_nodes],
            triangles: 0,
            structure,
        }
    }

    pub fn add_shape(&mut self, shape: ShapeId, mesh: Arc<ShapeMesh>, vp: &mut Viewport) {
        let i = shape.0 as usize;
        let handle = vp.upload(&mesh);
        let asm = &self.structure.assembly;
        for &inst in &asm.by_shape[i] {
            let ins = &asm.instances[inst as usize];
            let h = vp.add_instance(handle, ins.world, ins.node.0);
            if (h.0 as usize) >= self.handle_to_instance.len() {
                self.handle_to_instance.resize(h.0 as usize + 1, u32::MAX);
            }
            self.handle_to_instance[h.0 as usize] = inst;
            self.instance_handles[inst as usize] = Some(h);
            self.triangles += mesh.triangle_count() as u64;
            // accumulate node bboxes up the tree
            let world_bbox = mesh.bbox.transformed(&ins.world);
            let mut n = Some(ins.node);
            while let Some(id) = n {
                self.node_bbox[id.0 as usize].union(&world_bbox);
                n = asm.nodes[id.0 as usize].parent;
            }
            if self.hidden_effective(ins.node) {
                vp.set_visible(h, false);
            }
        }
        self.shape_handles[i] = Some(handle);
        self.meshes[i] = Some(mesh);
    }

    pub fn finish(&mut self, _vp: &mut Viewport) {}

    pub fn instanced_triangles(&self) -> u64 {
        self.triangles
    }

    pub fn hidden_effective(&self, node: NodeId) -> bool {
        let asm = &self.structure.assembly;
        let mut n = Some(node);
        while let Some(id) = n {
            if self.hidden.contains(&id) {
                return true;
            }
            n = asm.nodes[id.0 as usize].parent;
        }
        false
    }

    fn refresh_visibility(&self, vp: &mut Viewport) {
        let asm = &self.structure.assembly;
        for (i, h) in self.instance_handles.iter().enumerate() {
            if let Some(h) = h {
                vp.set_visible(*h, !self.hidden_effective(asm.instances[i].node));
            }
        }
    }

    pub fn set_node_hidden(&mut self, node: NodeId, hidden: bool, vp: &mut Viewport) {
        if hidden {
            self.hidden.insert(node);
        } else {
            // unhide: also unhide ancestors so it actually shows
            self.hidden.remove(&node);
            let asm = &self.structure.assembly;
            let mut n = asm.nodes[node.0 as usize].parent;
            while let Some(id) = n {
                self.hidden.remove(&id);
                n = asm.nodes[id.0 as usize].parent;
            }
        }
        self.refresh_visibility(vp);
    }

    pub fn unhide_all(&mut self, vp: &mut Viewport) {
        self.hidden.clear();
        self.refresh_visibility(vp);
    }

    /// Hide everything except `node` (and its ancestors/descendants).
    pub fn isolate_node(&mut self, node: NodeId, vp: &mut Viewport) {
        let asm = &self.structure.assembly;
        let mut keep: HashSet<NodeId> = HashSet::new();
        let mut n = Some(node);
        while let Some(id) = n {
            keep.insert(id);
            n = asm.nodes[id.0 as usize].parent;
        }
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            keep.insert(id);
            stack.extend(asm.nodes[id.0 as usize].children.iter().copied());
        }
        self.hidden.clear();
        for (i, _) in asm.nodes.iter().enumerate() {
            let id = NodeId(i as u32);
            if !keep.contains(&id) {
                // hide only top-most non-kept nodes (children inherit)
                let parent_kept = asm.nodes[i].parent.is_none_or(|p| keep.contains(&p));
                if parent_kept {
                    self.hidden.insert(id);
                }
            }
        }
        self.refresh_visibility(vp);
    }

    /// All assembly instance indices under a node (inclusive).
    pub fn instances_under_node(&self, node: NodeId) -> Vec<u32> {
        let asm = &self.structure.assembly;
        let mut set: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            set.insert(id);
            stack.extend(asm.nodes[id.0 as usize].children.iter().copied());
        }
        asm.instances.iter().enumerate().filter(|(_, i)| set.contains(&i.node)).map(|(k, _)| k as u32).collect()
    }

    pub fn selection_from_pick(&self, pick: &PickResult, point: glam::DVec3) -> Option<Selection> {
        let inst_index = *self.handle_to_instance.get(pick.instance.0 as usize)?;
        let ins = self.structure.assembly.instances.get(inst_index as usize)?;
        Some(Selection {
            instance: pick.instance,
            instance_index: inst_index,
            node: ins.node,
            shape: ins.shape,
            body: pick.body,
            face_slot: pick.face_slot,
            face_src: pick.face_src,
            face_index: pick.face_index,
            point,
        })
    }

    /// Highlight a node (all its instances) or a single face.
    pub fn apply_selection(&self, sel: Option<&Selection>, vp: &mut Viewport) {
        match sel {
            None => vp.set_selected_instances(&[]),
            Some(s) => {
                vp.set_selected_instances(&[s.instance]);
                vp.set_selected_faces(s.instance, &[s.face_index]);
            }
        }
    }

    pub fn select_node(&self, node: NodeId, vp: &mut Viewport) {
        let insts: Vec<InstanceHandle> = self.instances_under_node(node).into_iter().filter_map(|i| self.instance_handles[i as usize]).collect();
        vp.set_selected_instances(&insts);
    }

    /// Face details for the properties panel.
    pub fn face_info(&self, sel: &Selection) -> Option<FaceInfo> {
        let mesh = self.meshes.get(sel.shape.0 as usize)?.as_ref()?;
        let body = mesh.bodies.get(sel.body as usize)?;
        let fr = body.face_ranges.get(sel.face_slot as usize)?;
        // average normal over the face's vertices (used for section alignment)
        let mut n = glam::DVec3::ZERO;
        let mut count = 0;
        let s = fr.indices.start as usize;
        let e = (fr.indices.end as usize).min(body.indices.len());
        for &vi in &body.indices[s..e] {
            if let Some(nn) = body.normals.get(vi as usize) {
                n += glam::DVec3::new(nn[0] as f64, nn[1] as f64, nn[2] as f64);
                count += 1;
            }
        }
        let world = self.structure.assembly.instances[sel.instance_index as usize].world;
        let normal = if count > 0 { (world.matrix3 * (n / count as f64)).normalize_or_zero() } else { glam::DVec3::Z };
        Some(FaceInfo { surface_kind: fr.surface_kind, color: fr.color, triangles: (e - s) as u32 / 3, body_name: body.name.clone(), body_triangles: body.triangle_count() as u32, normal })
    }
}

pub struct FaceInfo {
    pub surface_kind: step_mesh::mesh::SurfaceKind,
    pub color: [u8; 4],
    pub triangles: u32,
    pub body_name: String,
    pub body_triangles: u32,
    pub normal: glam::DVec3,
}
