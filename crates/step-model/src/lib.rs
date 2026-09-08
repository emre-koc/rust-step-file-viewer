//! `step-model`: typed, lazy decoders over a [`StepFile`].
//!
//! Nothing here allocates a full object model. Each decoder reads one entity's attributes on
//! demand and returns either a small record of `EntityId`s (topology, product structure) or a
//! finished geometry value in millimetres (`step_mesh::geom` types). Complex instances are handled
//! transparently by looking up the relevant partial entity.

pub mod geometry;
pub mod product;
pub mod topology;
pub mod units;

use std::fmt;

use step_p21::{Arg, Args, EntityId, EntityType, StepFile};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    P21(#[from] step_p21::Error),
    #[error("{id}: unsupported {what} '{name}'")]
    Unsupported { id: EntityId, what: &'static str, name: String },
    #[error("{id}: {msg}")]
    Invalid { id: EntityId, msg: &'static str },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn is_unsupported(&self) -> bool {
        matches!(self, Error::Unsupported { .. })
    }
}

/// Decoder facade over an indexed file.
#[derive(Clone, Copy)]
pub struct Model<'f> {
    pub file: &'f StepFile,
}

impl fmt::Debug for Model<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Model({} entities)", self.file.len())
    }
}

impl<'f> Model<'f> {
    pub fn new(file: &'f StepFile) -> Self {
        Model { file }
    }

    /// Entity type, or `Missing`.
    #[inline]
    pub fn ty(&self, id: EntityId) -> EntityType {
        self.file.entity_type(id)
    }

    /// Does `id` exist and is (or contains) `ty`?
    #[inline]
    pub fn is_a(&self, id: EntityId, ty: EntityType) -> bool {
        self.file.is_a(id, ty)
    }

    /// Attributes of `id` as `ty`: the simple entity's args, or the matching partial entity of a
    /// complex instance.
    pub fn part(&self, id: EntityId, ty: EntityType) -> Result<Args<'f>> {
        let rec = self.file.get(id).ok_or(step_p21::Error::Missing(id))?;
        if rec.is_complex() {
            self.file.complex_part(id, ty).ok_or_else(|| {
                step_p21::Error::WrongType { id, expected: ty.name(), found: self.file.type_name(id).unwrap_or_default() }.into()
            })
        } else if rec.ty == ty {
            Ok(self.file.args(id).expect("present"))
        } else {
            Err(step_p21::Error::WrongType { id, expected: ty.name(), found: self.file.type_name(id).unwrap_or_default() }
                .into())
        }
    }

    /// First of `tys` that `id` is; returns which one and its args.
    pub fn any_part(&self, id: EntityId, tys: &[EntityType]) -> Result<(EntityType, Args<'f>)> {
        let rec = self.file.get(id).ok_or(step_p21::Error::Missing(id))?;
        if rec.is_complex() {
            for &ty in tys {
                if let Some(a) = self.file.complex_part(id, ty) {
                    return Ok((ty, a));
                }
            }
        } else if tys.contains(&rec.ty) {
            return Ok((rec.ty, self.file.args(id).expect("present")));
        }
        Err(step_p21::Error::WrongType { id, expected: tys.first().map_or("?", |t| t.name()), found: self.file.type_name(id).unwrap_or_default() }.into())
    }

    /// Raw args of a simple entity together with its type.
    pub fn typed_args(&self, id: EntityId) -> Result<(EntityType, Args<'f>)> {
        Ok(self.file.typed_args(id)?)
    }

    pub fn type_name(&self, id: EntityId) -> String {
        self.file.type_name(id).unwrap_or_else(|| "<missing>".into())
    }

    fn unsupported(&self, id: EntityId, what: &'static str) -> Error {
        Error::Unsupported { id, what, name: self.type_name(id) }
    }

    fn invalid(&self, id: EntityId, msg: &'static str) -> Error {
        Error::Invalid { id, msg }
    }
}

// ----- attribute helpers -----

pub(crate) fn attr<'a>(id: EntityId, args: Args<'a>, index: usize) -> Result<Arg<'a>> {
    args.nth(index).ok_or_else(|| step_p21::Error::Attr { id, index, msg: "missing attribute" }.into())
}

pub(crate) fn ref_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<EntityId> {
    attr(id, args, index)?.as_ref().ok_or_else(|| step_p21::Error::Attr { id, index, msg: "expected entity reference" }.into())
}

pub(crate) fn opt_ref_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<Option<EntityId>> {
    match attr(id, args, index)? {
        Arg::Ref(r) => Ok(Some(r)),
        Arg::Null | Arg::Derived => Ok(None),
        _ => Err(step_p21::Error::Attr { id, index, msg: "expected entity reference or $" }.into()),
    }
}

pub(crate) fn real_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<f64> {
    attr(id, args, index)?.as_f64().ok_or_else(|| step_p21::Error::Attr { id, index, msg: "expected real" }.into())
}

pub(crate) fn int_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<i64> {
    attr(id, args, index)?.as_i64().ok_or_else(|| step_p21::Error::Attr { id, index, msg: "expected integer" }.into())
}

pub(crate) fn bool_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<bool> {
    let a = attr(id, args, index)?;
    match a {
        Arg::Enum(b"T") => Ok(true),
        Arg::Enum(b"F") | Arg::Enum(b"U") => Ok(false),
        Arg::Null | Arg::Derived => Ok(true),
        _ => Err(step_p21::Error::Attr { id, index, msg: "expected .T./.F." }.into()),
    }
}

pub(crate) fn string_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<String> {
    match attr(id, args, index)? {
        Arg::Str(s) => Ok(step_p21::decode_string(s).into_owned()),
        Arg::Null | Arg::Derived => Ok(String::new()),
        _ => Err(step_p21::Error::Attr { id, index, msg: "expected string" }.into()),
    }
}

pub(crate) fn list_attr<'a>(id: EntityId, args: Args<'a>, index: usize) -> Result<Args<'a>> {
    match attr(id, args, index)? {
        Arg::List(l) => Ok(l),
        Arg::Null | Arg::Derived => Ok(Args::new(&[])),
        _ => Err(step_p21::Error::Attr { id, index, msg: "expected list" }.into()),
    }
}

pub(crate) fn ref_list_attr(id: EntityId, args: Args<'_>, index: usize) -> Result<Vec<EntityId>> {
    Ok(list_attr(id, args, index)?.refs().collect())
}
