//! Product structure, representations and presentation records (ids only).

use step_p21::{Arg, EntityId, EntityType};

use crate::{Model, Result, attr, list_attr, ref_attr, ref_list_attr, string_attr};

#[derive(Clone, Debug)]
pub struct RepresentationRec {
    pub name: String,
    pub items: Vec<EntityId>,
    pub context: EntityId,
}

#[derive(Clone, Copy, Debug)]
pub struct MappedItemRec {
    /// REPRESENTATION_MAP
    pub source: EntityId,
    /// AXIS2_PLACEMENT_3D or CARTESIAN_TRANSFORMATION_OPERATOR_3D
    pub target: EntityId,
}

#[derive(Clone, Copy, Debug)]
pub struct RepresentationMapRec {
    pub origin: EntityId,
    pub representation: EntityId,
}

#[derive(Clone, Debug)]
pub struct ProductRec {
    pub id_str: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug)]
pub struct NauoRec {
    pub parent: EntityId,
    pub child: EntityId,
}

#[derive(Clone, Debug)]
pub struct NauoNames {
    pub id_str: String,
    pub name: String,
    pub reference_designator: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RepRelationshipRec {
    pub rep_1: EntityId,
    pub rep_2: EntityId,
    /// ITEM_DEFINED_TRANSFORMATION (or CARTESIAN_TRANSFORMATION_OPERATOR) if present.
    pub transformation: Option<EntityId>,
}

#[derive(Clone, Copy, Debug)]
pub struct ItemDefinedTransformationRec {
    pub item_1: EntityId,
    pub item_2: EntityId,
}

#[derive(Clone, Debug)]
pub struct StyledItemRec {
    pub styles: Vec<EntityId>,
    pub item: EntityId,
}

impl<'f> Model<'f> {
    /// `(name, items, context_of_items)` of any representation.
    pub fn representation(&self, id: EntityId) -> Result<RepresentationRec> {
        use EntityType as T;
        let (_, args) = self.any_part(
            id,
            &[
                T::ShapeRepresentation,
                T::AdvancedBrepShapeRepresentation,
                T::ManifoldSurfaceShapeRepresentation,
                T::FacetedBrepShapeRepresentation,
                T::GeometricallyBoundedSurfaceShapeRepresentation,
                T::GeometricallyBoundedWireframeShapeRepresentation,
                T::EdgeBasedWireframeShapeRepresentation,
                T::Representation,
            ],
        )?;
        Ok(RepresentationRec { name: string_attr(id, args, 0)?, items: ref_list_attr(id, args, 1)?, context: ref_attr(id, args, 2)? })
    }

    pub fn is_shape_representation(&self, id: EntityId) -> bool {
        let ty = self.ty(id);
        if ty == EntityType::Complex {
            return self.file.complex_part_types(id).any(|t| t.is_shape_representation() || t == EntityType::Representation);
        }
        ty.is_shape_representation()
    }

    pub fn mapped_item(&self, id: EntityId) -> Result<MappedItemRec> {
        let args = self.part(id, EntityType::MappedItem)?;
        Ok(MappedItemRec { source: ref_attr(id, args, 1)?, target: ref_attr(id, args, 2)? })
    }

    pub fn representation_map(&self, id: EntityId) -> Result<RepresentationMapRec> {
        let args = self.part(id, EntityType::RepresentationMap)?;
        Ok(RepresentationMapRec { origin: ref_attr(id, args, 0)?, representation: ref_attr(id, args, 1)? })
    }

    /// `PRODUCT(id, name, description, frame_of_reference)`
    pub fn product(&self, id: EntityId) -> Result<ProductRec> {
        let args = self.part(id, EntityType::Product)?;
        Ok(ProductRec { id_str: string_attr(id, args, 0)?, name: string_attr(id, args, 1)? })
    }

    /// PRODUCT_DEFINITION → its formation's PRODUCT.
    pub fn product_of_definition(&self, pd: EntityId) -> Result<EntityId> {
        let args = self.part(pd, EntityType::ProductDefinition)?;
        let formation = ref_attr(pd, args, 2)?;
        let (_, fargs) = self.any_part(
            formation,
            &[EntityType::ProductDefinitionFormation, EntityType::ProductDefinitionFormationWithSpecifiedSource],
        )?;
        ref_attr(formation, fargs, 2)
    }

    /// `PRODUCT_DEFINITION_SHAPE(name, description, definition)` → definition (a PRODUCT_DEFINITION,
    /// a NEXT_ASSEMBLY_USAGE_OCCURRENCE, or something else such as a PROPERTY_DEFINITION).
    pub fn product_definition_shape_target(&self, pds: EntityId) -> Result<EntityId> {
        let (_, args) = self.any_part(pds, &[EntityType::ProductDefinitionShape, EntityType::PropertyDefinition, EntityType::ShapeAspect])?;
        ref_attr(pds, args, 2)
    }

    /// SHAPE_ASPECT(name, description, of_shape (PRODUCT_DEFINITION_SHAPE), product_definitional)
    pub fn shape_aspect_of_shape(&self, sa: EntityId) -> Result<EntityId> {
        let args = self.part(sa, EntityType::ShapeAspect)?;
        ref_attr(sa, args, 2)
    }

    /// `SHAPE_DEFINITION_REPRESENTATION(definition, used_representation)` (also PROPERTY_DEFINITION_REPRESENTATION).
    pub fn definition_representation(&self, id: EntityId) -> Result<(EntityId, EntityId)> {
        let (_, args) =
            self.any_part(id, &[EntityType::ShapeDefinitionRepresentation, EntityType::PropertyDefinitionRepresentation])?;
        Ok((ref_attr(id, args, 0)?, ref_attr(id, args, 1)?))
    }

    /// `NEXT_ASSEMBLY_USAGE_OCCURRENCE(id, name, description, relating (parent), related (child), reference_designator)`
    pub fn nauo(&self, id: EntityId) -> Result<NauoRec> {
        let (_, args) = self.any_part(id, &[EntityType::NextAssemblyUsageOccurrence, EntityType::AssemblyComponentUsage, EntityType::ProductDefinitionUsage, EntityType::ProductDefinitionRelationship])?;
        Ok(NauoRec { parent: ref_attr(id, args, 3)?, child: ref_attr(id, args, 4)? })
    }

    pub fn nauo_names(&self, id: EntityId) -> NauoNames {
        let Ok((_, args)) = self.any_part(id, &[EntityType::NextAssemblyUsageOccurrence, EntityType::AssemblyComponentUsage, EntityType::ProductDefinitionUsage, EntityType::ProductDefinitionRelationship]) else {
            return NauoNames { id_str: String::new(), name: String::new(), reference_designator: String::new() };
        };
        NauoNames {
            id_str: string_attr(id, args, 0).unwrap_or_default(),
            name: string_attr(id, args, 1).unwrap_or_default(),
            reference_designator: string_attr(id, args, 5).unwrap_or_default(),
        }
    }

    /// `CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(representation_relation, represented_product_relation)`
    pub fn cdsr(&self, id: EntityId) -> Result<(EntityId, EntityId)> {
        let args = self.part(id, EntityType::ContextDependentShapeRepresentation)?;
        Ok((ref_attr(id, args, 0)?, ref_attr(id, args, 1)?))
    }

    /// Representation relationship (simple or complex with transformation).
    pub fn rep_relationship(&self, id: EntityId) -> Result<RepRelationshipRec> {
        let (_, args) = self.any_part(id, &[EntityType::RepresentationRelationship, EntityType::ShapeRepresentationRelationship])?;
        let transformation = self
            .file
            .complex_part(id, EntityType::RepresentationRelationshipWithTransformation)
            .and_then(|a| a.nth(0))
            .and_then(|a| a.as_ref())
            .or_else(|| {
                // simple REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(name, desc, rep1, rep2, transformation)
                if self.ty(id) == EntityType::RepresentationRelationshipWithTransformation {
                    self.file.args(id).and_then(|a| a.nth(4)).and_then(|a| a.as_ref())
                } else {
                    None
                }
            });
        Ok(RepRelationshipRec { rep_1: ref_attr(id, args, 2)?, rep_2: ref_attr(id, args, 3)?, transformation })
    }

    /// `ITEM_DEFINED_TRANSFORMATION(name, description, transform_item_1, transform_item_2)`
    pub fn item_defined_transformation(&self, id: EntityId) -> Result<ItemDefinedTransformationRec> {
        let args = self.part(id, EntityType::ItemDefinedTransformation)?;
        Ok(ItemDefinedTransformationRec { item_1: ref_attr(id, args, 2)?, item_2: ref_attr(id, args, 3)? })
    }

    /// `STYLED_ITEM(name, styles, item)` / `OVER_RIDING_STYLED_ITEM(name, styles, item, over_ridden_style)`
    pub fn styled_item(&self, id: EntityId) -> Result<StyledItemRec> {
        let (_, args) = self.any_part(id, &[EntityType::StyledItem, EntityType::OverRidingStyledItem])?;
        Ok(StyledItemRec { styles: ref_list_attr(id, args, 1)?, item: ref_attr(id, args, 2)? })
    }

    /// Only the `item` reference of a styled item (cheap pre-filter).
    pub fn styled_item_target(&self, id: EntityId) -> Option<EntityId> {
        let (_, args) = self.any_part(id, &[EntityType::StyledItem, EntityType::OverRidingStyledItem]).ok()?;
        attr(id, args, 2).ok()?.as_ref()
    }

    /// Resolve a PRESENTATION_STYLE_ASSIGNMENT to a surface colour (RGBA8), if it has one.
    pub fn surface_colour_of_style(&self, psa: EntityId) -> Option<[u8; 4]> {
        use EntityType as T;
        let (_, args) = self.any_part(psa, &[T::PresentationStyleAssignment, T::PresentationStyleByContext]).ok()?;
        let styles = list_attr(psa, args, 0).ok()?;
        let mut alpha = 255u8;
        let mut colour: Option<[u8; 3]> = None;
        for st in styles.refs() {
            if self.is_a(st, T::SurfaceStyleUsage) {
                let a = self.part(st, T::SurfaceStyleUsage).ok()?;
                // (side, style: SURFACE_SIDE_STYLE)
                let side_style = attr(st, a, 1).ok()?.as_ref()?;
                let ss = self.part(side_style, T::SurfaceSideStyle).ok()?;
                for el in list_attr(side_style, ss, 1).ok()?.refs() {
                    if self.is_a(el, T::SurfaceStyleFillArea) {
                        let fa = self.part(el, T::SurfaceStyleFillArea).ok()?;
                        let fill = ref_attr(el, fa, 0).ok()?;
                        let fs = self.part(fill, T::FillAreaStyle).ok()?;
                        for fsc in list_attr(fill, fs, 1).ok()?.refs() {
                            if let Ok(c) = self.part(fsc, T::FillAreaStyleColour)
                                && let Ok(cid) = ref_attr(fsc, c, 1)
                                    && let Some(rgb) = self.colour_rgb(cid) {
                                        colour = Some(rgb);
                                    }
                        }
                    } else if self.is_a(el, T::SurfaceStyleRenderingWithProperties) || self.is_a(el, T::SurfaceStyleRendering) {
                        let (_, ra) = self.any_part(el, &[T::SurfaceStyleRenderingWithProperties, T::SurfaceStyleRendering]).ok()?;
                        // (rendering_method, surface_colour, properties)
                        if let Some(cid) = attr(el, ra, 1).ok().and_then(|a| a.as_ref())
                            && colour.is_none() {
                                colour = self.colour_rgb(cid);
                            }
                        if let Ok(props) = list_attr(el, ra, 2) {
                            for p in props.refs() {
                                if let Ok(t) = self.part(p, T::SurfaceStyleTransparent)
                                    && let Some(v) = attr(p, t, 0).ok().and_then(|a| a.as_f64()) {
                                        alpha = ((1.0 - v.clamp(0.0, 1.0)) * 255.0).round() as u8;
                                    }
                            }
                        }
                    }
                }
            }
        }
        colour.map(|[r, g, b]| [r, g, b, alpha])
    }

    /// COLOUR_RGB or a pre-defined colour name → RGB8.
    pub fn colour_rgb(&self, id: EntityId) -> Option<[u8; 3]> {
        use EntityType as T;
        if let Ok(a) = self.part(id, T::ColourRgb) {
            // (name, red, green, blue)
            let r = attr(id, a, 1).ok()?.as_f64()?;
            let g = attr(id, a, 2).ok()?.as_f64()?;
            let b = attr(id, a, 3).ok()?.as_f64()?;
            let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            return Some([to8(r), to8(g), to8(b)]);
        }
        if let Ok((_, a)) = self.any_part(id, &[T::DraughtingPreDefinedColour, T::PreDefinedColour]) {
            let name = match attr(id, a, 0).ok()? {
                Arg::Str(s) => step_p21::decode_string(s).to_ascii_lowercase(),
                _ => return None,
            };
            return Some(match name.as_str() {
                "red" => [255, 0, 0],
                "green" => [0, 255, 0],
                "blue" => [0, 0, 255],
                "yellow" => [255, 255, 0],
                "magenta" => [255, 0, 255],
                "cyan" => [0, 255, 255],
                "black" => [0, 0, 0],
                "white" => [255, 255, 255],
                "grey" | "gray" => [128, 128, 128],
                _ => return None,
            });
        }
        None
    }
}
