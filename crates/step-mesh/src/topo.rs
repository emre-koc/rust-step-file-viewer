//! Input B-rep topology consumed by the tessellator. Filled by `step-brep` from a STEP file.
//!
//! **Sharing contract:** there is exactly one [`Edge`] per STEP `EDGE_CURVE` and one [`Vertex`] per
//! `VERTEX_POINT`. Both faces adjacent to an edge reference the same [`EdgeId`] (through their own
//! [`OrientedEdge`]), so the tessellator can discretise each edge once and both faces get identical
//! boundary polylines — the mesh is crack-free by construction.

use glam::DVec3;

use crate::geom::{Curve3, Surface};
use crate::mesh::Aabb;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
        impl $name {
            #[inline]
            pub fn idx(self) -> usize {
                self.0 as usize
            }
        }
    };
}
id_type!(VertexId);
id_type!(EdgeId);
id_type!(LoopId);
id_type!(FaceId);
id_type!(ShellId);
id_type!(BodyId);

#[derive(Clone, Debug)]
pub struct Vertex {
    pub p: DVec3,
    /// STEP `#id` of the VERTEX_POINT (0 if synthetic).
    pub src: u32,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub curve: Curve3,
    pub v0: VertexId,
    pub v1: VertexId,
    /// `EDGE_CURVE.same_sense`: whether the curve parameter increases from `v0` to `v1`.
    pub same_sense: bool,
    pub src: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct OrientedEdge {
    pub edge: EdgeId,
    /// `ORIENTED_EDGE.orientation == .F.`: traverse the edge from `v1` to `v0`.
    pub reversed: bool,
}

#[derive(Clone, Debug)]
pub struct Loop {
    /// Already oriented consistently with the face bound (a `FACE_BOUND` with orientation `.F.`
    /// has been reversed by the extractor: edge order reversed and every `reversed` flag toggled).
    pub edges: Vec<OrientedEdge>,
    /// `FACE_OUTER_BOUND` (true) vs `FACE_BOUND` (false, a hole). When a file marks none as outer,
    /// the tessellator picks the largest ring.
    pub is_outer: bool,
}

#[derive(Clone, Debug)]
pub struct Face {
    pub surface: Surface,
    pub loops: Vec<LoopId>,
    /// `ADVANCED_FACE.same_sense`: surface normal points outward when true.
    pub same_sense: bool,
    /// Resolved RGBA (face > shell > body > default) — always set by the extractor.
    pub color: [u8; 4],
    pub src: u32,
}

#[derive(Clone, Debug)]
pub struct Shell {
    pub faces: Vec<FaceId>,
    /// `CLOSED_SHELL` vs `OPEN_SHELL`.
    pub closed: bool,
    pub src: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// `MANIFOLD_SOLID_BREP` / `BREP_WITH_VOIDS` / `FACETED_BREP`.
    Solid,
    /// `SHELL_BASED_SURFACE_MODEL` and other non-closed geometry; rendered double-sided.
    Sheet,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub shells: Vec<ShellId>,
    pub kind: BodyKind,
    pub name: String,
    pub src: u32,
}

/// One shape representation's worth of B-rep, in millimetres.
#[derive(Clone, Debug, Default)]
pub struct ShapeTopology {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub loops: Vec<Loop>,
    pub faces: Vec<Face>,
    pub shells: Vec<Shell>,
    pub bodies: Vec<Body>,
    /// Closure tolerance of the file (`UNCERTAINTY_MEASURE_WITH_UNIT`) converted to mm.
    pub tol: f64,
    /// Name of the owning product (for diagnostics).
    pub name: String,
    /// STEP `#id` of the shape representation this was built from.
    pub src_rep: u32,
}

impl ShapeTopology {
    pub fn vertex(&self, id: VertexId) -> &Vertex {
        &self.vertices[id.idx()]
    }
    pub fn edge(&self, id: EdgeId) -> &Edge {
        &self.edges[id.idx()]
    }
    pub fn loop_(&self, id: LoopId) -> &Loop {
        &self.loops[id.idx()]
    }
    pub fn face(&self, id: FaceId) -> &Face {
        &self.faces[id.idx()]
    }
    pub fn shell(&self, id: ShellId) -> &Shell {
        &self.shells[id.idx()]
    }
    pub fn body(&self, id: BodyId) -> &Body {
        &self.bodies[id.idx()]
    }

    /// Bounding box of all vertex positions (cheap; available before tessellation).
    pub fn vertex_bbox(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for v in &self.vertices {
            b.extend(v.p);
        }
        b
    }

    /// Start and end vertex of an oriented edge, honouring `reversed`.
    pub fn oriented_ends(&self, oe: OrientedEdge) -> (VertexId, VertexId) {
        let e = self.edge(oe.edge);
        if oe.reversed { (e.v1, e.v0) } else { (e.v0, e.v1) }
    }
}
