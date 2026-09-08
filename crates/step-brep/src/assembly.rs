//! Product structure → node tree → flat instance list.

use glam::{DAffine3, DMat3, DVec3};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use step_mesh::geom::Frame;
use step_model::geometry::Scale;
use step_model::units::UnitScale;
use step_model::Model;
use step_p21::{EntityId, EntityType};

use crate::diag::{DiagKind, Diagnostics};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ShapeId(pub u32);

/// A geometry-carrying representation, shared by all its instances.
#[derive(Clone, Debug)]
pub struct ShapeRef {
    pub rep: EntityId,
    pub rep_type: EntityType,
    pub units: UnitScale,
    /// Product definition owning this representation (for names / colours).
    pub owner: Option<EntityId>,
    /// Number of body-level items (solids/sheets) found among the rep items.
    pub body_items: u32,
}

impl ShapeRef {
    pub fn scale(&self) -> Scale {
        Scale { len: self.units.length, ang: self.units.angle }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    /// Slash-separated path from the root, unique within the tree.
    pub path: String,
    pub product_definition: Option<EntityId>,
    /// NEXT_ASSEMBLY_USAGE_OCCURRENCE that created this node (None for roots / mapped items).
    pub occurrence: Option<EntityId>,
    pub local: DAffine3,
    pub world: DAffine3,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub shapes: SmallVec<[ShapeId; 1]>,
    pub depth: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub shape: ShapeId,
    pub node: NodeId,
    pub world: DAffine3,
}

#[derive(Clone, Debug, Default)]
pub struct Assembly {
    pub nodes: Vec<Node>,
    pub roots: Vec<NodeId>,
    pub shapes: Vec<ShapeRef>,
    pub instances: Vec<Instance>,
    /// Instances per shape (indices into `instances`).
    pub by_shape: Vec<Vec<u32>>,
    pub product_count: u32,
    pub occurrence_count: u32,
}

impl Assembly {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }
    pub fn shape(&self, id: ShapeId) -> &ShapeRef {
        &self.shapes[id.0 as usize]
    }
    pub fn max_depth(&self) -> u16 {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }
}

/// Rigid transform of a placement frame (mm).
pub fn frame_affine(f: &Frame) -> DAffine3 {
    DAffine3::from_mat3_translation(DMat3::from_cols(f.x, f.y, f.z), f.o)
}

fn is_body_item(model: &Model<'_>, id: EntityId) -> bool {
    use EntityType as T;
    [T::ManifoldSolidBrep, T::BrepWithVoids, T::FacetedBrep, T::ShellBasedSurfaceModel, T::ClosedShell, T::OpenShell, T::AdvancedFace, T::FaceSurface, T::GeometricSet, T::GeometricCurveSet]
        .iter()
        .any(|&t| model.is_a(id, t))
}

struct Builder<'m, 'f> {
    model: &'m Model<'f>,
    diags: Diagnostics,
    units_cache: FxHashMap<EntityId, UnitScale>,
    shape_index: FxHashMap<EntityId, ShapeId>,
    asm: Assembly,
}

impl<'m, 'f> Builder<'m, 'f> {
    fn units_of_rep(&mut self, rep: EntityId) -> UnitScale {
        let ctx = self.model.representation(rep).map(|r| r.context).unwrap_or(rep);
        if let Some(u) = self.units_cache.get(&ctx) {
            return u.clone();
        }
        let u = self.model.unit_scale(ctx);
        self.units_cache.insert(ctx, u.clone());
        u
    }

    fn shape_for_rep(&mut self, rep: EntityId, owner: Option<EntityId>) -> Option<ShapeId> {
        if let Some(&s) = self.shape_index.get(&rep) {
            return Some(s);
        }
        let r = self.model.representation(rep).ok()?;
        let body_items = r.items.iter().filter(|&&i| is_body_item(self.model, i)).count() as u32;
        if body_items == 0 {
            return None;
        }
        let units = self.units_of_rep(rep);
        let id = ShapeId(self.asm.shapes.len() as u32);
        self.asm.shapes.push(ShapeRef { rep, rep_type: self.model.ty(rep), units, owner, body_items });
        self.asm.by_shape.push(Vec::new());
        self.shape_index.insert(rep, id);
        Some(id)
    }

    /// Placement (AXIS2) → affine in mm using the rep's units.
    fn placement_affine(&mut self, axis: EntityId, rep: EntityId) -> Option<DAffine3> {
        let u = self.units_of_rep(rep);
        let s = Scale { len: u.length, ang: u.angle };
        if self.model.is_a(axis, EntityType::CartesianTransformationOperator3d) {
            return self.cto3d_affine(axis, s);
        }
        self.model.placement(axis, s).ok().map(|f| frame_affine(&f))
    }

    /// CARTESIAN_TRANSFORMATION_OPERATOR_3D(name, axis1, axis2, local_origin, scale, axis3)
    fn cto3d_affine(&self, id: EntityId, s: Scale) -> Option<DAffine3> {
        let args = self.model.part(id, EntityType::CartesianTransformationOperator3d).ok()?;
        let dir = |i: usize| args.nth(i).and_then(|a| a.as_ref()).and_then(|d| self.model.direction(d).ok());
        let origin = args.nth(3).and_then(|a| a.as_ref()).and_then(|p| self.model.cartesian_point(p, s).ok()).unwrap_or(DVec3::ZERO);
        let scale = args.nth(4).and_then(|a| a.as_f64()).unwrap_or(1.0);
        let z = dir(5).unwrap_or(DVec3::Z);
        let x = dir(1).unwrap_or_else(|| step_mesh::geom::frame::any_perpendicular(z));
        let y = dir(2).unwrap_or_else(|| z.cross(x));
        Some(DAffine3::from_mat3_translation(DMat3::from_cols(x * scale, y * scale, z * scale), origin))
    }

    fn add_node(&mut self, name: String, parent: Option<NodeId>, local: DAffine3, pd: Option<EntityId>, occurrence: Option<EntityId>) -> NodeId {
        let (world, depth, path) = match parent {
            Some(p) => {
                let pn = &self.asm.nodes[p.0 as usize];
                (pn.world * local, pn.depth + 1, format!("{}/{}", pn.path, name))
            }
            None => (local, 0, name.clone()),
        };
        let id = NodeId(self.asm.nodes.len() as u32);
        self.asm.nodes.push(Node { name, path, product_definition: pd, occurrence, local, world, parent, children: Vec::new(), shapes: SmallVec::new(), depth });
        if let Some(p) = parent {
            self.asm.nodes[p.0 as usize].children.push(id);
        }
        id
    }

    fn attach_shape(&mut self, node: NodeId, shape: ShapeId) {
        let world = self.asm.nodes[node.0 as usize].world;
        let idx = self.asm.instances.len() as u32;
        self.asm.instances.push(Instance { shape, node, world });
        self.asm.by_shape[shape.0 as usize].push(idx);
        self.asm.nodes[node.0 as usize].shapes.push(shape);
    }

    /// Attach the shapes of `rep` (and its MAPPED_ITEM children) to `node`.
    fn attach_rep(&mut self, node: NodeId, rep: EntityId, owner: Option<EntityId>, depth: u32) {
        if depth > 32 {
            return;
        }
        if let Some(s) = self.shape_for_rep(rep, owner) {
            self.attach_shape(node, s);
        }
        // AP214-style mapped items
        let Ok(r) = self.model.representation(rep) else { return };
        for item in r.items {
            if !self.model.is_a(item, EntityType::MappedItem) {
                continue;
            }
            let Ok(mi) = self.model.mapped_item(item) else { continue };
            let Ok(rm) = self.model.representation_map(mi.source) else { continue };
            let target = self.placement_affine(mi.target, rep).unwrap_or(DAffine3::IDENTITY);
            let origin = self.placement_affine(rm.origin, rm.representation).unwrap_or(DAffine3::IDENTITY);
            let local = target * origin.inverse();
            let name = self.model.representation(rm.representation).map(|x| x.name).ok().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("map{}", item.0));
            let child = self.add_node(name, Some(node), local, None, None);
            self.attach_rep(child, rm.representation, owner, depth + 1);
        }
    }
}

/// Build the assembly tree from product structure. Falls back to one root per geometry-carrying
/// representation when the file has no product structure.
pub fn build(model: &Model<'_>, diags_out: &mut Diagnostics) -> Assembly {
    use EntityType as T;
    let mut b = Builder { model, diags: Diagnostics::default(), units_cache: FxHashMap::default(), shape_index: FxHashMap::default(), asm: Assembly::default() };

    // 1. product definitions and names
    let pds: Vec<EntityId> = model.file.by_type(T::ProductDefinition).collect();
    let mut pd_name: FxHashMap<EntityId, String> = FxHashMap::default();
    for &pd in &pds {
        let name = model
            .product_of_definition(pd)
            .and_then(|p| model.product(p))
            .map(|p| if p.name.trim().is_empty() { p.id_str } else { p.name })
            .unwrap_or_else(|_| format!("pd{}", pd.0));
        pd_name.insert(pd, name);
    }
    b.asm.product_count = model.file.by_type(T::Product).count() as u32;

    // 2. representation ownership via SHAPE_DEFINITION_REPRESENTATION
    let mut rep_owner: FxHashMap<EntityId, EntityId> = FxHashMap::default();
    let mut pd_reps: FxHashMap<EntityId, Vec<EntityId>> = FxHashMap::default();
    for sdr in model.file.by_type(T::ShapeDefinitionRepresentation) {
        let Ok((definition, rep)) = model.definition_representation(sdr) else { continue };
        let pd = resolve_definition_to_pd(model, definition);
        let Some(pd) = pd else { continue };
        if !pd_name.contains_key(&pd) {
            continue; // definition was not a product definition (e.g. validation property)
        }
        // follow simple SHAPE_REPRESENTATION_RELATIONSHIPs (no transformation) both ways
        let mut stack = vec![rep];
        let mut seen: FxHashSet<EntityId> = FxHashSet::default();
        while let Some(r) = stack.pop() {
            if !seen.insert(r) {
                continue;
            }
            rep_owner.entry(r).or_insert(pd);
            pd_reps.entry(pd).or_default().push(r);
            // relationships are found lazily below (indexed once)
        }
    }
    // index simple SRRs once
    let mut srr_links: FxHashMap<EntityId, Vec<EntityId>> = FxHashMap::default();
    for srr in model.file.by_type(T::ShapeRepresentationRelationship) {
        if model.is_a(srr, T::RepresentationRelationshipWithTransformation) {
            continue;
        }
        let Ok(rr) = model.rep_relationship(srr) else { continue };
        if rr.transformation.is_some() {
            continue;
        }
        srr_links.entry(rr.rep_1).or_default().push(rr.rep_2);
        srr_links.entry(rr.rep_2).or_default().push(rr.rep_1);
    }
    // propagate ownership across SRR links
    let owned: Vec<(EntityId, EntityId)> = rep_owner.iter().map(|(r, p)| (*r, *p)).collect();
    for (rep, pd) in owned {
        let mut stack = vec![rep];
        let mut seen: FxHashSet<EntityId> = FxHashSet::default();
        while let Some(r) = stack.pop() {
            if !seen.insert(r) {
                continue;
            }
            if let Some(links) = srr_links.get(&r) {
                for &other in links {
                    if let std::collections::hash_map::Entry::Vacant(e) = rep_owner.entry(other) {
                        e.insert(pd);
                        pd_reps.entry(pd).or_default().push(other);
                    }
                    stack.push(other);
                }
            }
        }
    }

    // 3. occurrences
    struct Child {
        pd: EntityId,
        local: DAffine3,
        nauo: EntityId,
    }
    let mut children: FxHashMap<EntityId, Vec<Child>> = FxHashMap::default();
    let mut is_child: FxHashSet<EntityId> = FxHashSet::default();
    let mut nauo_transform: FxHashMap<EntityId, DAffine3> = FxHashMap::default();

    for cdsr in model.file.by_type(T::ContextDependentShapeRepresentation) {
        let Ok((rr_id, pds_id)) = model.cdsr(cdsr) else { continue };
        let Ok(nauo) = model.product_definition_shape_target(pds_id) else { continue };
        let Ok(n) = model.nauo(nauo) else { continue };
        let Ok(rr) = model.rep_relationship(rr_id) else { continue };
        let local = match rr.transformation {
            None => DAffine3::IDENTITY,
            Some(t) => match model.item_defined_transformation(t) {
                Ok(idt) => {
                    // decide which rep is the child's by ownership
                    let o1 = rep_owner.get(&rr.rep_1).copied();
                    let o2 = rep_owner.get(&rr.rep_2).copied();
                    let (child_rep, parent_rep, rep1_is_child) = if o1 == Some(n.child) && o2 != Some(n.child) {
                        (rr.rep_1, rr.rep_2, true)
                    } else if o2 == Some(n.child) && o1 != Some(n.child) {
                        (rr.rep_2, rr.rep_1, false)
                    } else if o2 == Some(n.parent) {
                        (rr.rep_1, rr.rep_2, true)
                    } else if o1 == Some(n.parent) {
                        (rr.rep_2, rr.rep_1, false)
                    } else {
                        b.diags.push(DiagKind::AmbiguousTransform, Some(cdsr), "cannot decide child/parent representation; assuming rep_1 = child");
                        (rr.rep_1, rr.rep_2, true)
                    };
                    // which item lives in the child rep?
                    let child_items: Vec<EntityId> = model.representation(child_rep).map(|r| r.items).unwrap_or_default();
                    let (child_axis, placement) = if child_items.contains(&idt.item_1) {
                        (idt.item_1, idt.item_2)
                    } else if child_items.contains(&idt.item_2) {
                        (idt.item_2, idt.item_1)
                    } else if rep1_is_child {
                        (idt.item_1, idt.item_2)
                    } else {
                        (idt.item_2, idt.item_1)
                    };
                    let p = b.placement_affine(placement, parent_rep).unwrap_or(DAffine3::IDENTITY);
                    let c = b.placement_affine(child_axis, child_rep).unwrap_or(DAffine3::IDENTITY);
                    p * c.inverse()
                }
                Err(_) => {
                    // CARTESIAN_TRANSFORMATION_OPERATOR_3D used directly
                    b.cto3d_affine(t, Scale::MM).unwrap_or(DAffine3::IDENTITY)
                }
            },
        };
        nauo_transform.insert(nauo, local);
    }
    let mut nauo_count = 0;
    for nauo in model.file.by_type(T::NextAssemblyUsageOccurrence) {
        let Ok(n) = model.nauo(nauo) else { continue };
        nauo_count += 1;
        let local = nauo_transform.get(&nauo).copied().unwrap_or(DAffine3::IDENTITY);
        children.entry(n.parent).or_default().push(Child { pd: n.child, local, nauo });
        is_child.insert(n.child);
    }
    b.asm.occurrence_count = nauo_count;

    // 4. roots and DFS
    let mut roots: Vec<EntityId> = pds
        .iter()
        .copied()
        .filter(|pd| !is_child.contains(pd) && (children.contains_key(pd) || pd_reps.contains_key(pd)))
        .collect();
    roots.sort_unstable();
    if roots.is_empty() {
        b.diags.push(DiagKind::NoProductStructure, None, "no product structure; using every geometry representation as a root");
        let mut reps: Vec<EntityId> = model.file.ids().filter(|&id| model.is_shape_representation(id)).collect();
        reps.sort_unstable();
        for rep in reps {
            let name = model.representation(rep).map(|r| r.name).unwrap_or_default();
            let name = if name.is_empty() { format!("rep{}", rep.0) } else { name };
            if b.shape_for_rep(rep, None).is_none() {
                continue;
            }
            let node = b.add_node(name, None, DAffine3::IDENTITY, None, None);
            b.attach_rep(node, rep, None, 0);
            b.asm.roots.push(node);
        }
    } else {
        // iterative DFS with a visited-on-path guard against cycles
        struct Frame_ {
            pd: EntityId,
            parent: Option<NodeId>,
            local: DAffine3,
            nauo: Option<EntityId>,
            depth: u32,
        }
        let mut stack: Vec<Frame_> = roots.iter().rev().map(|&pd| Frame_ { pd, parent: None, local: DAffine3::IDENTITY, nauo: None, depth: 0 }).collect();
        while let Some(fr) = stack.pop() {
            if fr.depth > 64 {
                b.diags.push(DiagKind::MalformedEntity, Some(fr.pd), "assembly nesting deeper than 64; cycle?");
                continue;
            }
            let base = pd_name.get(&fr.pd).cloned().unwrap_or_else(|| format!("pd{}", fr.pd.0));
            let mut name = base.clone();
            if let Some(nauo) = fr.nauo {
                let names = model.nauo_names(nauo);
                if !names.reference_designator.trim().is_empty() {
                    name = format!("{} [{}]", base, names.reference_designator.trim());
                }
            }
            // disambiguate siblings
            if let Some(p) = fr.parent {
                let siblings = &b.asm.nodes[p.0 as usize].children;
                let dup = siblings.iter().filter(|&&c| b.asm.nodes[c.0 as usize].name == name || b.asm.nodes[c.0 as usize].name.starts_with(&format!("{name}:"))).count();
                if dup > 0 {
                    name = format!("{name}:{}", dup + 1);
                }
            }
            let node = b.add_node(name, fr.parent, fr.local, Some(fr.pd), fr.nauo);
            if fr.parent.is_none() {
                b.asm.roots.push(node);
            }
            if let Some(reps) = pd_reps.get(&fr.pd) {
                let mut reps = reps.clone();
                reps.sort_unstable();
                reps.dedup();
                for rep in reps {
                    b.attach_rep(node, rep, Some(fr.pd), 0);
                }
            }
            if let Some(ch) = children.get(&fr.pd) {
                for c in ch.iter().rev() {
                    stack.push(Frame_ { pd: c.pd, parent: Some(node), local: c.local, nauo: Some(c.nauo), depth: fr.depth + 1 });
                }
            }
        }
    }

    diags_out.merge(std::mem::take(&mut b.diags));
    b.asm
}

/// PRODUCT_DEFINITION_SHAPE / SHAPE_ASPECT / PRODUCT_DEFINITION → the product definition, if any.
fn resolve_definition_to_pd(model: &Model<'_>, definition: EntityId) -> Option<EntityId> {
    use EntityType as T;
    if model.is_a(definition, T::ProductDefinition) {
        return Some(definition);
    }
    if model.is_a(definition, T::ProductDefinitionShape) {
        let target = model.product_definition_shape_target(definition).ok()?;
        return if model.is_a(target, T::ProductDefinition) { Some(target) } else { None };
    }
    if model.is_a(definition, T::ShapeAspect) {
        let pds = model.shape_aspect_of_shape(definition).ok()?;
        let target = model.product_definition_shape_target(pds).ok()?;
        return if model.is_a(target, T::ProductDefinition) { Some(target) } else { None };
    }
    None
}
