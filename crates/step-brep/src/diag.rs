//! Load-time diagnostics: counted by kind, with a bounded number of samples per kind.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use step_p21::EntityId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DiagKind {
    UnsupportedSurface,
    UnsupportedCurve,
    DanglingRef,
    MalformedEntity,
    UnknownUnit,
    NoProductStructure,
    AmbiguousTransform,
    EmptyRepresentation,
    DegenerateGeometry,
    ColourUnresolved,
}

impl fmt::Display for DiagKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagKind,
    pub entity: Option<EntityId>,
    pub msg: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub counts: BTreeMap<DiagKind, u32>,
    /// At most `SAMPLES_PER_KIND` per kind.
    pub samples: Vec<Diagnostic>,
}

pub const SAMPLES_PER_KIND: usize = 20;

impl Diagnostics {
    pub fn push(&mut self, kind: DiagKind, entity: Option<EntityId>, msg: impl Into<String>) {
        let c = self.counts.entry(kind).or_insert(0);
        *c += 1;
        if (*c as usize) <= SAMPLES_PER_KIND {
            self.samples.push(Diagnostic { kind, entity, msg: msg.into() });
        }
    }

    pub fn merge(&mut self, other: Diagnostics) {
        for (k, c) in other.counts {
            *self.counts.entry(k).or_insert(0) += c;
        }
        for s in other.samples {
            let have = self.samples.iter().filter(|d| d.kind == s.kind).count();
            if have < SAMPLES_PER_KIND {
                self.samples.push(s);
            }
        }
    }

    pub fn total(&self) -> u32 {
        self.counts.values().sum()
    }

    pub fn count(&self, kind: DiagKind) -> u32 {
        self.counts.get(&kind).copied().unwrap_or(0)
    }
}

/// Thread-safe sink shared by parallel extraction tasks.
#[derive(Debug, Default)]
pub struct DiagSink(Mutex<Diagnostics>);

impl DiagSink {
    pub fn push(&self, kind: DiagKind, entity: Option<EntityId>, msg: impl Into<String>) {
        self.0.lock().unwrap().push(kind, entity, msg);
    }
    pub fn merge(&self, d: Diagnostics) {
        self.0.lock().unwrap().merge(d);
    }
    pub fn into_inner(self) -> Diagnostics {
        self.0.into_inner().unwrap()
    }
    pub fn snapshot(&self) -> Diagnostics {
        self.0.lock().unwrap().clone()
    }
}
