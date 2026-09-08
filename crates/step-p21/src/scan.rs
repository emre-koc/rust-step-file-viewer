//! Parallel entity index over the DATA section.

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::args::{find_close_paren, is_ws};
use crate::error::{Error, Result};
use crate::{EntityId, EntityType};

/// Location of one entity's attribute list inside the file. 12 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct EntityRec {
    /// Offset of the first byte after the opening `(`.
    pub args_start: u32,
    /// Length up to (excluding) the matching `)`.
    pub args_len: u32,
    pub ty: EntityType,
    pub flags: u8,
    pub(crate) _pad: u8,
}

pub const FLAG_COMPLEX: u8 = 1;

impl EntityRec {
    #[inline]
    pub fn is_complex(&self) -> bool {
        self.flags & FLAG_COMPLEX != 0
    }
    #[inline]
    pub fn present(&self) -> bool {
        self.ty != EntityType::Missing
    }
}

/// One partial entity of a complex instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComplexPart {
    pub ty: EntityType,
    pub name_start: u32,
    pub name_len: u16,
    pub args_start: u32,
    pub args_len: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComplexEntity {
    pub id: EntityId,
    pub parts: SmallVec<[ComplexPart; 8]>,
}

#[derive(Debug, Default)]
pub(crate) struct ScanOut<'a> {
    pub recs: Vec<(u32, EntityRec)>,
    pub complex: Vec<ComplexEntity>,
    pub other_names: FxHashMap<&'a [u8], u32>,
    pub known_counts: Vec<u32>,
    pub max_id: u32,
    pub errors: Vec<(usize, &'static str)>,
    /// Byte offset just past the last entity consumed.
    pub end: usize,
}

#[derive(Clone, Debug, Default)]
pub struct IndexStats {
    pub entities: u32,
    pub complex: u32,
    pub data_bytes: usize,
    pub chunks: u32,
    pub errors: u32,
    pub sequential_fallback: bool,
}

pub(crate) struct Indexed<'a> {
    pub table: Vec<EntityRec>,
    pub complex: Vec<ComplexEntity>,
    pub known_counts: Vec<u32>,
    pub other_names: FxHashMap<&'a [u8], u32>,
    pub errors: Vec<(usize, &'static str)>,
    pub stats: IndexStats,
}

#[inline]
fn skip_ws_and_comments(b: &[u8], mut i: usize) -> usize {
    loop {
        while i < b.len() && is_ws(b[i]) {
            i += 1;
        }
        if i + 1 < b.len() && b[i] == b'/' && b[i + 1] == b'*' {
            match memchr::memmem::find(&b[i + 2..], b"*/") {
                Some(j) => i = i + 2 + j + 2,
                None => return b.len(),
            }
        } else {
            return i;
        }
    }
}

/// Snap `pos` forward to the start of an entity (`#` preceded, ignoring whitespace, by `;`).
fn snap_to_entity_start(b: &[u8], mut pos: usize) -> usize {
    while pos < b.len() {
        let Some(j) = memchr::memchr(b'#', &b[pos..]) else { return b.len() };
        let at = pos + j;
        let mut k = at;
        while k > 0 && is_ws(b[k - 1]) {
            k -= 1;
        }
        if k == 0 || b[k - 1] == b';' {
            return at;
        }
        pos = at + 1;
    }
    b.len()
}

/// Error recovery: jump to the next `#` at the start of a line.
fn resync(b: &[u8], pos: usize) -> usize {
    match memchr::memmem::find(&b[pos.min(b.len())..], b"\n#") {
        Some(j) => pos + j + 1,
        None => b.len(),
    }
}

/// Parse a complex instance's inner bytes `NAME(args)NAME(args)…`.
pub(crate) fn parse_complex_parts(b: &[u8], inner_start: usize, inner_end: usize) -> SmallVec<[ComplexPart; 8]> {
    let mut parts = SmallVec::new();
    let mut i = inner_start;
    while i < inner_end {
        i = skip_ws_and_comments(b, i);
        if i >= inner_end {
            break;
        }
        let ns = i;
        while i < inner_end && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
        if i == ns {
            break; // garbage
        }
        let name = &b[ns..i];
        let mut j = skip_ws_and_comments(b, i);
        if j < inner_end && b[j] == b'(' {
            let close = match find_close_paren(b, j) {
                Some(c) if c <= inner_end => c,
                _ => break,
            };
            parts.push(ComplexPart {
                ty: EntityType::from_name(name),
                name_start: ns as u32,
                name_len: name.len().min(u16::MAX as usize) as u16,
                args_start: (j + 1) as u32,
                args_len: (close - j - 1) as u32,
            });
            j = close + 1;
        } else {
            parts.push(ComplexPart {
                ty: EntityType::from_name(name),
                name_start: ns as u32,
                name_len: name.len() as u16,
                args_start: i as u32,
                args_len: 0,
            });
        }
        i = j;
    }
    parts
}

/// Scan entities in `b[start..]` until the position reaches `stop` (the nominal chunk end); the
/// last entity may extend past `stop`.
pub(crate) fn scan_range<'a>(b: &'a [u8], start: usize, stop: usize) -> ScanOut<'a> {
    let mut out = ScanOut { known_counts: vec![0u32; EntityType::ALL.len() + 3], ..Default::default() };
    let mut i = start;
    loop {
        i = skip_ws_and_comments(b, i);
        if i >= stop || i >= b.len() {
            break;
        }
        // Stop at ENDSEC; (the caller normally bounds `stop` before it, but be safe).
        if b[i] != b'#' {
            if b[i..].starts_with(b"ENDSEC") {
                break;
            }
            out.errors.push((i, "expected '#'"));
            i = resync(b, i + 1);
            continue;
        }
        let ent_start = i;
        i += 1;
        let mut id: u32 = 0;
        let ds = i;
        while i < b.len() && b[i].is_ascii_digit() {
            id = id.wrapping_mul(10).wrapping_add((b[i] - b'0') as u32);
            i += 1;
        }
        if i == ds {
            out.errors.push((ent_start, "expected entity id"));
            i = resync(b, i);
            continue;
        }
        i = skip_ws_and_comments(b, i);
        if i >= b.len() || b[i] != b'=' {
            out.errors.push((ent_start, "expected '='"));
            i = resync(b, i);
            continue;
        }
        i = skip_ws_and_comments(b, i + 1);
        if i >= b.len() {
            break;
        }
        let rec;
        if b[i] == b'(' {
            // complex instance
            let Some(close) = find_close_paren(b, i) else {
                out.errors.push((ent_start, "unterminated complex instance"));
                break;
            };
            let parts = parse_complex_parts(b, i + 1, close);
            for p in &parts {
                if p.ty == EntityType::Other {
                    *out.other_names.entry(&b[p.name_start as usize..p.name_start as usize + p.name_len as usize]).or_insert(0) += 1;
                } else {
                    out.known_counts[p.ty as usize] += 1;
                }
            }
            out.complex.push(ComplexEntity { id: EntityId(id), parts });
            rec = EntityRec {
                args_start: (i + 1) as u32,
                args_len: (close - i - 1) as u32,
                ty: EntityType::Complex,
                flags: FLAG_COMPLEX,
                _pad: 0,
            };
            i = close + 1;
        } else {
            let ns = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let name = &b[ns..i];
            i = skip_ws_and_comments(b, i);
            if i >= b.len() || b[i] != b'(' {
                out.errors.push((ent_start, "expected '(' after type name"));
                i = resync(b, i);
                continue;
            }
            let Some(close) = find_close_paren(b, i) else {
                out.errors.push((ent_start, "unterminated attribute list"));
                break;
            };
            let ty = EntityType::from_name(name);
            if ty == EntityType::Other {
                *out.other_names.entry(name).or_insert(0) += 1;
            } else {
                out.known_counts[ty as usize] += 1;
            }
            rec = EntityRec { args_start: (i + 1) as u32, args_len: (close - i - 1) as u32, ty, flags: 0, _pad: 0 };
            i = close + 1;
        }
        i = skip_ws_and_comments(b, i);
        if i < b.len() && b[i] == b';' {
            i += 1;
        } else {
            out.errors.push((ent_start, "expected ';'"));
        }
        out.max_id = out.max_id.max(id);
        out.recs.push((id, rec));
    }
    out.end = i;
    out
}

/// Index the DATA section `b[data_start..data_end]` (exclusive of `ENDSEC;`).
pub(crate) fn index_data<'a>(b: &'a [u8], data_start: usize, data_end: usize, threads: usize) -> Result<Indexed<'a>> {
    let len = data_end.saturating_sub(data_start);
    if b.len() > u32::MAX as usize - 16 {
        return Err(Error::TooLarge(b.len() as u64));
    }
    let n_chunks = if len < 1 << 20 { 1 } else { (threads.max(1) * 4).min(len / (256 << 10)).max(1) };

    // chunk boundaries
    let mut bounds = Vec::with_capacity(n_chunks + 1);
    bounds.push(data_start);
    for k in 1..n_chunks {
        let nominal = data_start + len * k / n_chunks;
        let snapped = snap_to_entity_start(b, nominal).min(data_end);
        if snapped > *bounds.last().unwrap() {
            bounds.push(snapped);
        }
    }
    bounds.push(data_end);
    bounds.dedup();

    let ranges: Vec<(usize, usize)> = bounds.windows(2).map(|w| (w[0], w[1])).collect();
    let mut outs: Vec<ScanOut<'a>> =
        ranges.par_iter().map(|&(s, e)| scan_range(&b[..data_end.max(s)], s, e)).collect();

    // Validate chunk seams: a chunk's last entity must not run past the next chunk's start.
    let mut sequential_fallback = false;
    let seams_ok = outs.iter().zip(ranges.iter().skip(1)).all(|(o, &(next_start, _))| o.end <= next_start);
    if !seams_ok && ranges.len() > 1 {
        // Rare: a boundary landed inside a string literal. Redo in one pass.
        sequential_fallback = true;
        outs = vec![scan_range(&b[..data_end], data_start, data_end)];
    }

    let max_id = outs.iter().map(|o| o.max_id).max().unwrap_or(0);
    let total: usize = outs.iter().map(|o| o.recs.len()).sum();
    let mut table = vec![EntityRec::default(); max_id as usize + 1];
    let mut complex = Vec::with_capacity(outs.iter().map(|o| o.complex.len()).sum());
    let mut known_counts = vec![0u32; EntityType::ALL.len() + 3];
    let mut other_names: FxHashMap<&'a [u8], u32> = FxHashMap::default();
    let mut errors = Vec::new();
    for o in outs {
        for (id, rec) in o.recs {
            let slot = &mut table[id as usize];
            if slot.present() {
                errors.push((rec.args_start as usize, "duplicate entity id"));
            }
            *slot = rec;
        }
        complex.extend(o.complex);
        for (k, c) in o.known_counts.iter().enumerate() {
            known_counts[k] += c;
        }
        for (name, c) in o.other_names {
            *other_names.entry(name).or_insert(0) += c;
        }
        errors.extend(o.errors);
    }
    complex.sort_unstable_by_key(|c| c.id.0);
    let stats = IndexStats {
        entities: total as u32,
        complex: complex.len() as u32,
        data_bytes: len,
        chunks: ranges.len() as u32,
        errors: errors.len() as u32,
        sequential_fallback,
    };
    Ok(Indexed { table, complex, known_counts, other_names, errors, stats })
}
