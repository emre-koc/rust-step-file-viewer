//! `step-mesh`: geometry kernel and tessellator for stepview.
//!
//! This crate owns the *input* B-rep model ([`topo::ShapeTopology`] built from analytic and NURBS
//! [`geom::Surface`]s / [`geom::Curve3`]s) and the *output* triangle meshes ([`mesh::ShapeMesh`]).
//! `step-brep` fills a `ShapeTopology` from a STEP file; `step-render`/`step-export` consume the
//! `ShapeMesh`. The tessellator itself lives in the private modules and is exposed as [`tessellate`].
//!
//! Conventions: all lengths in millimetres, f64 for geometry, f32 only in GPU-facing output buffers.
//! Surface parametrizations follow ISO 10303-42 (u is the angular parameter on cylinders, cones,
//! spheres and tori). Faces carry `same_sense`; when false the surface normal is flipped.

pub mod geom;
pub mod mesh;
pub mod topo;

pub use mesh::{Aabb, BodyMesh, Diag, DiagKind, FaceRange, ShapeMesh, TessParams, TessStats};
pub use topo::ShapeTopology;

// --- tessellator implementation modules ---
mod assemble; // parallel driver, body concatenation, feature edges
mod discretize; // edge → polyline
mod face; // per-face pipeline and fast paths
mod param; // surface eval / inverse / normals
mod seam; // periodic unwrap, synthesized cuts, poles
mod triangulate; // uv scaling, Steiner grid, CDT + fallbacks

#[cfg(test)]
mod tests;

/// Tessellate a shape into render-ready meshes. Never panics on bad geometry: every per-face failure
/// becomes a [`Diag`] and the face is degraded or skipped.
pub fn tessellate(topo: &ShapeTopology, params: &TessParams) -> ShapeMesh {
    assemble::tessellate(topo, params)
}
