//! Points, directions, placements, curves and surfaces → `step_mesh::geom` values in mm.

use glam::DVec3;
use step_mesh::geom::{Curve3, Frame, NurbsCurve, NurbsSurface, Surface};
use step_p21::{Arg, Args, EntityId, EntityType};

use crate::{Model, Result, attr, int_attr, list_attr, opt_ref_attr, real_attr, ref_attr};

/// Geometry decode context: scale factors of the owning representation context.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    /// mm per length unit.
    pub len: f64,
    /// radians per angle unit.
    pub ang: f64,
}

impl Scale {
    pub const MM: Scale = Scale { len: 1.0, ang: 1.0 };
}

impl<'f> Model<'f> {
    /// `CARTESIAN_POINT(name, (x, y[, z]))` in mm.
    pub fn cartesian_point(&self, id: EntityId, s: Scale) -> Result<DVec3> {
        let args = self.part(id, EntityType::CartesianPoint)?;
        let c = list_attr(id, args, 1)?.f64x3()?;
        Ok(DVec3::from_array(c) * s.len)
    }

    /// `DIRECTION(name, (x, y[, z]))`, normalised (zero vectors are returned as-is).
    pub fn direction(&self, id: EntityId) -> Result<DVec3> {
        let args = self.part(id, EntityType::Direction)?;
        let c = list_attr(id, args, 1)?.f64x3()?;
        let d = DVec3::from_array(c);
        Ok(d.try_normalize().unwrap_or(d))
    }

    /// `VECTOR(name, orientation, magnitude)` → direction × magnitude, in mm.
    pub fn vector(&self, id: EntityId, s: Scale) -> Result<DVec3> {
        let args = self.part(id, EntityType::Vector)?;
        let d = self.direction(ref_attr(id, args, 1)?)?;
        let m = real_attr(id, args, 2)?;
        Ok(d * m * s.len)
    }

    /// `AXIS2_PLACEMENT_3D(name, location, axis?, ref_direction?)` (also accepts 2D and AXIS1).
    pub fn placement(&self, id: EntityId, s: Scale) -> Result<Frame> {
        let (ty, args) = self.any_part(id, &[EntityType::Axis2Placement3d, EntityType::Axis2Placement2d, EntityType::Axis1Placement])?;
        let o = self.cartesian_point(ref_attr(id, args, 1)?, s)?;
        match ty {
            EntityType::Axis2Placement3d => {
                let axis = opt_ref_attr(id, args, 2)?.map(|d| self.direction(d)).transpose()?;
                let refd = opt_ref_attr(id, args, 3)?.map(|d| self.direction(d)).transpose()?;
                Ok(Frame::new(o, axis, refd))
            }
            EntityType::Axis2Placement2d => {
                let refd = opt_ref_attr(id, args, 2)?.map(|d| self.direction(d)).transpose()?;
                Ok(Frame::new(o, Some(DVec3::Z), refd))
            }
            _ => {
                // AXIS1_PLACEMENT(name, location, axis?)
                let axis = opt_ref_attr(id, args, 2)?.map(|d| self.direction(d)).transpose()?;
                Ok(Frame::new(o, axis, None))
            }
        }
    }

    // ----- curves -----

    /// Decode any supported 3D curve. Trimmed/surface/seam curves resolve to their basis curve.
    pub fn curve(&self, id: EntityId, s: Scale) -> Result<Curve3> {
        self.curve_depth(id, s, 0)
    }

    fn curve_depth(&self, id: EntityId, s: Scale, depth: u32) -> Result<Curve3> {
        use EntityType as T;
        if depth > 8 {
            return Err(self.invalid(id, "curve reference cycle"));
        }
        let rec = self.file.get(id).ok_or(step_p21::Error::Missing(id))?;
        if rec.is_complex() {
            // rational / uniform / bezier B-spline curve variants
            if let Ok(base) = self.part(id, T::BSplineCurve) {
                return self.bspline_curve_complex(id, base, s);
            }
            return Err(self.unsupported(id, "curve"));
        }
        let args = self.file.args(id).expect("present");
        match rec.ty {
            T::Line => {
                let p = self.cartesian_point(ref_attr(id, args, 1)?, s)?;
                let v = self.vector(ref_attr(id, args, 2)?, s)?;
                let d = v.try_normalize().ok_or_else(|| self.invalid(id, "zero-length line direction"))?;
                Ok(Curve3::Line { p, d })
            }
            T::Circle => {
                let f = self.placement(ref_attr(id, args, 1)?, s)?;
                let r = real_attr(id, args, 2)? * s.len;
                Ok(Curve3::Circle { f, r })
            }
            T::Ellipse => {
                let f = self.placement(ref_attr(id, args, 1)?, s)?;
                let a = real_attr(id, args, 2)? * s.len;
                let b = real_attr(id, args, 3)? * s.len;
                Ok(Curve3::Ellipse { f, a, b })
            }
            T::Polyline => {
                let pts = list_attr(id, args, 1)?.refs().map(|p| self.cartesian_point(p, s)).collect::<Result<Vec<_>>>()?;
                Ok(Curve3::Polyline(pts))
            }
            T::BSplineCurveWithKnots => {
                // (name, degree, ctrl, form, closed, self_intersect, mults, knots, spec)
                let degree = int_attr(id, args, 1)?.max(1) as usize;
                let ctrl = self.point_list(id, args, 2, s)?;
                let mults = list_attr(id, args, 6)?.ints()?;
                let knots = list_attr(id, args, 7)?.reals()?;
                Ok(Curve3::Nurbs(Box::new(NurbsCurve::from_step(degree, &ctrl, None, &mults, &knots))))
            }
            T::BezierCurve | T::UniformCurve | T::QuasiUniformCurve => {
                let degree = int_attr(id, args, 1)?.max(1) as usize;
                let ctrl = self.point_list(id, args, 2, s)?;
                let (mults, knots) = implied_knots(rec.ty, degree, ctrl.len());
                Ok(Curve3::Nurbs(Box::new(NurbsCurve::from_step(degree, &ctrl, None, &mults, &knots))))
            }
            T::TrimmedCurve => self.curve_depth(ref_attr(id, args, 1)?, s, depth + 1),
            T::SurfaceCurve | T::SeamCurve | T::IntersectionCurve => self.curve_depth(ref_attr(id, args, 1)?, s, depth + 1),
            T::CompositeCurve => {
                // Approximate: concatenate sampled segments is left to the tessellator; here we only
                // support composite curves whose segments are all polylines/lines by flattening.
                let mut pts: Vec<DVec3> = Vec::new();
                for seg in list_attr(id, args, 1)?.refs() {
                    let sargs = self.part(seg, T::CompositeCurveSegment)?;
                    let same_sense = crate::bool_attr(seg, sargs, 1)?;
                    let parent = ref_attr(seg, sargs, 2)?;
                    let c = self.curve_depth(parent, s, depth + 1)?;
                    let mut seg_pts = sample_curve_coarse(&c);
                    if !same_sense {
                        seg_pts.reverse();
                    }
                    if let (Some(last), Some(first)) = (pts.last(), seg_pts.first())
                        && last.distance(*first) < 1e-9 {
                            seg_pts.remove(0);
                        }
                    pts.extend(seg_pts);
                }
                if pts.len() < 2 {
                    return Err(self.invalid(id, "empty composite curve"));
                }
                Ok(Curve3::Polyline(pts))
            }
            _ => Err(self.unsupported(id, "curve")),
        }
    }

    fn bspline_curve_complex(&self, id: EntityId, base: Args<'f>, s: Scale) -> Result<Curve3> {
        use EntityType as T;
        // B_SPLINE_CURVE(degree, ctrl, form, closed, self_intersect)
        let degree = int_attr(id, base, 0)?.max(1) as usize;
        let ctrl = self.point_list(id, base, 1, s)?;
        let weights = self
            .file
            .complex_part(id, T::RationalBSplineCurve)
            .and_then(|w| list_attr(id, w, 0).ok())
            .and_then(|l| l.reals().ok());
        let (mults, knots) = if let Some(k) = self.file.complex_part(id, T::BSplineCurveWithKnots) {
            (
                list_attr(id, k, 0)?.ints()?,
                list_attr(id, k, 1)?.reals()?,
            )
        } else {
            let kind = [T::BezierCurve, T::UniformCurve, T::QuasiUniformCurve]
                .into_iter()
                .find(|&t| self.file.complex_part(id, t).is_some())
                .unwrap_or(T::QuasiUniformCurve);
            implied_knots(kind, degree, ctrl.len())
        };
        Ok(Curve3::Nurbs(Box::new(NurbsCurve::from_step(degree, &ctrl, weights.as_deref(), &mults, &knots))))
    }

    fn point_list(&self, id: EntityId, args: Args<'f>, index: usize, s: Scale) -> Result<Vec<DVec3>> {
        list_attr(id, args, index)?.refs().map(|p| self.cartesian_point(p, s)).collect()
    }

    fn point_rows(&self, id: EntityId, args: Args<'f>, index: usize, s: Scale) -> Result<Vec<Vec<DVec3>>> {
        let mut rows = Vec::new();
        for row in list_attr(id, args, index)?.iter() {
            let Arg::List(l) = row else { return Err(self.invalid(id, "control net row is not a list")) };
            rows.push(l.refs().map(|p| self.cartesian_point(p, s)).collect::<Result<Vec<_>>>()?);
        }
        Ok(rows)
    }

    fn real_rows(&self, id: EntityId, args: Args<'f>, index: usize) -> Result<Vec<Vec<f64>>> {
        let mut rows = Vec::new();
        for row in list_attr(id, args, index)?.iter() {
            let Arg::List(l) = row else { return Err(self.invalid(id, "weight row is not a list")) };
            rows.push(l.reals()?);
        }
        Ok(rows)
    }

    // ----- surfaces -----

    /// Decode a surface. Unknown kinds return `Surface::Unsupported(name)` (not an error) so the
    /// face can be counted and skipped; malformed data is an error.
    pub fn surface(&self, id: EntityId, s: Scale) -> Result<Surface> {
        self.surface_depth(id, s, 0)
    }

    fn surface_depth(&self, id: EntityId, s: Scale, depth: u32) -> Result<Surface> {
        use EntityType as T;
        if depth > 8 {
            return Err(self.invalid(id, "surface reference cycle"));
        }
        let rec = self.file.get(id).ok_or(step_p21::Error::Missing(id))?;
        if rec.is_complex() {
            if let Ok(base) = self.part(id, T::BSplineSurface) {
                return self.bspline_surface_complex(id, base, s);
            }
            return Ok(Surface::Unsupported(self.type_name(id)));
        }
        let args = self.file.args(id).expect("present");
        Ok(match rec.ty {
            T::Plane => Surface::Plane { f: self.placement(ref_attr(id, args, 1)?, s)? },
            T::CylindricalSurface => {
                Surface::Cylinder { f: self.placement(ref_attr(id, args, 1)?, s)?, r: real_attr(id, args, 2)? * s.len }
            }
            T::ConicalSurface => Surface::Cone {
                f: self.placement(ref_attr(id, args, 1)?, s)?,
                r: real_attr(id, args, 2)? * s.len,
                alpha: real_attr(id, args, 3)? * s.ang,
            },
            T::SphericalSurface => {
                Surface::Sphere { f: self.placement(ref_attr(id, args, 1)?, s)?, r: real_attr(id, args, 2)? * s.len }
            }
            T::ToroidalSurface | T::DegenerateToroidalSurface => Surface::Torus {
                f: self.placement(ref_attr(id, args, 1)?, s)?,
                big_r: real_attr(id, args, 2)? * s.len,
                r: real_attr(id, args, 3)? * s.len,
            },
            T::SurfaceOfLinearExtrusion => Surface::Extrusion {
                profile: self.curve(ref_attr(id, args, 1)?, s)?,
                dir: self.vector(ref_attr(id, args, 2)?, s)?,
            },
            T::SurfaceOfRevolution => Surface::Revolution {
                profile: self.curve(ref_attr(id, args, 1)?, s)?,
                axis: self.placement(ref_attr(id, args, 2)?, s)?,
            },
            T::BSplineSurfaceWithKnots => {
                // (name, udeg, vdeg, rows, form, uclosed, vclosed, selfint, umults, vmults, uknots, vknots, spec)
                let ud = int_attr(id, args, 1)?.max(1) as usize;
                let vd = int_attr(id, args, 2)?.max(1) as usize;
                let rows = self.point_rows(id, args, 3, s)?;
                let um = list_attr(id, args, 8)?.ints()?;
                let vm = list_attr(id, args, 9)?.ints()?;
                let uk = list_attr(id, args, 10)?.reals()?;
                let vk = list_attr(id, args, 11)?.reals()?;
                Surface::Nurbs(Box::new(NurbsSurface::from_step([ud, vd], &rows, None, &um, &uk, &vm, &vk)))
            }
            T::BezierSurface | T::UniformSurface | T::QuasiUniformSurface => {
                let ud = int_attr(id, args, 1)?.max(1) as usize;
                let vd = int_attr(id, args, 2)?.max(1) as usize;
                let rows = self.point_rows(id, args, 3, s)?;
                let nu = rows.len();
                let nv = rows.first().map_or(0, Vec::len);
                let kind = match rec.ty {
                    T::BezierSurface => T::BezierCurve,
                    T::UniformSurface => T::UniformCurve,
                    _ => T::QuasiUniformCurve,
                };
                let (um, uk) = implied_knots(kind, ud, nu);
                let (vm, vk) = implied_knots(kind, vd, nv);
                Surface::Nurbs(Box::new(NurbsSurface::from_step([ud, vd], &rows, None, &um, &uk, &vm, &vk)))
            }
            T::RectangularTrimmedSurface | T::CurveBoundedSurface => {
                return self.surface_depth(ref_attr(id, args, 1)?, s, depth + 1);
            }
            _ => Surface::Unsupported(rec.ty.name().to_string()),
        })
    }

    fn bspline_surface_complex(&self, id: EntityId, base: Args<'f>, s: Scale) -> Result<Surface> {
        use EntityType as T;
        // B_SPLINE_SURFACE(udeg, vdeg, rows, form, uclosed, vclosed, selfint)
        let ud = int_attr(id, base, 0)?.max(1) as usize;
        let vd = int_attr(id, base, 1)?.max(1) as usize;
        let rows = self.point_rows(id, base, 2, s)?;
        let weights = match self.file.complex_part(id, T::RationalBSplineSurface) {
            Some(w) => Some(self.real_rows(id, w, 0)?),
            None => None,
        };
        let (um, vm, uk, vk) = if let Some(k) = self.file.complex_part(id, T::BSplineSurfaceWithKnots) {
            (
                list_attr(id, k, 0)?.ints()?,
                list_attr(id, k, 1)?.ints()?,
                list_attr(id, k, 2)?.reals()?,
                list_attr(id, k, 3)?.reals()?,
            )
        } else {
            let kind = if self.file.complex_part(id, T::BezierSurface).is_some() {
                T::BezierCurve
            } else if self.file.complex_part(id, T::UniformSurface).is_some() {
                T::UniformCurve
            } else {
                T::QuasiUniformCurve
            };
            let nu = rows.len();
            let nv = rows.first().map_or(0, Vec::len);
            let (um, uk) = implied_knots(kind, ud, nu);
            let (vm, vk) = implied_knots(kind, vd, nv);
            (um, vm, uk, vk)
        };
        Ok(Surface::Nurbs(Box::new(NurbsSurface::from_step([ud, vd], &rows, weights.as_deref(), &um, &uk, &vm, &vk))))
    }
}

/// Knot multiplicities and values implied by BEZIER / UNIFORM / QUASI_UNIFORM forms.
pub fn implied_knots(kind: EntityType, degree: usize, n_ctrl: usize) -> (Vec<i64>, Vec<f64>) {
    let p = degree.max(1);
    let n = n_ctrl.max(p + 1);
    match kind {
        EntityType::UniformCurve => {
            // unclamped: n + p + 1 distinct knots
            let m = n + p + 1;
            ((0..m).map(|_| 1).collect(), (0..m).map(|i| i as f64).collect())
        }
        EntityType::BezierCurve => {
            // segments of degree p sharing endpoints: interior multiplicity p
            let segs = ((n - 1) / p).max(1);
            let mut mults = vec![(p + 1) as i64];
            for _ in 1..segs {
                mults.push(p as i64);
            }
            mults.push((p + 1) as i64);
            let knots = (0..=segs).map(|i| i as f64).collect();
            (mults, knots)
        }
        _ => {
            // quasi-uniform: clamped ends, interior single knots
            let interior = n - p - 1;
            let mut mults = vec![(p + 1) as i64];
            mults.extend(std::iter::repeat_n(1, interior));
            mults.push((p + 1) as i64);
            let knots = (0..=(interior + 1)).map(|i| i as f64).collect();
            (mults, knots)
        }
    }
}

/// Coarse sampling used only to flatten composite curves.
fn sample_curve_coarse(c: &Curve3) -> Vec<DVec3> {
    match c {
        Curve3::Polyline(p) => p.clone(),
        Curve3::Line { p, d } => vec![*p, *p + *d],
        Curve3::Circle { .. } | Curve3::Ellipse { .. } => (0..=32).map(|i| c.point(i as f64 / 32.0 * std::f64::consts::TAU)).collect(),
        Curve3::Nurbs(n) => {
            let (a, b) = n.domain();
            (0..=32).map(|i| n.point(a + (b - a) * i as f64 / 32.0)).collect()
        }
    }
}

#[allow(dead_code)]
fn _assert_attr_used(id: EntityId, args: Args<'_>) -> Result<Arg<'_>> {
    attr(id, args, 0)
}
