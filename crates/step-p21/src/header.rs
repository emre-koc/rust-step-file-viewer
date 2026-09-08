//! HEADER section: FILE_DESCRIPTION, FILE_NAME, FILE_SCHEMA.

use crate::args::{Arg, Args, find_close_paren};
use crate::decode_string;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ap {
    Ap203,
    Ap214,
    Ap242,
    Unknown,
}

impl Ap {
    pub fn label(self) -> &'static str {
        match self {
            Ap::Ap203 => "AP203",
            Ap::Ap214 => "AP214",
            Ap::Ap242 => "AP242",
            Ap::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub description: Vec<String>,
    pub implementation_level: String,
    pub name: String,
    pub time_stamp: String,
    pub author: Vec<String>,
    pub organization: Vec<String>,
    pub preprocessor_version: String,
    pub originating_system: String,
    pub authorization: String,
    pub schema: Vec<String>,
}

impl Header {
    pub fn ap(&self) -> Ap {
        for s in &self.schema {
            let u = s.to_ascii_uppercase();
            if u.contains("AP242") {
                return Ap::Ap242;
            }
            if u.contains("AUTOMOTIVE_DESIGN") || u.contains("AP214") {
                return Ap::Ap214;
            }
            if u.contains("CONFIG_CONTROL_DESIGN") || u.contains("AP203") {
                return Ap::Ap203;
            }
        }
        Ap::Unknown
    }

    /// Parse the bytes between `HEADER;` and `ENDSEC;`.
    pub(crate) fn parse(b: &[u8]) -> Header {
        let mut h = Header::default();
        let mut i = 0;
        while i < b.len() {
            while i < b.len() && (crate::args::is_ws(b[i]) || b[i] == b';') {
                i += 1;
            }
            if i >= b.len() {
                break;
            }
            let ns = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let name = &b[ns..i];
            while i < b.len() && crate::args::is_ws(b[i]) {
                i += 1;
            }
            if i >= b.len() || b[i] != b'(' {
                i += 1;
                continue;
            }
            let Some(close) = find_close_paren(b, i) else { break };
            let args = Args::new(&b[i + 1..close]);
            match name {
                b"FILE_DESCRIPTION" => {
                    h.description = list_of_strings(args.nth(0));
                    h.implementation_level = string(args.nth(1));
                }
                b"FILE_NAME" => {
                    h.name = string(args.nth(0));
                    h.time_stamp = string(args.nth(1));
                    h.author = list_of_strings(args.nth(2));
                    h.organization = list_of_strings(args.nth(3));
                    h.preprocessor_version = string(args.nth(4));
                    h.originating_system = string(args.nth(5));
                    h.authorization = string(args.nth(6));
                }
                b"FILE_SCHEMA" => {
                    h.schema = list_of_strings(args.nth(0));
                }
                _ => {}
            }
            i = close + 1;
        }
        h
    }
}

fn string(a: Option<Arg<'_>>) -> String {
    a.and_then(|a| a.as_str_raw()).map(|s| decode_string(s).into_owned()).unwrap_or_default()
}

fn list_of_strings(a: Option<Arg<'_>>) -> Vec<String> {
    match a {
        Some(Arg::List(l)) => l.iter().filter_map(|x| x.as_str_raw()).map(|s| decode_string(s).into_owned()).collect(),
        Some(Arg::Str(s)) => vec![decode_string(s).into_owned()],
        _ => Vec::new(),
    }
}
