//! Zero-copy, lazy cursor over the attribute list of one entity instance.
//!
//! `Args` wraps the bytes between the outer parentheses; iterating yields [`Arg`]s without any
//! allocation. Lists and typed values are returned as nested `Args` slices, parsed only if visited.

use crate::EntityId;
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args<'a> {
    bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arg<'a> {
    /// `#123`
    Ref(EntityId),
    /// `1.5E-3`, `0.E0`
    Real(f64),
    /// `42`
    Int(i64),
    /// Raw bytes between the quotes; decode with [`crate::decode_string`].
    Str(&'a [u8]),
    /// `.T.`, `.UNSPECIFIED.` → the bytes between the dots.
    Enum(&'a [u8]),
    /// `( ... )`
    List(Args<'a>),
    /// `LENGTH_MEASURE(1.E-3)` → (`LENGTH_MEASURE`, inner args)
    Typed(&'a [u8], Args<'a>),
    /// `$`
    Null,
    /// `*`
    Derived,
    /// Bytes that could not be classified.
    Invalid(&'a [u8]),
}

impl<'a> Args<'a> {
    #[inline]
    pub fn new(inner: &'a [u8]) -> Self {
        Args { bytes: inner }
    }

    #[inline]
    pub fn raw(&self) -> &'a [u8] {
        self.bytes
    }

    #[inline]
    pub fn iter(&self) -> ArgIter<'a> {
        ArgIter { bytes: self.bytes, pos: 0 }
    }

    /// Number of top-level arguments (walks the list).
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// `i`-th argument (O(i)).
    pub fn nth(&self, i: usize) -> Option<Arg<'a>> {
        self.iter().nth(i)
    }

    pub fn collect(&self) -> Vec<Arg<'a>> {
        self.iter().collect()
    }

    /// Entity references among the top-level arguments (non-refs skipped).
    pub fn refs(&self) -> impl Iterator<Item = EntityId> + 'a {
        self.iter().filter_map(|a| if let Arg::Ref(r) = a { Some(r) } else { None })
    }

    /// All top-level arguments as reals (ints promoted). Errors on anything else.
    pub fn reals(&self) -> Result<Vec<f64>> {
        self.iter().map(|a| a.as_f64().ok_or(Error::Arg("expected real"))).collect()
    }

    pub fn ints(&self) -> Result<Vec<i64>> {
        self.iter().map(|a| a.as_i64().ok_or(Error::Arg("expected integer"))).collect()
    }

    /// Exactly three reals `(x, y, z)`; a 2-tuple is zero-extended.
    pub fn f64x3(&self) -> Result<[f64; 3]> {
        let mut out = [0.0; 3];
        let mut n = 0;
        for a in self.iter() {
            if n >= 3 {
                return Err(Error::Arg("more than 3 coordinates"));
            }
            out[n] = a.as_f64().ok_or(Error::Arg("expected real coordinate"))?;
            n += 1;
        }
        if n < 2 {
            return Err(Error::Arg("fewer than 2 coordinates"));
        }
        Ok(out)
    }
}

impl<'a> Arg<'a> {
    #[inline]
    pub fn as_ref(&self) -> Option<EntityId> {
        if let Arg::Ref(r) = self { Some(*r) } else { None }
    }
    #[inline]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Arg::Real(v) => Some(*v),
            Arg::Int(i) => Some(*i as f64),
            Arg::Typed(_, inner) => inner.nth(0).and_then(|a| a.as_f64()),
            _ => None,
        }
    }
    #[inline]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Arg::Int(i) => Some(*i),
            Arg::Real(v) if v.fract() == 0.0 => Some(*v as i64),
            _ => None,
        }
    }
    #[inline]
    pub fn as_str_raw(&self) -> Option<&'a [u8]> {
        if let Arg::Str(s) = self { Some(s) } else { None }
    }
    /// Decoded string (escapes resolved).
    pub fn as_string(&self) -> Option<String> {
        self.as_str_raw().map(|s| crate::decode_string(s).into_owned())
    }
    #[inline]
    pub fn as_list(&self) -> Option<Args<'a>> {
        if let Arg::List(l) = self { Some(*l) } else { None }
    }
    #[inline]
    pub fn as_enum(&self) -> Option<&'a [u8]> {
        if let Arg::Enum(e) = self { Some(e) } else { None }
    }
    /// `.T.` → true, `.F.` → false.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_enum()? {
            b"T" => Some(true),
            b"F" => Some(false),
            _ => None,
        }
    }
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Arg::Null | Arg::Derived)
    }
    pub fn as_typed(&self) -> Option<(&'a [u8], Args<'a>)> {
        if let Arg::Typed(n, a) = self { Some((n, *a)) } else { None }
    }
}

pub struct ArgIter<'a> {
    bytes: &'a [u8],
    pos: usize,
}

#[inline]
pub(crate) fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c)
}

impl<'a> ArgIter<'a> {
    #[inline]
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && is_ws(self.bytes[self.pos]) {
            self.pos += 1;
        }
    }
}

impl<'a> Iterator for ArgIter<'a> {
    type Item = Arg<'a>;

    fn next(&mut self) -> Option<Arg<'a>> {
        self.skip_ws();
        let b = self.bytes;
        if self.pos >= b.len() {
            return None;
        }
        let start = self.pos;
        let arg = match b[start] {
            b'#' => {
                let mut i = start + 1;
                let mut id: u32 = 0;
                while i < b.len() && b[i].is_ascii_digit() {
                    id = id.wrapping_mul(10).wrapping_add((b[i] - b'0') as u32);
                    i += 1;
                }
                self.pos = i;
                Arg::Ref(EntityId(id))
            }
            b'\'' => {
                let end = find_string_end(b, start).unwrap_or(b.len());
                self.pos = (end + 1).min(b.len());
                Arg::Str(&b[start + 1..end])
            }
            b'.' => {
                let rest = &b[start + 1..];
                let end = memchr::memchr(b'.', rest).map(|j| start + 1 + j).unwrap_or(b.len());
                self.pos = (end + 1).min(b.len());
                Arg::Enum(&b[start + 1..end])
            }
            b'$' => {
                self.pos += 1;
                Arg::Null
            }
            b'*' => {
                self.pos += 1;
                Arg::Derived
            }
            b'(' => {
                let close = find_close_paren(b, start).unwrap_or(b.len());
                self.pos = (close + 1).min(b.len());
                Arg::List(Args::new(&b[start + 1..close.min(b.len())]))
            }
            c if c.is_ascii_uppercase() || c == b'_' => {
                let mut i = start;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let name = &b[start..i];
                let mut j = i;
                while j < b.len() && is_ws(b[j]) {
                    j += 1;
                }
                if j < b.len() && b[j] == b'(' {
                    let close = find_close_paren(b, j).unwrap_or(b.len());
                    self.pos = (close + 1).min(b.len());
                    Arg::Typed(name, Args::new(&b[j + 1..close.min(b.len())]))
                } else {
                    self.pos = i;
                    Arg::Typed(name, Args::new(&[]))
                }
            }
            _ => {
                // number (or garbage): run to the next separator
                let mut i = start;
                while i < b.len() && !matches!(b[i], b',' | b')' | b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                self.pos = i;
                parse_number(&b[start..i])
            }
        };
        // consume the separator
        self.skip_ws();
        if self.pos < b.len() && b[self.pos] == b',' {
            self.pos += 1;
        }
        Some(arg)
    }
}

/// Parse an integer or real token.
pub fn parse_number(tok: &[u8]) -> Arg<'_> {
    if tok.is_empty() {
        return Arg::Invalid(tok);
    }
    let is_real = tok.iter().any(|&c| matches!(c, b'.' | b'E' | b'e'));
    if !is_real {
        let mut v: i64 = 0;
        let (neg, digits) = match tok[0] {
            b'-' => (true, &tok[1..]),
            b'+' => (false, &tok[1..]),
            _ => (false, tok),
        };
        if digits.is_empty() {
            return Arg::Invalid(tok);
        }
        for &c in digits {
            if !c.is_ascii_digit() {
                return Arg::Invalid(tok);
            }
            v = v.wrapping_mul(10).wrapping_add((c - b'0') as i64);
        }
        return Arg::Int(if neg { -v } else { v });
    }
    match parse_real(tok) {
        Some(v) => Arg::Real(v),
        None => Arg::Invalid(tok),
    }
}

/// Parse a Part 21 real. Handles `1.E0`, `0.E0`, `-1.E-5`, `.5`, `1.` forms.
pub fn parse_real(tok: &[u8]) -> Option<f64> {
    if let Ok(v) = fast_float2::parse::<f64, _>(tok) {
        return Some(v);
    }
    // Normalise unusual forms into a small buffer: insert 0 after a bare '.', before a bare 'E'.
    let mut buf = [0u8; 64];
    let mut n = 0;
    let mut prev_dot = false;
    for &c in tok {
        if n + 2 >= buf.len() {
            return None;
        }
        if prev_dot && !c.is_ascii_digit() {
            buf[n] = b'0';
            n += 1;
        }
        buf[n] = if c == b'e' { b'E' } else { c };
        n += 1;
        prev_dot = c == b'.';
    }
    if prev_dot {
        buf[n] = b'0';
        n += 1;
    }
    fast_float2::parse::<f64, _>(&buf[..n]).ok()
}

/// Index of the closing quote of the string starting at `open` (a `'`), treating `''` as an escaped
/// quote. Returns `None` if unterminated.
#[inline]
pub fn find_string_end(b: &[u8], open: usize) -> Option<usize> {
    let mut i = open + 1;
    loop {
        let j = memchr::memchr(b'\'', &b[i..])? + i;
        if b.get(j + 1) == Some(&b'\'') {
            i = j + 2;
        } else {
            return Some(j);
        }
    }
}

/// Index of the `)` matching the `(` at `open`, skipping string literals.
pub fn find_close_paren(b: &[u8], open: usize) -> Option<usize> {
    let mut depth: u32 = 1;
    let mut i = open + 1;
    loop {
        let j = memchr::memchr3(b'(', b')', b'\'', &b[i..])? + i;
        match b[j] {
            b'(' => {
                depth += 1;
                i = j + 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
                i = j + 1;
            }
            _ => {
                // string: skip to its end (the `''` escape works out because the second quote
                // simply opens and the next one closes again)
                let end = memchr::memchr(b'\'', &b[j + 1..])? + j + 1;
                i = end + 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<Arg<'_>> {
        Args::new(s.as_bytes()).collect()
    }

    #[test]
    fn creo_compact_form() {
        let a = args("'',(#364,#370),#338,.F.");
        assert_eq!(a[0], Arg::Str(b""));
        let list = a[1].as_list().unwrap();
        assert_eq!(list.refs().collect::<Vec<_>>(), vec![EntityId(364), EntityId(370)]);
        assert_eq!(a[2], Arg::Ref(EntityId(338)));
        assert_eq!(a[3].as_bool(), Some(false));
    }

    #[test]
    fn solidworks_spaced_form() {
        let a = args(" 'NONE',  ( -17.30715500759693271, 1.179947487346378221, -1.386488357672962968 ) ");
        assert_eq!(a[0], Arg::Str(b"NONE"));
        let xyz = a[1].as_list().unwrap().f64x3().unwrap();
        assert!((xyz[0] + 17.30715500759693271).abs() < 1e-15);
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_number(b"0.E0"), Arg::Real(0.0));
        assert_eq!(parse_number(b"-1.E-5"), Arg::Real(-1e-5));
        assert_eq!(parse_number(b"1.000000000000000082E-05"), Arg::Real(1.000000000000000082E-05));
        assert_eq!(parse_number(b"2.54E1"), Arg::Real(25.4));
        assert_eq!(parse_number(b"42"), Arg::Int(42));
        assert_eq!(parse_number(b"-7"), Arg::Int(-7));
        assert_eq!(parse_number(b".5"), Arg::Real(0.5));
        assert_eq!(parse_number(b"1."), Arg::Real(1.0));
        assert!(matches!(parse_number(b"abc"), Arg::Invalid(_)));
    }

    #[test]
    fn typed_null_derived_enum() {
        let a = args("LENGTH_MEASURE(1.E-3),#662,'closure',$,*,.UNSPECIFIED.");
        let (name, inner) = a[0].as_typed().unwrap();
        assert_eq!(name, b"LENGTH_MEASURE");
        assert_eq!(inner.nth(0).unwrap(), Arg::Real(1e-3));
        assert_eq!(a[0].as_f64(), Some(1e-3));
        assert_eq!(a[3], Arg::Null);
        assert_eq!(a[4], Arg::Derived);
        assert_eq!(a[5].as_enum(), Some(&b"UNSPECIFIED"[..]));
    }

    #[test]
    fn strings_with_quotes_parens_and_hashes() {
        let a = args("'a''b',(#1),'x(y)#2,z',#3");
        assert_eq!(a[0], Arg::Str(b"a''b"));
        assert_eq!(a[0].as_string().unwrap(), "a'b");
        assert_eq!(a[2], Arg::Str(b"x(y)#2,z"));
        assert_eq!(a[3], Arg::Ref(EntityId(3)));
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn nested_lists_depth_4() {
        let a = args("(((1,2),(3,4)),((5,6)))");
        let outer = a[0].as_list().unwrap();
        let first = outer.nth(0).unwrap().as_list().unwrap();
        let pair = first.nth(1).unwrap().as_list().unwrap();
        assert_eq!(pair.ints().unwrap(), vec![3, 4]);
    }

    #[test]
    fn multiline_with_crlf() {
        let a = args("'',3,(#1167,#1168,\r\n#1169),\r\n.UNSPECIFIED.,.F.,.F.,(4,1,1,4),(0.E0,3.333333333333E-1,1.E0),\r\n.UNSPECIFIED.");
        assert_eq!(a.len(), 9);
        assert_eq!(a[2].as_list().unwrap().refs().count(), 3);
        assert_eq!(a[7].as_list().unwrap().reals().unwrap().len(), 3);
    }
}
