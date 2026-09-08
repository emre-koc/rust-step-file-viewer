use glam::DVec3;

use super::{Curve3, Frame, NurbsSurface};

/// Surface underlying a STEP `ADVANCED_FACE`. Parametrizations (ISO 10303-42):
///
/// | variant | P(u, v) |
/// |---|---|
/// | Plane | `o + u·x + v·y` |
/// | Cylinder | `o + r(cos u·x + sin u·y) + v·z` |
/// | Cone | `o + (r + v·tan α)(cos u·x + sin u·y) + v·z` |
/// | Sphere | `o + r(cos v cos u·x + cos v sin u·y + sin v·z)` |
/// | Torus | `o + (R + r cos v)(cos u·x + sin u·y) + r sin v·z` |
/// | Extrusion | `profile(u) + v·dir` (`dir` not normalised; `v` in profile-parameter-free units) |
/// | Revolution | `profile(v)` rotated by angle `u` about `axis.z` through `axis.o` |
/// | Nurbs | rational B-spline surface |
#[derive(Clone, Debug)]
pub enum Surface {
    Plane { f: Frame },
    Cylinder { f: Frame, r: f64 },
    /// `alpha` is the semi-angle in radians.
    Cone { f: Frame, r: f64, alpha: f64 },
    Sphere { f: Frame, r: f64 },
    Torus { f: Frame, big_r: f64, r: f64 },
    Extrusion { profile: Curve3, dir: DVec3 },
    Revolution { profile: Curve3, axis: Frame },
    Nurbs(Box<NurbsSurface>),
    /// Surface type the reader does not know; the face is reported and skipped.
    Unsupported(String),
}

impl Surface {
    pub fn kind(&self) -> &'static str {
        match self {
            Surface::Plane { .. } => "plane",
            Surface::Cylinder { .. } => "cylinder",
            Surface::Cone { .. } => "cone",
            Surface::Sphere { .. } => "sphere",
            Surface::Torus { .. } => "torus",
            Surface::Extrusion { .. } => "extrusion",
            Surface::Revolution { .. } => "revolution",
            Surface::Nurbs(_) => "nurbs",
            Surface::Unsupported(_) => "unsupported",
        }
    }

    pub fn is_planar(&self) -> bool {
        matches!(self, Surface::Plane { .. })
    }
}
