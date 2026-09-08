use glam::DVec3;

use super::{Frame, NurbsCurve};

/// 3D curve as referenced by a STEP `EDGE_CURVE`. Parameter conventions follow ISO 10303-42.
#[derive(Clone, Debug)]
pub enum Curve3 {
    /// `p + t·d`, `d` unit length (the STEP `VECTOR` magnitude is folded away; `t` is in mm).
    Line { p: DVec3, d: DVec3 },
    /// `f.o + r(cos t · f.x + sin t · f.y)`, `t` in radians.
    Circle { f: Frame, r: f64 },
    /// `f.o + a cos t · f.x + b sin t · f.y`.
    Ellipse { f: Frame, a: f64, b: f64 },
    /// Possibly rational B-spline curve.
    Nurbs(Box<NurbsCurve>),
    /// Straight segments through the given points (STEP `POLYLINE`).
    Polyline(Vec<DVec3>),
}

impl Curve3 {
    /// Evaluate the curve at parameter `t`.
    pub fn point(&self, t: f64) -> DVec3 {
        match self {
            Curve3::Line { p, d } => *p + *d * t,
            Curve3::Circle { f, r } => f.from_local(DVec3::new(r * t.cos(), r * t.sin(), 0.0)),
            Curve3::Ellipse { f, a, b } => f.from_local(DVec3::new(a * t.cos(), b * t.sin(), 0.0)),
            Curve3::Nurbs(c) => c.point(t),
            Curve3::Polyline(pts) => {
                if pts.len() < 2 {
                    return pts.first().copied().unwrap_or(DVec3::ZERO);
                }
                let n = (pts.len() - 1) as f64;
                let s = t.clamp(0.0, n);
                let i = (s.floor() as usize).min(pts.len() - 2);
                pts[i].lerp(pts[i + 1], s - i as f64)
            }
        }
    }

    /// First derivative dP/dt.
    pub fn tangent(&self, t: f64) -> DVec3 {
        match self {
            Curve3::Line { d, .. } => *d,
            Curve3::Circle { f, r } => f.dir_from_local(DVec3::new(-r * t.sin(), r * t.cos(), 0.0)),
            Curve3::Ellipse { f, a, b } => f.dir_from_local(DVec3::new(-a * t.sin(), b * t.cos(), 0.0)),
            Curve3::Nurbs(c) => c.derivative(t),
            Curve3::Polyline(pts) => {
                if pts.len() < 2 {
                    return DVec3::ZERO;
                }
                let n = pts.len() - 1;
                let i = (t.floor().max(0.0) as usize).min(n - 1);
                pts[i + 1] - pts[i]
            }
        }
    }

    /// Angular period for closed analytic curves (`2π` for circles/ellipses), else `None`.
    pub fn period(&self) -> Option<f64> {
        match self {
            Curve3::Circle { .. } | Curve3::Ellipse { .. } => Some(std::f64::consts::TAU),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Curve3::Line { .. } => "line",
            Curve3::Circle { .. } => "circle",
            Curve3::Ellipse { .. } => "ellipse",
            Curve3::Nurbs(_) => "nurbs",
            Curve3::Polyline(_) => "polyline",
        }
    }
}
