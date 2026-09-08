//! Part 21 string literal decoding.
//!
//! Raw content between the quotes may contain: `''` (a single quote), `\S\c` (ISO 8859 upper half,
//! `c + 0x80`), `\X\hh` (one ISO 8859-1 byte), `\X2\hhhh…\X0\` (UTF-16BE), `\X4\hhhhhhhh…\X0\`
//! (UTF-32BE), and `\P?\` page directives (ignored). Bytes ≥ 0x80 outside escapes are treated as
//! ISO 8859-1 (SolidWorks writes Latin-1 names directly).

use std::borrow::Cow;

/// Decode the raw bytes between the quotes of a Part 21 string.
pub fn decode_string(raw: &[u8]) -> Cow<'_, str> {
    if raw.iter().all(|&b| b < 0x80 && b != b'\\' && b != b'\'') {
        // Fast path: plain ASCII without escapes.
        // SAFETY-free: validated ASCII above.
        return Cow::Borrowed(std::str::from_utf8(raw).unwrap_or(""));
    }
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let b = raw[i];
        match b {
            b'\'' => {
                // `''` inside the literal is one quote; a lone quote should not occur but keep it.
                out.push('\'');
                i += if raw.get(i + 1) == Some(&b'\'') { 2 } else { 1 };
            }
            b'\\' => {
                if let Some((s, consumed)) = decode_escape(&raw[i..]) {
                    out.push_str(&s);
                    i += consumed;
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            0x80.. => {
                out.push(b as char); // ISO 8859-1
                i += 1;
            }
            _ => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

fn hex_val(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

fn parse_hex(bytes: &[u8]) -> Option<u32> {
    let mut v = 0u32;
    for &b in bytes {
        v = v.checked_mul(16)? + hex_val(b)?;
    }
    Some(v)
}

/// Decode one escape sequence starting at `\`. Returns the text and the number of bytes consumed.
fn decode_escape(s: &[u8]) -> Option<(String, usize)> {
    // s[0] == b'\\'
    let kind = *s.get(1)?;
    match kind {
        b'S' => {
            if s.get(2) != Some(&b'\\') {
                return None;
            }
            let c = *s.get(3)?;
            Some(((c.wrapping_add(0x80) as char).to_string(), 4))
        }
        b'X' => {
            match s.get(2)? {
                b'\\' => {
                    // \X\hh
                    let v = parse_hex(s.get(3..5)?)?;
                    Some(((v as u8 as char).to_string(), 5))
                }
                b'2' | b'4' => {
                    let width = if s[2] == b'2' { 4 } else { 8 };
                    if s.get(3) != Some(&b'\\') {
                        return None;
                    }
                    let mut i = 4;
                    let mut text = String::new();
                    let mut utf16: Vec<u16> = Vec::new();
                    loop {
                        if s.get(i..i + 4) == Some(b"\\X0\\") {
                            i += 4;
                            break;
                        }
                        let v = parse_hex(s.get(i..i + width)?)?;
                        if width == 4 {
                            utf16.push(v as u16);
                        } else {
                            text.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                        }
                        i += width;
                    }
                    if !utf16.is_empty() {
                        text.push_str(&String::from_utf16_lossy(&utf16));
                    }
                    Some((text, i))
                }
                _ => None,
            }
        }
        b'P' => {
            // \P?\ page directive: skip
            if s.get(3) == Some(&b'\\') { Some((String::new(), 4)) } else { None }
        }
        b'N' => {
            // \N\ newline
            if s.get(2) == Some(&b'\\') { Some(("\n".to_string(), 3)) } else { None }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        assert_eq!(decode_string(b"hello"), "hello");
        assert_eq!(decode_string(b""), "");
    }
    #[test]
    fn doubled_quote() {
        assert_eq!(decode_string(b"It''s"), "It's");
    }
    #[test]
    fn latin1_raw() {
        // 0xDD / 0xDE are Latin-1 Ý / Þ; the SolidWorks file is really Latin-9 Turkish but we
        // only promise a lossless, printable round trip.
        assert_eq!(decode_string(b"BEL\xDDRT"), "BEL\u{DD}RT");
    }
    #[test]
    fn x2_utf16() {
        assert_eq!(decode_string(b"\\X2\\00C4\\X0\\bc"), "Äbc");
    }
    #[test]
    fn x4_utf32() {
        assert_eq!(decode_string(b"a\\X4\\0001F600\\X0\\"), "a😀");
    }
    #[test]
    fn s_escape_and_x_escape() {
        assert_eq!(decode_string(b"\\S\\D"), "\u{C4}");
        assert_eq!(decode_string(b"\\X\\E9"), "é");
    }
    #[test]
    fn unknown_backslash_kept() {
        assert_eq!(decode_string(b"a\\b"), "a\\b");
    }
}
