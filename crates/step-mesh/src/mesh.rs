//! Tessellator output consumed by the renderer, exporters and the cache.

use std::ops::Range;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::topo::{BodyId, FaceId};

/// Axis-aligned bounding box in mm.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: DVec3::splat(f64::INFINITY), max: DVec3::splat(f64::NEG_INFINITY) };

    pub fn is_empty(&self) -> bool {
        !(self.min.x <= self.max.x && self.min.y <= self.max.y && self.min.z <= self.max.z)
    }
    pub fn extend(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&mut self, other: &Aabb) {
        if !other.is_empty() {
            self.min = self.min.min(other.min);
            self.max = self.max.max(other.max);
        }
    }
    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }
    pub fn size(&self) -> DVec3 {
        self.max - self.min
    }
    pub fn diagonal(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.size().length() }
    }
    /// Bounding box of this box's corners after an affine transform.
    pub fn transformed(&self, m: &glam::DAffine3) -> Aabb {
        if self.is_empty() {
            return *self;
        }
        let mut out = Aabb::EMPTY;
        for i in 0..8 {
            let c = DVec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            );
            out.extend(m.transform_point3(c));
        }
        out
    }
}

impl Default for Aabb {
    fn default() -> Self {
        Aabb::EMPTY
    }
}

/// Tessellation controls. `chord == 0.0` means "derive from model size"
/// (`clamp(2.5e-4 · bbox_diagonal, 0.005, 0.5)` mm).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TessParams {
    /// Max chord deviation between the true surface/curve and the mesh, in mm.
    pub chord: f64,
    /// Max angle between consecutive edge segments, degrees.
    pub max_angle_edge_deg: f64,
    /// Max normal turn between neighbouring mesh vertices on a curved surface, degrees.
    pub max_angle_surf_deg: f64,
    /// Emit feature-edge line indices (`BodyMesh::edge_indices`).
    pub feature_edges: bool,
    /// Weld coincident vertices within a body (fewer vertices, enables manifold checks).
    pub weld: bool,
}

impl TessParams {
    pub const PREVIEW: TessParams =
        TessParams { chord: 0.0, max_angle_edge_deg: 15.0, max_angle_surf_deg: 20.0, feature_edges: true, weld: false };
    pub const FINE: TessParams =
        TessParams { chord: 0.0, max_angle_edge_deg: 8.0, max_angle_surf_deg: 10.0, feature_edges: true, weld: false };
    pub const COARSE: TessParams =
        TessParams { chord: 0.0, max_angle_edge_deg: 25.0, max_angle_surf_deg: 30.0, feature_edges: false, weld: false };

    /// Stable bit pattern used in cache keys.
    pub fn key_bits(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for v in [self.chord, self.max_angle_edge_deg, self.max_angle_surf_deg] {
            h ^= v.to_bits();
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= (self.feature_edges as u64) | ((self.weld as u64) << 1);
        h.wrapping_mul(0x100000001b3)
    }
}

impl Default for TessParams {
    fn default() -> Self {
        TessParams::PREVIEW
    }
}

/// Triangles belonging to one B-rep face inside a [`BodyMesh`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaceRange {
    pub face: FaceId,
    /// STEP `#id` of the ADVANCED_FACE.
    pub src: u32,
    /// Range into `BodyMesh::indices` (multiple of 3).
    pub indices: Range<u32>,
    pub color: [u8; 4],
    pub surface_kind: SurfaceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SurfaceKind {
    Plane = 0,
    Cylinder = 1,
    Cone = 2,
    Sphere = 3,
    Torus = 4,
    Extrusion = 5,
    Revolution = 6,
    Nurbs = 7,
    Unsupported = 255,
}

/// Render-ready mesh for one body. Positions are relative to `origin` (subtracted before the f32
/// cast to keep precision on large-coordinate models); the renderer adds it back in its transform.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BodyMesh {
    pub body: BodyId,
    pub name: String,
    pub origin: DVec3,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex index into `face_ranges` (drives per-face color and ID picking).
    pub face_slot: Vec<u32>,
    /// Triangle list.
    pub indices: Vec<u32>,
    pub face_ranges: Vec<FaceRange>,
    /// Line list into `positions` for feature edges (empty if not requested).
    pub edge_indices: Vec<u32>,
    /// True for sheet bodies (render without back-face culling).
    pub double_sided: bool,
    /// World-space (mm) bounds of this body.
    pub bbox: Aabb,
}

impl BodyMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Tessellation of a whole [`crate::ShapeTopology`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShapeMesh {
    pub bodies: Vec<BodyMesh>,
    pub bbox: Aabb,
    pub stats: TessStats,
    pub diags: Vec<Diag>,
}

impl ShapeMesh {
    pub fn triangle_count(&self) -> usize {
        self.bodies.iter().map(BodyMesh::triangle_count).sum()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TessStats {
    pub faces: u32,
    pub faces_ok: u32,
    pub faces_fallback: u32,
    pub faces_skipped: u32,
    pub vertices: u32,
    pub triangles: u32,
    pub edge_ms: f32,
    pub face_ms: f32,
    pub assemble_ms: f32,
    /// Effective chord tolerance in mm after auto-derivation.
    pub chord: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DiagKind {
    /// Consecutive loop edges did not meet within tolerance; a bridging segment was inserted.
    LoopGap,
    /// An edge had to be flipped to close the loop.
    LoopFlippedEdge,
    /// A periodic face without a seam edge got a synthesized cut.
    SeamSynthesized,
    /// Loop winding around a periodic direction was not 0 or ±1.
    UnexpectedWinding,
    /// Constrained Delaunay rejected a constraint (crossing loops); triangulated without it.
    CdtConflict,
    /// Face fell back to a cruder triangulation (level 1 = robust CDT … 4 = convex fan).
    TriangulationFallback,
    /// Newton point inversion did not converge; used the seed.
    InverseNoConverge,
    /// Face could not be meshed and was skipped.
    DegenerateFace,
    /// Triangle winding disagreed with the analytic normal for most of a face and was flipped.
    WindingFlipped,
    /// Surface kind not implemented.
    UnsupportedSurface,
    /// A panic inside face tessellation was caught.
    Panic,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diag {
    pub kind: DiagKind,
    pub face: Option<FaceId>,
    /// STEP `#id` of the face (0 if unknown).
    pub src: u32,
    /// Fallback level or other small payload.
    pub level: u8,
    pub msg: String,
}

// serde for the id newtypes used in output
impl Serialize for FaceId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0)
    }
}
impl<'de> Deserialize<'de> for FaceId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(FaceId)
    }
}
impl Serialize for BodyId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0)
    }
}
impl<'de> Deserialize<'de> for BodyId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u32::deserialize(d).map(BodyId)
    }
}
impl Default for FaceId {
    fn default() -> Self {
        FaceId(u32::MAX)
    }
}
impl Default for BodyId {
    fn default() -> Self {
        BodyId(u32::MAX)
    }
}
