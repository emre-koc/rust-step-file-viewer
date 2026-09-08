use glam::{DVec3, DMat3};

/// Right-handed orthonormal placement built from a STEP `AXIS2_PLACEMENT_3D`.
///
/// `z` is the placement axis, `x` the (re-orthogonalised) reference direction, `y = z × x`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub o: DVec3,
    pub x: DVec3,
    pub y: DVec3,
    pub z: DVec3,
}

impl Frame {
    pub const IDENTITY: Frame = Frame { o: DVec3::ZERO, x: DVec3::X, y: DVec3::Y, z: DVec3::Z };

    /// Build a frame robustly. `axis` and `ref_dir` may be missing, zero-length, or (in real files)
    /// parallel to each other; in every such case a valid frame is still produced.
    pub fn new(origin: DVec3, axis: Option<DVec3>, ref_dir: Option<DVec3>) -> Frame {
        let z = axis
            .and_then(|a| a.try_normalize())
            .unwrap_or(DVec3::Z);
        let x = ref_dir
            .map(|r| r - z * r.dot(z))
            .and_then(|r| r.try_normalize())
            .unwrap_or_else(|| any_perpendicular(z));
        let y = z.cross(x);
        Frame { o: origin, x, y, z }
    }

    #[inline]
    pub fn to_local(&self, p: DVec3) -> DVec3 {
        let d = p - self.o;
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    #[inline]
    pub fn from_local(&self, l: DVec3) -> DVec3 {
        self.o + self.x * l.x + self.y * l.y + self.z * l.z
    }

    /// Rotate a direction into local coordinates (no translation).
    #[inline]
    pub fn dir_to_local(&self, d: DVec3) -> DVec3 {
        DVec3::new(d.dot(self.x), d.dot(self.y), d.dot(self.z))
    }

    /// Rotate a local direction into world coordinates.
    #[inline]
    pub fn dir_from_local(&self, l: DVec3) -> DVec3 {
        self.x * l.x + self.y * l.y + self.z * l.z
    }

    /// Rotation part as a matrix with columns `x, y, z`.
    pub fn rotation(&self) -> DMat3 {
        DMat3::from_cols(self.x, self.y, self.z)
    }
}

/// A unit vector perpendicular to `z` (assumed unit length).
pub fn any_perpendicular(z: DVec3) -> DVec3 {
    let candidate = if z.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    (candidate - z * candidate.dot(z)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_axis_and_ref_still_yields_orthonormal_frame() {
        let f = Frame::new(DVec3::ZERO, Some(DVec3::Z), Some(DVec3::Z));
        assert!(f.x.dot(f.z).abs() < 1e-12);
        assert!((f.x.length() - 1.0).abs() < 1e-12);
        assert!((f.y - f.z.cross(f.x)).length() < 1e-12);
    }

    #[test]
    fn zero_axis_falls_back() {
        let f = Frame::new(DVec3::ZERO, Some(DVec3::ZERO), None);
        assert_eq!(f.z, DVec3::Z);
    }
}
