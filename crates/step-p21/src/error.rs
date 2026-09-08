use thiserror::Error;

use crate::EntityId;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a STEP Part 21 file (missing ISO-10303-21 header)")]
    NotStep,
    #[error("file too large for 32-bit offsets ({0} bytes)")]
    TooLarge(u64),
    #[error("DATA section not found")]
    NoDataSection,
    #[error("malformed entity near byte {offset}: {msg}")]
    Malformed { offset: usize, msg: &'static str },
    #[error("entity #{0} not found")]
    Missing(EntityId),
    #[error("entity #{id}: expected {expected}, found {found}")]
    WrongType { id: EntityId, expected: &'static str, found: String },
    #[error("entity #{id}: attribute {index}: {msg}")]
    Attr { id: EntityId, index: usize, msg: &'static str },
    #[error("attribute parse: {0}")]
    Arg(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
