//! Walk one shape representation into a `ShapeTopology` for the tessellator.

use glam::DVec3;
use rustc_hash::FxHashMap;
use step_mesh::geom::{Curve3, Surface};
use step_mesh::topo::*;
use step_model::topology::{BodyKindRec, LoopRec};
use step_model::Model;
use step_p21::{EntityId, EntityType};

use crate::assembly::ShapeRef;
use crate::diag::{DiagKind, Diagnostics};
use crate::style::StyleIndex;

struct Extractor<'m, 'f> {
    model: &'m Model<'f>,
    scale: step_model::geometry::Scale,
    styles: &'m StyleIndex,
    topo: ShapeTopology,
    vertex_ids: FxHashMap<EntityId, VertexId>,
    edge_ids: FxHashMap<EntityId, EdgeId>,
    diags: Diagnostics,
}

impl<'m, 'f> Extractor<'m, 'f> {
    fn vertex(&mut self, vp: EntityId) -> Option<VertexId> {
        if let Some(&v) = self.vertex_ids.get(&vp) {
            return Some(v);
        }
        let p = match self.model.vertex_point(vp).and_then(|pt| self.model.cartesian_point(pt, self.scale)) {
            Ok(p) => p,
            Err(e) => {
                self.diags.push(DiagKind::DanglingRef, Some(vp), format!("vertex: {e}"));
                return None;
            }
        };
        let id = VertexId(self.topo.vertices.len() as u32);
        self.topo.vertices.push(Vertex { p, src: vp.0 });
        self.vertex_ids.insert(vp, id);
        Some(id)
    }

    fn synthetic_vertex(&mut self, p: DVec3) -> VertexId {
        let id = VertexId(self.topo.vertices.len() as u32);
        self.topo.vertices.push(Vertex { p, src: 0 });
        id
    }

    fn edge(&mut self, ec: EntityId) -> Option<EdgeId> {
        if let Some(&e) = self.edge_ids.get(&ec) {
            return Some(e);
        }
        let rec = match self.model.edge_curve(ec) {
            Ok(r) => r,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(ec), format!("edge: {e}"));
                return None;
            }
        };
        let v0 = self.vertex(rec.start)?;
        let v1 = self.vertex(rec.end)?;
        let curve = match self.model.curve(rec.curve, self.scale) {
            Ok(c) => c,
            Err(e) => {
                let kind = if e.is_unsupported() { DiagKind::UnsupportedCurve } else { DiagKind::MalformedEntity };
                self.diags.push(kind, Some(rec.curve), format!("{e}; using a straight segment"));
                let (a, b) = (self.topo.vertices[v0.idx()].p, self.topo.vertices[v1.idx()].p);
                let d = (b - a).try_normalize().unwrap_or(DVec3::X);
                Curve3::Line { p: a, d }
            }
        };
        let id = EdgeId(self.topo.edges.len() as u32);
        self.topo.edges.push(Edge { curve, v0, v1, same_sense: rec.same_sense, src: ec.0 });
        self.edge_ids.insert(ec, id);
        Some(id)
    }

    /// Build a loop; returns None if it has no usable edges (vertex loops).
    fn loop_(&mut self, loop_id: EntityId, orientation: bool, is_outer: bool) -> Option<LoopId> {
        let rec = match self.model.loop_(loop_id) {
            Ok(r) => r,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(loop_id), format!("loop: {e}"));
                return None;
            }
        };
        let mut edges: Vec<OrientedEdge> = Vec::new();
        match rec {
            LoopRec::Edges(oes) => {
                for oe in oes {
                    let o = match self.model.oriented_edge(oe) {
                        Ok(o) => o,
                        Err(e) => {
                            self.diags.push(DiagKind::MalformedEntity, Some(oe), format!("oriented edge: {e}"));
                            continue;
                        }
                    };
                    if let Some(edge) = self.edge(o.edge) {
                        edges.push(OrientedEdge { edge, reversed: !o.orientation });
                    }
                }
            }
            LoopRec::Vertex(_) => return None,
            LoopRec::Poly(points) => {
                let pts: Vec<DVec3> = points.iter().filter_map(|&p| self.model.cartesian_point(p, self.scale).ok()).collect();
                if pts.len() < 3 {
                    return None;
                }
                let vids: Vec<VertexId> = pts.iter().map(|&p| self.synthetic_vertex(p)).collect();
                for i in 0..pts.len() {
                    let j = (i + 1) % pts.len();
                    let d = (pts[j] - pts[i]).try_normalize().unwrap_or(DVec3::X);
                    let id = EdgeId(self.topo.edges.len() as u32);
                    self.topo.edges.push(Edge { curve: Curve3::Line { p: pts[i], d }, v0: vids[i], v1: vids[j], same_sense: true, src: 0 });
                    edges.push(OrientedEdge { edge: id, reversed: false });
                }
            }
        }
        if edges.is_empty() {
            return None;
        }
        if !orientation {
            edges.reverse();
            for e in &mut edges {
                e.reversed = !e.reversed;
            }
        }
        let id = LoopId(self.topo.loops.len() as u32);
        self.topo.loops.push(Loop { edges, is_outer });
        Some(id)
    }

    fn face(&mut self, face_id: EntityId, flip: bool, shell_id: EntityId, body_id: EntityId) -> Option<FaceId> {
        let (face_id, orient) = match self.model.resolve_face(face_id) {
            Ok(x) => x,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(face_id), format!("face: {e}"));
                return None;
            }
        };
        let rec = match self.model.face(face_id) {
            Ok(r) => r,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(face_id), format!("face: {e}"));
                return None;
            }
        };
        let surface = match self.model.surface(rec.surface, self.scale) {
            Ok(Surface::Unsupported(name)) => {
                self.diags.push(DiagKind::UnsupportedSurface, Some(rec.surface), name.clone());
                Surface::Unsupported(name)
            }
            Ok(s) => s,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(rec.surface), format!("surface: {e}"));
                return None;
            }
        };
        let mut loops = Vec::with_capacity(rec.bounds.len());
        for b in &rec.bounds {
            let bound = match self.model.face_bound(*b) {
                Ok(b) => b,
                Err(e) => {
                    self.diags.push(DiagKind::MalformedEntity, Some(*b), format!("bound: {e}"));
                    continue;
                }
            };
            if let Some(l) = self.loop_(bound.loop_, bound.orientation, bound.outer) {
                loops.push(l);
            }
        }
        // outer bound first
        loops.sort_by_key(|l| !self.topo.loops[l.idx()].is_outer);
        let color = self.styles.resolve(face_id, shell_id, body_id);
        let id = FaceId(self.topo.faces.len() as u32);
        self.topo.faces.push(Face { surface, loops, same_sense: rec.same_sense ^ flip ^ !orient, color, src: face_id.0 });
        Some(id)
    }

    fn shell(&mut self, shell_id: EntityId, body_id: EntityId, flip_all: bool) -> Option<ShellId> {
        let rec = match self.model.shell(shell_id) {
            Ok(r) => r,
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(shell_id), format!("shell: {e}"));
                return None;
            }
        };
        let flip = rec.reversed ^ flip_all;
        let mut faces = Vec::with_capacity(rec.faces.len());
        for f in rec.faces {
            if let Some(fid) = self.face(f, flip, shell_id, body_id) {
                faces.push(fid);
            }
        }
        if faces.is_empty() {
            return None;
        }
        let id = ShellId(self.topo.shells.len() as u32);
        self.topo.shells.push(Shell { faces, closed: rec.closed, src: shell_id.0 });
        Some(id)
    }

    fn body(&mut self, item: EntityId, default_name: &str) {
        let rec = match self.model.body(item) {
            Ok(Some(r)) => r,
            Ok(None) => {
                // a bare shell or face used directly as a representation item
                use EntityType as T;
                if [T::ClosedShell, T::OpenShell, T::OrientedClosedShell, T::OrientedOpenShell].iter().any(|&t| self.model.is_a(item, t)) {
                    if let Some(s) = self.shell(item, item, false) {
                        let closed = self.topo.shells[s.idx()].closed;
                        self.topo.bodies.push(Body { shells: vec![s], kind: if closed { BodyKind::Solid } else { BodyKind::Sheet }, name: default_name.to_string(), src: item.0 });
                    }
                } else if (self.model.is_a(item, T::AdvancedFace) || self.model.is_a(item, T::FaceSurface))
                    && let Some(f) = self.face(item, false, item, item) {
                        let sid = ShellId(self.topo.shells.len() as u32);
                        self.topo.shells.push(Shell { faces: vec![f], closed: false, src: item.0 });
                        self.topo.bodies.push(Body { shells: vec![sid], kind: BodyKind::Sheet, name: default_name.to_string(), src: item.0 });
                    }
                return;
            }
            Err(e) => {
                self.diags.push(DiagKind::MalformedEntity, Some(item), format!("body: {e}"));
                return;
            }
        };
        let mut shells = Vec::new();
        for s in &rec.shells {
            if let Some(sid) = self.shell(*s, item, false) {
                shells.push(sid);
            }
        }
        if shells.is_empty() {
            self.diags.push(DiagKind::EmptyRepresentation, Some(item), "body without usable shells");
            return;
        }
        let kind = match rec.kind {
            BodyKindRec::Solid => BodyKind::Solid,
            BodyKindRec::Sheet => BodyKind::Sheet,
        };
        let name = if rec.name.trim().is_empty() { default_name.to_string() } else { rec.name };
        self.topo.bodies.push(Body { shells, kind, name, src: item.0 });
    }
}

/// Extract the B-rep of one shape representation.
pub fn extract_shape(model: &Model<'_>, shape: &ShapeRef, name: &str, styles: &StyleIndex) -> (ShapeTopology, Diagnostics) {
    let scale = shape.scale();
    let mut ex = Extractor {
        model,
        scale,
        styles,
        topo: ShapeTopology { tol: if shape.units.uncertainty > 0.0 { shape.units.uncertainty } else { 1e-3 }, name: name.to_string(), src_rep: shape.rep.0, ..Default::default() },
        vertex_ids: FxHashMap::default(),
        edge_ids: FxHashMap::default(),
        diags: Diagnostics::default(),
    };
    match model.representation(shape.rep) {
        Ok(rep) => {
            for item in rep.items {
                ex.body(item, name);
            }
        }
        Err(e) => ex.diags.push(DiagKind::MalformedEntity, Some(shape.rep), format!("representation: {e}")),
    }
    if ex.topo.bodies.is_empty() {
        ex.diags.push(DiagKind::EmptyRepresentation, Some(shape.rep), "no bodies extracted");
    }
    (ex.topo, ex.diags)
}
