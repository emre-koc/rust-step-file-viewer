//! `step-p21`: fast ISO 10303-21 (STEP Part 21) indexer with lazy attribute decoding.
//!
//! [`StepFile::open`] memory-maps the file, parses the HEADER, and builds an entity table in
//! parallel (one `EntityRec` per `#id`, 12 bytes each). Attributes are *not* parsed up front: call
//! [`StepFile::args`] to get a zero-copy [`Args`] cursor over one entity's attribute list.
//! Complex (multi-supertype) instances are recorded with their partial entities so callers can ask
//! for a specific supertype's attributes with [`StepFile::complex_part`].

mod args;
mod error;
mod header;
mod scan;
mod string;
mod types;

use std::fmt;
use std::path::Path;
use std::time::Instant;

pub use args::{Arg, ArgIter, Args, parse_real};
pub use error::{Error, Result};
pub use header::{Ap, Header};
pub use scan::{ComplexEntity, ComplexPart, EntityRec, IndexStats};
pub use string::decode_string;
pub use types::EntityType;

/// Entity instance name `#id`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u32);

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

enum Backing {
    Mmap(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl std::ops::Deref for Backing {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Backing::Mmap(m) => m,
            Backing::Owned(v) => v,
        }
    }
}

/// An indexed Part 21 file.
pub struct StepFile {
    bytes: Backing,
    header: Header,
    table: Vec<EntityRec>,
    complex: Vec<ComplexEntity>,
    /// (type name, count) for simple entities, sorted by count descending.
    type_counts: Vec<(String, u32)>,
    /// (type name, count) for partial entities inside complex instances.
    complex_type_counts: Vec<(String, u32)>,
    errors: Vec<(usize, &'static str)>,
    stats: IndexStats,
    index_ms: f32,
    data_start: usize,
    data_end: usize,
}

impl StepFile {
    /// Memory-map and index a file.
    pub fn open(path: impl AsRef<Path>) -> Result<StepFile> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        if len > u32::MAX as u64 - 16 {
            return Err(Error::TooLarge(len));
        }
        // SAFETY: the file is opened read-only and we never write through the map. Concurrent
        // modification by another process would be a data race on the bytes, which this viewer
        // accepts (it re-validates structure and never trusts offsets beyond the slice length).
        let map = unsafe { memmap2::Mmap::map(&file)? };
        #[cfg(unix)]
        {
            let _ = map.advise(memmap2::Advice::Sequential);
        }
        Self::from_backing(Backing::Mmap(map))
    }

    /// Index an in-memory buffer (tests, embedded fixtures).
    pub fn from_bytes(bytes: Vec<u8>) -> Result<StepFile> {
        Self::from_backing(Backing::Owned(bytes))
    }

    fn from_backing(bytes: Backing) -> Result<StepFile> {
        let t0 = Instant::now();
        let b: &[u8] = &bytes;
        let head = &b[..b.len().min(4096)];
        if memchr::memmem::find(head, b"ISO-10303-21").is_none() {
            return Err(Error::NotStep);
        }
        // HEADER; ... ENDSEC;
        let header = match memchr::memmem::find(b, b"HEADER;") {
            Some(hs) => {
                let he = memchr::memmem::find(&b[hs..], b"ENDSEC;").map(|e| hs + e).unwrap_or(hs + 7);
                Header::parse(&b[hs + 7..he])
            }
            None => Header::default(),
        };
        // DATA; ... ENDSEC;  (DATA may carry parameters in Part 21 ed.3: DATA(...);)
        let ds = memchr::memmem::find(b, b"DATA").ok_or(Error::NoDataSection)?;
        let data_start = {
            let mut i = ds + 4;
            // skip optional (...) and the ';'
            while i < b.len() && args::is_ws(b[i]) {
                i += 1;
            }
            if i < b.len() && b[i] == b'(' {
                i = args::find_close_paren(b, i).map(|c| c + 1).unwrap_or(i);
            }
            match memchr::memchr(b';', &b[i..]) {
                Some(j) => i + j + 1,
                None => return Err(Error::NoDataSection),
            }
        };
        let data_end = memchr::memmem::rfind(b, b"ENDSEC;").filter(|&e| e > data_start).unwrap_or(b.len());

        let threads = rayon::current_num_threads();
        let indexed = scan::index_data(b, data_start, data_end, threads)?;

        let mut type_counts: Vec<(String, u32)> = Vec::new();
        for (k, &c) in indexed.known_counts.iter().enumerate() {
            if c > 0 && k >= 3 {
                type_counts.push((EntityType::ALL[k - 3].name().to_string(), c));
            }
        }
        for (name, c) in &indexed.other_names {
            type_counts.push((String::from_utf8_lossy(name).into_owned(), *c));
        }
        // Split simple vs complex-part counts: recount complex parts separately.
        let mut complex_counts: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        let mut complex_other: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for ce in &indexed.complex {
            for p in &ce.parts {
                if p.ty == EntityType::Other {
                    let name = String::from_utf8_lossy(&b[p.name_start as usize..p.name_start as usize + p.name_len as usize]).into_owned();
                    *complex_other.entry(name).or_insert(0) += 1;
                } else {
                    *complex_counts.entry(p.ty.name()).or_insert(0) += 1;
                }
            }
        }
        // known_counts/other_names included complex parts; subtract them for the simple list.
        for (name, c) in type_counts.iter_mut() {
            if let Some(cc) = complex_counts.get(name.as_str()) {
                *c -= cc;
            } else if let Some(cc) = complex_other.get(name) {
                *c -= cc;
            }
        }
        type_counts.retain(|(_, c)| *c > 0);
        type_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut complex_type_counts: Vec<(String, u32)> = complex_counts.into_iter().map(|(n, c)| (n.to_string(), c)).collect();
        complex_type_counts.extend(complex_other);
        complex_type_counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let index_ms = t0.elapsed().as_secs_f32() * 1000.0;
        tracing::debug!(entities = indexed.stats.entities, complex = indexed.stats.complex, chunks = indexed.stats.chunks, ms = index_ms, "indexed");
        Ok(StepFile {
            header,
            table: indexed.table,
            complex: indexed.complex,
            type_counts,
            complex_type_counts,
            errors: indexed.errors,
            stats: indexed.stats,
            index_ms,
            data_start,
            data_end,
            bytes,
        })
    }

    // ----- accessors -----

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn stats(&self) -> &IndexStats {
        &self.stats
    }
    pub fn index_ms(&self) -> f32 {
        self.index_ms
    }
    pub fn errors(&self) -> &[(usize, &'static str)] {
        &self.errors
    }
    /// Simple-entity type counts, descending.
    pub fn type_counts(&self) -> &[(String, u32)] {
        &self.type_counts
    }
    /// Partial-entity type counts inside complex instances, descending.
    pub fn complex_type_counts(&self) -> &[(String, u32)] {
        &self.complex_type_counts
    }
    /// Number of entity instances.
    pub fn len(&self) -> usize {
        self.stats.entities as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Largest `#id` in the file.
    pub fn max_id(&self) -> u32 {
        self.table.len().saturating_sub(1) as u32
    }
    pub fn data_range(&self) -> (usize, usize) {
        (self.data_start, self.data_end)
    }

    #[inline]
    pub fn get(&self, id: EntityId) -> Option<&EntityRec> {
        self.table.get(id.0 as usize).filter(|r| r.present())
    }

    /// Type of an entity (`Complex` for complex instances). `Missing` if absent.
    #[inline]
    pub fn entity_type(&self, id: EntityId) -> EntityType {
        self.table.get(id.0 as usize).map_or(EntityType::Missing, |r| r.ty)
    }

    /// Does the entity exist and is it (or, for complex instances, does it contain) `ty`?
    pub fn is_a(&self, id: EntityId, ty: EntityType) -> bool {
        match self.get(id) {
            None => false,
            Some(r) if r.is_complex() => self.complex_parts(id).is_some_and(|c| c.parts.iter().any(|p| p.ty == ty)),
            Some(r) => r.ty == ty,
        }
    }

    /// Attribute cursor of a simple entity. For complex instances this covers the whole
    /// `(A(..)B(..))` body; use [`complex_part`](Self::complex_part) instead.
    #[inline]
    pub fn args(&self, id: EntityId) -> Option<Args<'_>> {
        let r = self.get(id)?;
        Some(Args::new(&self.bytes[r.args_start as usize..(r.args_start + r.args_len) as usize]))
    }

    /// `(type, args)` of a simple entity, erroring on missing / complex.
    pub fn typed_args(&self, id: EntityId) -> Result<(EntityType, Args<'_>)> {
        let r = self.get(id).ok_or(Error::Missing(id))?;
        Ok((r.ty, Args::new(&self.bytes[r.args_start as usize..(r.args_start + r.args_len) as usize])))
    }

    /// Partial entities of a complex instance.
    pub fn complex_parts(&self, id: EntityId) -> Option<&ComplexEntity> {
        let i = self.complex.binary_search_by_key(&id.0, |c| c.id.0).ok()?;
        Some(&self.complex[i])
    }

    /// Attributes of one partial entity (`ty`) of a complex instance.
    pub fn complex_part(&self, id: EntityId, ty: EntityType) -> Option<Args<'_>> {
        let c = self.complex_parts(id)?;
        let p = c.parts.iter().find(|p| p.ty == ty)?;
        Some(Args::new(&self.bytes[p.args_start as usize..(p.args_start + p.args_len) as usize]))
    }

    /// Types of all partial entities of a complex instance (empty for simple ones).
    pub fn complex_part_types(&self, id: EntityId) -> impl Iterator<Item = EntityType> + '_ {
        self.complex_parts(id).into_iter().flat_map(|c| c.parts.iter().map(|p| p.ty))
    }

    /// Name of the entity's type as written in the file (works for `Other` too).
    pub fn type_name(&self, id: EntityId) -> Option<String> {
        let r = self.get(id)?;
        if r.is_complex() {
            let c = self.complex_parts(id)?;
            let names: Vec<String> = c
                .parts
                .iter()
                .map(|p| String::from_utf8_lossy(&self.bytes[p.name_start as usize..(p.name_start as usize + p.name_len as usize)]).into_owned())
                .collect();
            return Some(format!("({})", names.join(" ")));
        }
        if r.ty != EntityType::Other {
            return Some(r.ty.name().to_string());
        }
        // scan backwards from the '(' for the name
        let mut e = r.args_start as usize - 1; // index of '('
        while e > 0 && args::is_ws(self.bytes[e - 1]) {
            e -= 1;
        }
        let mut s = e;
        while s > 0 && (self.bytes[s - 1].is_ascii_alphanumeric() || self.bytes[s - 1] == b'_') {
            s -= 1;
        }
        Some(String::from_utf8_lossy(&self.bytes[s..e]).into_owned())
    }

    /// Full source text of one entity (`#id=...;`) for diagnostics.
    pub fn entity_source(&self, id: EntityId) -> Option<&str> {
        let r = self.get(id)?;
        let end = (r.args_start + r.args_len) as usize;
        let mut e = end;
        while e < self.bytes.len() && self.bytes[e] != b';' {
            e += 1;
        }
        let mut s = r.args_start as usize;
        while s > 0 && self.bytes[s - 1] != b'\n' {
            s -= 1;
            if self.bytes[s] == b'#' && (s == 0 || self.bytes[s - 1] == b'\n' || self.bytes[s - 1] == b';') {
                break;
            }
        }
        std::str::from_utf8(&self.bytes[s..(e + 1).min(self.bytes.len())]).ok()
    }

    /// Ids of all entities that are (or contain as a partial entity) `ty`.
    pub fn by_type(&self, ty: EntityType) -> impl Iterator<Item = EntityId> + '_ {
        let simple = self
            .table
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.ty == ty)
            .map(|(i, _)| EntityId(i as u32));
        let complex = self
            .complex
            .iter()
            .filter(move |c| c.parts.iter().any(|p| p.ty == ty))
            .map(|c| c.id);
        // both are ascending; simple merge
        let mut out: Vec<EntityId> = simple.chain(complex).collect();
        out.sort_unstable();
        out.into_iter()
    }

    /// Iterate all present entity ids in ascending order.
    pub fn ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.table.iter().enumerate().filter(|(_, r)| r.present()).map(|(i, _)| EntityId(i as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: &str = "ISO-10303-21;\r\nHEADER;\r\nFILE_DESCRIPTION (( 'STEP AP203' ),\r\n    '1' );\r\nFILE_NAME ('A-114.STEP',\r\n    '2024-05-22T14:37:20',\r\n    ( '' ),\r\n    ( '' ),\r\n    'SwSTEP 2.0',\r\n    'SolidWorks 2021',\r\n    '' );\r\nFILE_SCHEMA (( 'CONFIG_CONTROL_DESIGN' ));\r\nENDSEC;\r\n\r\nDATA;\r\n#1 = CARTESIAN_POINT ( 'NONE',  ( -17.3, 1.17, -1.38 ) ) ;\r\n#2 = DIRECTION ( 'NONE',  ( 0.0, 0.0, 1.0 ) ) ;\r\n/* a comment */\r\n#3 =( BOUNDED_SURFACE ( )  B_SPLINE_SURFACE ( 3, 3, ( ( #1, #2 ) ), .UNSPECIFIED., .F., .F., .F. ) RATIONAL_B_SPLINE_SURFACE ( ( ( 1.0, 2.0 ) ) ) );\r\n#5 = APPROVAL ( #1, 'BEL''RT; #99 (x)' ) ;\r\n#4 = ADVANCED_FACE ( 'NONE', ( #374 ), #1302, .T. ) ;\r\nENDSEC;\r\nEND-ISO-10303-21;\r\n";

    #[test]
    fn small_file_indexes() {
        let f = StepFile::from_bytes(SMALL.as_bytes().to_vec()).unwrap();
        assert_eq!(f.len(), 5);
        assert_eq!(f.max_id(), 5);
        assert_eq!(f.header().ap(), Ap::Ap203);
        assert_eq!(f.header().originating_system, "SolidWorks 2021");
        assert_eq!(f.header().name, "A-114.STEP");
        assert_eq!(f.entity_type(EntityId(1)), EntityType::CartesianPoint);
        assert_eq!(f.entity_type(EntityId(3)), EntityType::Complex);
        assert_eq!(f.entity_type(EntityId(5)), EntityType::Other);
        assert_eq!(f.type_name(EntityId(5)).unwrap(), "APPROVAL");
        assert!(f.is_a(EntityId(3), EntityType::RationalBSplineSurface));
        assert!(f.is_a(EntityId(3), EntityType::BSplineSurface));
        assert!(!f.is_a(EntityId(3), EntityType::Plane));
        let w = f.complex_part(EntityId(3), EntityType::RationalBSplineSurface).unwrap();
        let rows = w.nth(0).unwrap().as_list().unwrap();
        assert_eq!(rows.nth(0).unwrap().as_list().unwrap().reals().unwrap(), vec![1.0, 2.0]);
        let p = f.args(EntityId(1)).unwrap().nth(1).unwrap().as_list().unwrap().f64x3().unwrap();
        assert_eq!(p, [-17.3, 1.17, -1.38]);
        assert!(f.errors().is_empty(), "{:?}", f.errors());
        // string containing ';' '#' '(' must not break entity 5 or 4
        assert_eq!(f.entity_type(EntityId(4)), EntityType::AdvancedFace);
        assert_eq!(f.args(EntityId(5)).unwrap().nth(1).unwrap().as_string().unwrap(), "BEL'RT; #99 (x)");
        assert_eq!(f.type_counts()[0], ("ADVANCED_FACE".to_string(), 1));
        assert!(f.complex_type_counts().iter().any(|(n, c)| n == "BOUNDED_SURFACE" && *c == 1));
        assert_eq!(f.by_type(EntityType::BSplineSurface).collect::<Vec<_>>(), vec![EntityId(3)]);
    }

    #[test]
    fn chunk_boundary_invariance() {
        // Build a ~3 MB synthetic file and index it with different thread counts / chunkings.
        let mut s = String::from("ISO-10303-21;\nHEADER;\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n");
        let n = 60_000u32;
        for i in 1..=n {
            match i % 4 {
                0 => s.push_str(&format!("#{i}=CARTESIAN_POINT('',({}.E0,{}.E-1,\n{}.E0));\n", i, i * 2, i * 3)),
                1 => s.push_str(&format!("#{i}=DIRECTION('name ; with #{} (parens)',(0.E0,0.E0,1.E0));\n", i + 1)),
                2 => s.push_str(&format!("#{i}=(NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.)LENGTH_UNIT());\n")),
                _ => s.push_str(&format!("#{i}=ORIENTED_EDGE('',*,*,#{},.F.);\n", i - 1)),
            }
        }
        s.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
        let bytes = s.into_bytes();
        let b = &bytes[..];
        let ds = memchr::memmem::find(b, b"DATA;").unwrap() + 5;
        let de = memchr::memmem::rfind(b, b"ENDSEC;").unwrap();
        let reference = scan::index_data(b, ds, de, 1).unwrap();
        for threads in [2usize, 3, 7, 16, 64] {
            let got = scan::index_data(b, ds, de, threads).unwrap();
            assert_eq!(got.table, reference.table, "threads={threads}");
            assert_eq!(got.complex, reference.complex, "threads={threads}");
            assert!(got.errors.is_empty());
            assert!(!got.stats.sequential_fallback || threads == 1);
        }
        assert_eq!(reference.stats.entities, n);
        assert_eq!(reference.stats.complex, n / 4);
    }

    #[test]
    fn duplicate_and_malformed_are_reported_not_fatal() {
        let s = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\n#1=DIRECTION('',(1.,0.,0.));\n#2 GARBAGE\n#3=VECTOR('',#1,1.);\nENDSEC;\n";
        let f = StepFile::from_bytes(s.as_bytes().to_vec()).unwrap();
        assert!(f.errors().len() >= 2);
        assert_eq!(f.entity_type(EntityId(3)), EntityType::Vector);
    }

    #[test]
    fn rejects_non_step() {
        assert!(matches!(StepFile::from_bytes(b"hello world".to_vec()), Err(Error::NotStep)));
    }
}
