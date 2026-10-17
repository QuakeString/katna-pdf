// SPDX-License-Identifier: GPL-3.0-or-later

//! Writing objects, and incremental updates (ISO 32000-2 §7.5.6): new and
//! changed objects are appended after the original bytes, which stay
//! exactly as they were. That is what keeps earlier signatures valid.

use std::collections::BTreeMap;
use std::io::Write as _;

use crate::document::{Document, XrefKind};
use crate::object::{Dict, Name, Object, Ref};
use crate::parse::{is_delim, is_white};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Writing into an encrypted file needs its key (not in v0).
    Encrypted,
    /// The file's cross-reference data was broken; appending to it would
    /// leave a file other readers may not open. Save a full copy instead.
    Repaired,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UpdateError::Encrypted => "the PDF is encrypted",
            UpdateError::Repaired => "the PDF's cross-reference data is damaged",
        })
    }
}

impl std::error::Error for UpdateError {}

/// Serialises `obj` as PDF syntax.
pub fn write_object(out: &mut Vec<u8>, obj: &Object) {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Object::Real(r) => write_real(out, *r),
        Object::String { bytes, hex } => write_string(out, bytes, *hex),
        Object::Name(n) => write_name(out, n),
        Object::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                write_object(out, item);
            }
            out.push(b']');
        }
        Object::Dict(d) => write_dict(out, d),
        Object::Stream(s) => {
            let mut dict = s.dict.clone();
            dict.set("Length", s.data.len() as i64);
            write_dict(out, &dict);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.data);
            out.extend_from_slice(b"\nendstream");
        }
        Object::Ref(r) => {
            let _ = write!(out, "{} {} R", r.num, r.generation);
        }
    }
}

fn write_dict(out: &mut Vec<u8>, d: &Dict) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        write_name(out, k);
        out.push(b' ');
        write_object(out, v);
    }
    out.extend_from_slice(b">>");
}

fn write_real(out: &mut Vec<u8>, r: f64) {
    if !r.is_finite() {
        out.push(b'0');
        return;
    }
    // PDF has no exponents: fixed point, trailing zeros trimmed.
    let s = format!("{r:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    out.extend_from_slice(if s.is_empty() || s == "-" {
        b"0"
    } else {
        s.as_bytes()
    });
}

fn write_name(out: &mut Vec<u8>, n: &Name) {
    out.push(b'/');
    for &b in n.as_bytes() {
        if b == b'#' || is_white(b) || is_delim(b) || !(b'!'..=b'~').contains(&b) {
            let _ = write!(out, "#{b:02X}");
        } else {
            out.push(b);
        }
    }
}

fn write_string(out: &mut Vec<u8>, bytes: &[u8], hex: bool) {
    if hex {
        out.push(b'<');
        for b in bytes {
            let _ = write!(out, "{b:02X}");
        }
        out.push(b'>');
        return;
    }
    out.push(b'(');
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                out.push(b'\\');
                out.push(b);
            }
            b'\r' => out.extend_from_slice(b"\\r"),
            _ => out.push(b),
        }
    }
    out.push(b')');
}

/// Changes to append to a document.
pub struct Update<'a> {
    doc: &'a Document,
    next: u32,
    objects: BTreeMap<u32, (u16, Object)>,
    trailer: Dict,
}

impl Document {
    /// Starts an incremental update of this document.
    pub fn update(&self) -> Result<Update<'_>, UpdateError> {
        if self.is_encrypted() {
            return Err(UpdateError::Encrypted);
        }
        if self.was_repaired() {
            return Err(UpdateError::Repaired);
        }
        Ok(Update {
            doc: self,
            next: self.size(),
            objects: BTreeMap::new(),
            trailer: self.trailer().clone(),
        })
    }
}

impl Update<'_> {
    /// Adds a new object and returns its reference.
    pub fn add(&mut self, obj: Object) -> Ref {
        let r = Ref::new(self.next, 0);
        self.next += 1;
        self.objects.insert(r.num, (0, obj));
        r
    }

    /// Replaces object `r` (keeping its number and generation).
    pub fn replace(&mut self, r: Ref, obj: Object) {
        self.objects.insert(r.num, (r.generation, obj));
    }

    /// The trailer the update will write; `/Size`, `/Prev` and `/ID` are
    /// set by [`Update::finish`].
    pub fn trailer_mut(&mut self) -> &mut Dict {
        &mut self.trailer
    }

    /// The original bytes with the update appended.
    pub fn finish(self) -> Vec<u8> {
        let mut out = self.doc.bytes().to_vec();
        if !out.ends_with(b"\n") && !out.ends_with(b"\r") {
            out.push(b'\n');
        }
        let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
        for (&num, (generation, obj)) in &self.objects {
            offsets.insert(num, (out.len(), *generation));
            let _ = writeln!(out, "{num} {generation} obj");
            write_object(&mut out, obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let mut trailer = self.trailer;
        trailer.set("Size", i64::from(self.next));
        trailer.set("Prev", self.doc.startxref() as i64);
        // Keep the file's first ID and give this revision a new second one.
        if let Some(Object::Array(id)) = trailer.get("ID").cloned()
            && let Some(first) = id.first()
        {
            let second = revision_id(&out);
            trailer.set(
                "ID",
                vec![
                    first.clone(),
                    Object::String {
                        bytes: second,
                        hex: true,
                    },
                ],
            );
        }
        let xref_at = out.len();
        match self.doc.xref_kind() {
            XrefKind::Table => {
                out.extend_from_slice(b"xref\n");
                for run in runs(&offsets) {
                    let _ = writeln!(out, "{} {}", run[0].0, run.len());
                    for (_, (off, generation)) in run {
                        let _ = writeln!(out, "{off:010} {generation:05} n\r");
                    }
                }
                out.extend_from_slice(b"trailer\n");
                write_dict(&mut out, &trailer);
                out.push(b'\n');
            }
            XrefKind::Stream => {
                // The xref stream describes itself too.
                let own = self.next;
                trailer.set("Size", i64::from(own) + 1);
                offsets.insert(own, (xref_at, 0));
                let mut index = Vec::new();
                let mut rows = Vec::new();
                for run in runs(&offsets) {
                    index.push(Object::Int(i64::from(run[0].0)));
                    index.push(Object::Int(run.len() as i64));
                    for (_, (off, generation)) in run {
                        rows.push(1u8);
                        rows.extend_from_slice(&(off as u32).to_be_bytes());
                        rows.extend_from_slice(&generation.to_be_bytes());
                    }
                }
                let mut dict = trailer;
                dict.set("Type", Object::name("XRef"));
                dict.set("W", vec![Object::Int(1), Object::Int(4), Object::Int(2)]);
                dict.set("Index", index);
                let _ = writeln!(out, "{own} 0 obj");
                write_object(
                    &mut out,
                    &Object::Stream(crate::object::Stream { dict, data: rows }),
                );
                out.extend_from_slice(b"\nendobj\n");
            }
        }
        let _ = writeln!(out, "startxref\n{xref_at}\n%%EOF");
        out
    }
}

/// Consecutive object numbers, grouped for the subsections of a table.
fn runs(offsets: &BTreeMap<u32, (usize, u16)>) -> Vec<Vec<(u32, (usize, u16))>> {
    let mut out: Vec<Vec<(u32, (usize, u16))>> = Vec::new();
    for (&num, &v) in offsets {
        match out.last_mut() {
            Some(run) if run.last().is_some_and(|(n, _)| n + 1 == num) => run.push((num, v)),
            _ => out.push(vec![(num, v)]),
        }
    }
    out
}

/// A 16-byte identifier for this revision, from its contents.
fn revision_id(bytes: &[u8]) -> Vec<u8> {
    // FNV-1a, twice with different seeds: unique enough for an ID, and
    // no hashing dependency.
    let fnv = |seed: u64| {
        bytes
            .iter()
            .fold(seed, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
    };
    let mut id = fnv(0xcbf29ce484222325).to_be_bytes().to_vec();
    id.extend_from_slice(&fnv(0x84222325cbf29ce4 ^ bytes.len() as u64).to_be_bytes());
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(obj: &Object) -> String {
        let mut out = Vec::new();
        write_object(&mut out, obj);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn writes_objects() {
        let mut d = Dict::new();
        d.set("Type", Object::name("Annot"));
        d.set(
            "Rect",
            vec![Object::Int(0), Object::Real(1.5), Object::Real(-0.25)],
        );
        d.set("T", Object::string("a(b)"));
        d.set("P", Ref::new(3, 0));
        assert_eq!(
            text(&Object::Dict(d)),
            "<</Type /Annot/Rect [0 1.5 -0.25]/T (a\\(b\\))/P 3 0 R>>"
        );
        assert_eq!(text(&Object::Name(Name(b"A B#".to_vec()))), "/A#20B#23");
        assert_eq!(text(&Object::Real(1e20)), "100000000000000000000");
    }
}
