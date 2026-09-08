//! Colour index: which faces / shells / bodies carry an explicit surface colour.

use rustc_hash::FxHashMap;
use step_model::Model;
use step_p21::{EntityId, EntityType};

use crate::diag::{DiagKind, Diagnostics};

pub type Rgba8 = [u8; 4];

pub const DEFAULT_COLOUR: Rgba8 = [178, 178, 186, 255];

#[derive(Clone, Debug, Default)]
pub struct StyleIndex {
    pub face: FxHashMap<EntityId, Rgba8>,
    pub shell: FxHashMap<EntityId, Rgba8>,
    pub body: FxHashMap<EntityId, Rgba8>,
    /// Anything else (representations, geometric sets, mapped items).
    pub other: FxHashMap<EntityId, Rgba8>,
    pub styled_items_seen: u32,
    pub styled_items_skipped_curves: u32,
}

impl StyleIndex {
    pub fn is_empty(&self) -> bool {
        self.face.is_empty() && self.shell.is_empty() && self.body.is_empty() && self.other.is_empty()
    }

    /// Resolve a face colour: face override > shell > body > default.
    pub fn resolve(&self, face: EntityId, shell: EntityId, body: EntityId) -> Rgba8 {
        self.face
            .get(&face)
            .or_else(|| self.shell.get(&shell))
            .or_else(|| self.body.get(&body))
            .copied()
            .unwrap_or(DEFAULT_COLOUR)
    }
}

pub fn build_styles(model: &Model<'_>, diags: &mut Diagnostics) -> StyleIndex {
    let mut idx = StyleIndex::default();
    // Plain styled items first, overrides second so they win.
    for pass in [EntityType::StyledItem, EntityType::OverRidingStyledItem] {
        for id in model.file.by_type(pass) {
            if model.ty(id) != pass && !(pass == EntityType::OverRidingStyledItem && model.is_a(id, pass)) {
                continue;
            }
            idx.styled_items_seen += 1;
            let Some(target) = model.styled_item_target(id) else { continue };
            let tty = model.ty(target);
            let tty = if tty == EntityType::Complex {
                model.file.complex_part_types(target).next().unwrap_or(EntityType::Other)
            } else {
                tty
            };
            if tty.is_curve() || matches!(tty, EntityType::Missing) {
                idx.styled_items_skipped_curves += 1;
                continue;
            }
            let Ok(rec) = model.styled_item(id) else { continue };
            let mut colour = None;
            for psa in &rec.styles {
                if let Some(c) = model.surface_colour_of_style(*psa) {
                    colour = Some(c);
                    break;
                }
            }
            let Some(colour) = colour else {
                diags.push(DiagKind::ColourUnresolved, Some(id), format!("no surface colour for styled item targeting {}", model.type_name(target)));
                continue;
            };
            use EntityType as T;
            match tty {
                T::AdvancedFace | T::FaceSurface | T::OrientedFace => idx.face.insert(target, colour),
                T::ClosedShell | T::OpenShell | T::OrientedClosedShell | T::OrientedOpenShell | T::ConnectedFaceSet => idx.shell.insert(target, colour),
                T::ManifoldSolidBrep | T::BrepWithVoids | T::FacetedBrep | T::ShellBasedSurfaceModel => idx.body.insert(target, colour),
                _ => idx.other.insert(target, colour),
            };
        }
    }
    idx
}
