//! Thin topology records (ids only); geometry is decoded separately.

use step_p21::{EntityId, EntityType};

use crate::{Model, Result, bool_attr, ref_attr, ref_list_attr, string_attr};

#[derive(Clone, Debug)]
pub struct EdgeCurveRec {
    pub start: EntityId,
    pub end: EntityId,
    pub curve: EntityId,
    pub same_sense: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct OrientedEdgeRec {
    pub edge: EntityId,
    pub orientation: bool,
}

#[derive(Clone, Debug)]
pub enum LoopRec {
    /// `EDGE_LOOP`: oriented edges.
    Edges(Vec<EntityId>),
    /// `VERTEX_LOOP`: a single vertex (degenerate loop, e.g. cone apex).
    Vertex(EntityId),
    /// `POLY_LOOP`: cartesian points.
    Poly(Vec<EntityId>),
}

#[derive(Clone, Copy, Debug)]
pub struct BoundRec {
    pub loop_: EntityId,
    pub orientation: bool,
    pub outer: bool,
}

#[derive(Clone, Debug)]
pub struct FaceRec {
    pub bounds: Vec<EntityId>,
    pub surface: EntityId,
    pub same_sense: bool,
}

#[derive(Clone, Debug)]
pub struct ShellRec {
    pub faces: Vec<EntityId>,
    pub closed: bool,
    /// For ORIENTED_*_SHELL with orientation `.F.`: faces must be flipped.
    pub reversed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKindRec {
    Solid,
    Sheet,
}

#[derive(Clone, Debug)]
pub struct BodyRec {
    pub kind: BodyKindRec,
    /// Shells: outer first, then voids (for BREP_WITH_VOIDS) or all shells (sheet models).
    pub shells: Vec<EntityId>,
    pub name: String,
}

impl<'f> Model<'f> {
    /// `VERTEX_POINT(name, vertex_geometry)` → the point id.
    pub fn vertex_point(&self, id: EntityId) -> Result<EntityId> {
        let args = self.part(id, EntityType::VertexPoint)?;
        ref_attr(id, args, 1)
    }

    pub fn edge_curve(&self, id: EntityId) -> Result<EdgeCurveRec> {
        let args = self.part(id, EntityType::EdgeCurve)?;
        Ok(EdgeCurveRec {
            start: ref_attr(id, args, 1)?,
            end: ref_attr(id, args, 2)?,
            curve: ref_attr(id, args, 3)?,
            same_sense: bool_attr(id, args, 4)?,
        })
    }

    pub fn oriented_edge(&self, id: EntityId) -> Result<OrientedEdgeRec> {
        let args = self.part(id, EntityType::OrientedEdge)?;
        Ok(OrientedEdgeRec { edge: ref_attr(id, args, 3)?, orientation: bool_attr(id, args, 4)? })
    }

    pub fn loop_(&self, id: EntityId) -> Result<LoopRec> {
        let (ty, args) = self.any_part(id, &[EntityType::EdgeLoop, EntityType::VertexLoop, EntityType::PolyLoop])?;
        Ok(match ty {
            EntityType::EdgeLoop => LoopRec::Edges(ref_list_attr(id, args, 1)?),
            EntityType::VertexLoop => LoopRec::Vertex(ref_attr(id, args, 1)?),
            _ => LoopRec::Poly(ref_list_attr(id, args, 1)?),
        })
    }

    pub fn face_bound(&self, id: EntityId) -> Result<BoundRec> {
        let (ty, args) = self.any_part(id, &[EntityType::FaceOuterBound, EntityType::FaceBound])?;
        Ok(BoundRec { loop_: ref_attr(id, args, 1)?, orientation: bool_attr(id, args, 2)?, outer: ty == EntityType::FaceOuterBound })
    }

    pub fn face(&self, id: EntityId) -> Result<FaceRec> {
        let (_, args) = self.any_part(id, &[EntityType::AdvancedFace, EntityType::FaceSurface])?;
        Ok(FaceRec { bounds: ref_list_attr(id, args, 1)?, surface: ref_attr(id, args, 2)?, same_sense: bool_attr(id, args, 3)? })
    }

    /// Resolve an ORIENTED_FACE to its face and orientation, or return the face itself.
    pub fn resolve_face(&self, id: EntityId) -> Result<(EntityId, bool)> {
        if self.is_a(id, EntityType::OrientedFace) {
            let args = self.part(id, EntityType::OrientedFace)?;
            // ORIENTED_FACE(name, bounds*, face_element, orientation)
            return Ok((ref_attr(id, args, 2)?, bool_attr(id, args, 3)?));
        }
        Ok((id, true))
    }

    pub fn shell(&self, id: EntityId) -> Result<ShellRec> {
        use EntityType as T;
        let (ty, args) = self.any_part(id, &[T::ClosedShell, T::OpenShell, T::OrientedClosedShell, T::OrientedOpenShell, T::ConnectedFaceSet])?;
        match ty {
            T::OrientedClosedShell | T::OrientedOpenShell => {
                // (name, cfs_faces*, shell_element, orientation)
                let inner = ref_attr(id, args, 2)?;
                let orientation = bool_attr(id, args, 3)?;
                let mut s = self.shell(inner)?;
                s.reversed ^= !orientation;
                Ok(s)
            }
            _ => Ok(ShellRec { faces: ref_list_attr(id, args, 1)?, closed: ty == T::ClosedShell, reversed: false }),
        }
    }

    /// Body-level entities found among representation items.
    pub fn body(&self, id: EntityId) -> Result<Option<BodyRec>> {
        use EntityType as T;
        let Ok((ty, args)) = self.any_part(id, &[T::ManifoldSolidBrep, T::BrepWithVoids, T::FacetedBrep, T::ShellBasedSurfaceModel]) else {
            return Ok(None);
        };
        let name = string_attr(id, args, 0).unwrap_or_default();
        Ok(Some(match ty {
            T::ManifoldSolidBrep | T::FacetedBrep => BodyRec { kind: BodyKindRec::Solid, shells: vec![ref_attr(id, args, 1)?], name },
            T::BrepWithVoids => {
                let mut shells = vec![ref_attr(id, args, 1)?];
                shells.extend(ref_list_attr(id, args, 2)?);
                BodyRec { kind: BodyKindRec::Solid, shells, name }
            }
            _ => BodyRec { kind: BodyKindRec::Sheet, shells: ref_list_attr(id, args, 1)?, name },
        }))
    }
}
