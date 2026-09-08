//! Unit resolution per representation context. Everything is converted to mm and radians.

use step_p21::{Arg, EntityId, EntityType};

use crate::{Model, attr, list_attr, ref_attr};

/// Scale factors for one `GEOMETRIC_REPRESENTATION_CONTEXT`.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitScale {
    /// Millimetres per file length unit.
    pub length: f64,
    /// Radians per file plane-angle unit.
    pub angle: f64,
    /// Closure tolerance in mm (0 if none declared).
    pub uncertainty: f64,
    /// Human-readable length unit name ("mm", "inch", ...).
    pub length_name: String2,
}

/// Small inline string for unit names.
pub type String2 = smallvec::SmallVec<[u8; 16]>;

impl Default for UnitScale {
    fn default() -> Self {
        UnitScale { length: 1.0, angle: 1.0, uncertainty: 0.0, length_name: b"mm"[..].into() }
    }
}

impl UnitScale {
    pub fn length_name_str(&self) -> &str {
        std::str::from_utf8(&self.length_name).unwrap_or("?")
    }
}

fn si_prefix_factor(prefix: &[u8]) -> f64 {
    match prefix {
        b"EXA" => 1e18,
        b"PETA" => 1e15,
        b"TERA" => 1e12,
        b"GIGA" => 1e9,
        b"MEGA" => 1e6,
        b"KILO" => 1e3,
        b"HECTO" => 1e2,
        b"DECA" => 1e1,
        b"DECI" => 1e-1,
        b"CENTI" => 1e-2,
        b"MILLI" => 1e-3,
        b"MICRO" => 1e-6,
        b"NANO" => 1e-9,
        b"PICO" => 1e-12,
        b"FEMTO" => 1e-15,
        b"ATTO" => 1e-18,
        _ => 1.0,
    }
}

impl<'f> Model<'f> {
    /// Unit scales of a representation context (works when handed the representation itself too:
    /// it follows `context_of_items`).
    pub fn unit_scale(&self, ctx: EntityId) -> UnitScale {
        let ctx = self.resolve_context(ctx);
        let mut out = UnitScale::default();
        let mut found_length = false;
        if let Ok(args) = self.part(ctx, EntityType::GlobalUnitAssignedContext)
            && let Ok(units) = list_attr(ctx, args, 0) {
                for u in units.refs() {
                    if !found_length && self.is_a(u, EntityType::LengthUnit) {
                        if let Some((s, name)) = self.length_unit_mm(u, 0) {
                            out.length = s;
                            out.length_name = name;
                            found_length = true;
                        }
                    } else if self.is_a(u, EntityType::PlaneAngleUnit)
                        && let Some(s) = self.angle_unit_rad(u, 0) {
                            out.angle = s;
                        }
                }
            }
        if let Ok(args) = self.part(ctx, EntityType::GlobalUncertaintyAssignedContext)
            && let Ok(list) = list_attr(ctx, args, 0) {
                for um in list.refs() {
                    if let Some(v) = self.uncertainty_mm(um) {
                        out.uncertainty = v;
                        break;
                    }
                }
            }
        out
    }

    /// If `id` is a representation, return its context; else `id` itself.
    fn resolve_context(&self, id: EntityId) -> EntityId {
        if self.is_a(id, EntityType::GeometricRepresentationContext) || self.is_a(id, EntityType::RepresentationContext) {
            return id;
        }
        // representation: (name, items, context_of_items)
        if let Ok((_, args)) = self.any_part(
            id,
            &[
                EntityType::ShapeRepresentation,
                EntityType::AdvancedBrepShapeRepresentation,
                EntityType::ManifoldSurfaceShapeRepresentation,
                EntityType::FacetedBrepShapeRepresentation,
                EntityType::GeometricallyBoundedSurfaceShapeRepresentation,
                EntityType::GeometricallyBoundedWireframeShapeRepresentation,
                EntityType::EdgeBasedWireframeShapeRepresentation,
                EntityType::Representation,
            ],
        )
            && let Ok(c) = ref_attr(id, args, 2) {
                return c;
            }
        id
    }

    /// mm per unit for a length unit entity (SI or conversion based). `depth` guards cycles.
    fn length_unit_mm(&self, u: EntityId, depth: u32) -> Option<(f64, String2)> {
        if depth > 4 {
            return None;
        }
        if let Ok(si) = self.part(u, EntityType::SiUnit) {
            // (prefix, name)
            let prefix = match si.nth(0)? {
                Arg::Enum(p) => si_prefix_factor(p),
                _ => 1.0,
            };
            let name = si.nth(1)?.as_enum()?;
            let base_mm = match name {
                b"METRE" => 1000.0,
                _ => return None,
            };
            let label: String2 = match si.nth(0)? {
                Arg::Enum(b"MILLI") => b"mm"[..].into(),
                Arg::Enum(b"CENTI") => b"cm"[..].into(),
                Arg::Enum(b"MICRO") => b"um"[..].into(),
                Arg::Enum(b"KILO") => b"km"[..].into(),
                Arg::Enum(_) => b"m*"[..].into(),
                _ => b"m"[..].into(),
            };
            return Some((prefix * base_mm, label));
        }
        if let Ok(cb) = self.part(u, EntityType::ConversionBasedUnit) {
            // (name, conversion_factor: measure_with_unit)
            let name: String2 = match cb.nth(0)? {
                Arg::Str(s) => {
                    let s = step_p21::decode_string(s).to_ascii_lowercase();
                    s.as_bytes().iter().copied().take(16).collect()
                }
                _ => b"unit"[..].into(),
            };
            let mwu = cb.nth(1)?.as_ref()?;
            let (value, base) = self.measure_with_unit(mwu)?;
            let (base_mm, _) = self.length_unit_mm(base, depth + 1)?;
            return Some((value * base_mm, name));
        }
        None
    }

    fn angle_unit_rad(&self, u: EntityId, depth: u32) -> Option<f64> {
        if depth > 4 {
            return None;
        }
        if let Ok(si) = self.part(u, EntityType::SiUnit) {
            return match si.nth(1)?.as_enum()? {
                b"RADIAN" => Some(1.0),
                _ => None,
            };
        }
        if let Ok(cb) = self.part(u, EntityType::ConversionBasedUnit) {
            let mwu = cb.nth(1)?.as_ref()?;
            let (value, base) = self.measure_with_unit(mwu)?;
            return Some(value * self.angle_unit_rad(base, depth + 1)?);
        }
        None
    }

    /// `(value_component, unit_component)` of a MEASURE_WITH_UNIT (any subtype, simple or complex).
    fn measure_with_unit(&self, id: EntityId) -> Option<(f64, EntityId)> {
        let (_, args) = self
            .any_part(
                id,
                &[
                    EntityType::MeasureWithUnit,
                    EntityType::LengthMeasureWithUnit,
                    EntityType::PlaneAngleMeasureWithUnit,
                    EntityType::UncertaintyMeasureWithUnit,
                ],
            )
            .ok()?;
        let v = attr(id, args, 0).ok()?.as_f64()?;
        let u = attr(id, args, 1).ok()?.as_ref()?;
        Some((v, u))
    }

    fn uncertainty_mm(&self, id: EntityId) -> Option<f64> {
        let (v, u) = self.measure_with_unit(id)?;
        let (mm, _) = self.length_unit_mm(u, 0)?;
        Some(v * mm)
    }
}
