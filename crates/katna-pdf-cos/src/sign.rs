// SPDX-License-Identifier: GPL-3.0-or-later

//! Signature fields (ISO 32000-2 §12.8; docs/PLAN.md §4.4): the PDF side
//! of signing and checking. No cryptography here: signing prepares the
//! file with room for the signature and says which bytes it covers;
//! `katna-pdf-sign` makes the signature (CMS) and puts it in.
//!
//! The signature goes in as an incremental update, so earlier
//! signatures stay valid, and covers the whole file but its own
//! `/Contents`.

use std::time::SystemTime;

use crate::annots::{pdf_date, reals};
use crate::forms::{literal, text_object};
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::write::{UpdateError, write_object};
use crate::{Document, FieldKind};

/// Room for the signature when none is asked for: enough for a signature
/// with a chain of a few certificates and a timestamp.
pub const CONTENTS_LEN: usize = 16 * 1024;
/// Bytes covered stand in for this until the file is written.
const PLACEHOLDER: i64 = 9_999_999_999;
/// Print and locked (ISO 32000-2 §12.5.3): signed fields stay as signed.
const SIGNED_WIDGET_FLAGS: i64 = 4 | 128;

/// What the signature dictionary says about the signing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SigningInfo {
    /// Who signs, as the certificate names them.
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
}

/// Where a signature is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// A new field with no appearance.
    Invisible,
    /// A new field on page `page` at `rect` (left, bottom, right, top in
    /// its user space), showing `lines`.
    Visible {
        page: usize,
        rect: [f32; 4],
        lines: Vec<String>,
    },
    /// An empty signature field already in the form, showing `lines`.
    Field { id: Ref, lines: Vec<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignRequest {
    pub placement: Placement,
    /// The new field's name; a free `Signature<n>` when `None`.
    pub field_name: Option<String>,
    pub info: SigningInfo,
    /// When the signing happens, for `/M`.
    pub time: SystemTime,
    /// Bytes kept for the signature.
    pub contents_len: usize,
}

impl SignRequest {
    pub fn new(placement: Placement, info: SigningInfo) -> Self {
        Self {
            placement,
            field_name: None,
            info,
            time: SystemTime::now(),
            contents_len: CONTENTS_LEN,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    Update(UpdateError),
    /// The page asked for is not in the file.
    NoPage,
    /// The field asked for is not an empty signature field of the form.
    NoField,
    /// The signature made is longer than the room kept for it.
    TooLong {
        length: usize,
        room: usize,
    },
}

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignError::Update(e) => e.fmt(f),
            SignError::NoPage => f.write_str("there is no such page"),
            SignError::NoField => f.write_str("there is no empty signature field there"),
            SignError::TooLong { length, room } => write!(
                f,
                "the signature takes {length} bytes; {room} were kept for it"
            ),
        }
    }
}

impl std::error::Error for SignError {}

impl From<UpdateError> for SignError {
    fn from(e: UpdateError) -> Self {
        SignError::Update(e)
    }
}

/// A file ready to be signed: everything written but the signature.
#[derive(Debug, Clone)]
pub struct Prepared {
    bytes: Vec<u8>,
    range: [usize; 4],
}

impl Prepared {
    /// The bytes the signature covers: all but the `/Contents` kept for
    /// it, as `/ByteRange` gives them.
    pub fn signed_parts(&self) -> [&[u8]; 2] {
        let [a, b, c, d] = self.range;
        [&self.bytes[a..a + b], &self.bytes[c..c + d]]
    }

    /// The bytes kept for the signature.
    pub fn room(&self) -> usize {
        (self.range[2] - self.range[1] - 2) / 2
    }

    /// The signed file, with `signature` (DER) in its `/Contents`.
    pub fn finish(mut self, signature: &[u8]) -> Result<Vec<u8>, SignError> {
        let room = self.room();
        if signature.len() > room {
            return Err(SignError::TooLong {
                length: signature.len(),
                room,
            });
        }
        let at = self.range[1] + 1;
        for (i, byte) in signature.iter().enumerate() {
            let hex = format!("{byte:02X}");
            self.bytes[at + 2 * i..at + 2 * i + 2].copy_from_slice(hex.as_bytes());
        }
        Ok(self.bytes)
    }
}

/// A signature field of the file and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub field: Ref,
    pub name: String,
    pub page: Option<usize>,
    pub rect: [f32; 4],
    /// `/SubFilter`: how the signature is made, e.g.
    /// `ETSI.CAdES.detached` or `adbe.pkcs7.detached`.
    pub sub_filter: String,
    pub byte_range: Vec<i64>,
    /// The signature, DER, without the zeros it was padded with.
    pub contents: Vec<u8>,
    pub info: SigningInfo,
    /// `/M`, as written.
    pub time: Option<String>,
}

/// How much of a file a signature covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// All of it but the signature itself.
    Whole,
    /// The file as it was up to this many bytes; what follows was added
    /// after signing, as further incremental updates.
    Revision(usize),
    /// Its byte range is not one a signature may have: it skips more
    /// than the signature, or points past the file.
    Broken,
}

impl Signature {
    /// The bytes of `file` the signature covers, if its byte range is
    /// sound.
    pub fn signed_parts<'f>(&self, file: &'f [u8]) -> Option<[&'f [u8]; 2]> {
        match self.coverage(file) {
            Coverage::Broken => None,
            _ => {
                let r: Vec<usize> = self.byte_range.iter().map(|&v| v as usize).collect();
                Some([&file[r[0]..r[0] + r[1]], &file[r[2]..r[2] + r[3]]])
            }
        }
    }

    pub fn coverage(&self, file: &[u8]) -> Coverage {
        let [a, b, c, d] = match self.byte_range[..] {
            [a, b, c, d] if a >= 0 && b >= 0 && c >= 0 && d >= 0 => {
                [a as usize, b as usize, c as usize, d as usize]
            }
            _ => return Coverage::Broken,
        };
        let end = c.saturating_add(d);
        // The gap is the signature's hex string and no more.
        let gap_is_contents = c > b
            && file.get(b) == Some(&b'<')
            && file.get(c - 1) == Some(&b'>')
            && file[b + 1..c - 1].iter().all(u8::is_ascii_hexdigit);
        if a != 0 || end > file.len() || !gap_is_contents {
            return Coverage::Broken;
        }
        if end == file.len() {
            Coverage::Whole
        } else {
            Coverage::Revision(end)
        }
    }
}

impl Document {
    /// The signature fields that hold a signature.
    pub fn signatures(&self) -> Vec<Signature> {
        let mut out = Vec::new();
        for field in self.form() {
            if field.kind != FieldKind::Signature {
                continue;
            }
            let Some(Object::Dict(dict)) = self.get(field.id) else {
                continue;
            };
            let Object::Dict(v) = self.resolve_key(&dict, "V") else {
                continue;
            };
            let text = |key: &str| match self.resolve_key(&v, key) {
                Object::String { bytes, .. } => Some(crate::text_string(&bytes)),
                _ => None,
            };
            let byte_range = match self.resolve_key(&v, "ByteRange") {
                Object::Array(a) => a.iter().filter_map(|o| self.resolve(o).as_int()).collect(),
                _ => Vec::new(),
            };
            let mut contents = match self.resolve_key(&v, "Contents") {
                Object::String { bytes, .. } => bytes,
                _ => Vec::new(),
            };
            trim_padding(&mut contents);
            let widget = field.widgets.first();
            out.push(Signature {
                field: field.id,
                name: field.name.clone(),
                page: widget.map(|w| w.page),
                rect: widget.map_or([0.0; 4], |w| w.rect),
                sub_filter: match self.resolve_key(&v, "SubFilter") {
                    Object::Name(n) => String::from_utf8_lossy(n.as_bytes()).into_owned(),
                    _ => String::new(),
                },
                byte_range,
                contents,
                info: SigningInfo {
                    name: text("Name"),
                    reason: text("Reason"),
                    location: text("Location"),
                    contact: text("ContactInfo"),
                },
                time: text("M"),
            });
        }
        out
    }

    /// The file with a signature field filled but for the signature
    /// itself, as an incremental update.
    pub fn prepare_signature(&self, request: &SignRequest) -> Result<Prepared, SignError> {
        let mut update = self.update()?;
        let pages = self.pages();
        let mut sig = Dict::new();
        sig.set("Type", Object::name("Sig"));
        sig.set("Filter", Object::name("Adobe.PPKLite"));
        sig.set("SubFilter", Object::name("ETSI.CAdES.detached"));
        sig.set(
            "ByteRange",
            vec![
                Object::Int(0),
                Object::Int(PLACEHOLDER),
                Object::Int(PLACEHOLDER),
                Object::Int(PLACEHOLDER),
            ],
        );
        sig.set(
            "Contents",
            Object::String {
                bytes: vec![0; request.contents_len],
                hex: true,
            },
        );
        sig.set("M", text_object(&pdf_date(request.time)));
        let info = &request.info;
        for (key, value) in [
            ("Name", &info.name),
            ("Reason", &info.reason),
            ("Location", &info.location),
            ("ContactInfo", &info.contact),
        ] {
            if let Some(value) = value {
                sig.set(key, text_object(value));
            }
        }
        let sig = update.add(Object::Dict(sig));

        let form_ref = self
            .catalog()
            .and_then(|c| c.get("AcroForm").and_then(Object::as_ref));
        let mut form = self.acro_form().unwrap_or_default();
        let mut fields = match self.resolve_key(&form, "Fields") {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        match &request.placement {
            Placement::Field { id, lines } => {
                let field = self
                    .form()
                    .into_iter()
                    .find(|f| f.id == *id && f.kind == FieldKind::Signature)
                    .ok_or(SignError::NoField)?;
                let Some(Object::Dict(mut dict)) = self.get(field.id) else {
                    return Err(SignError::NoField);
                };
                if dict.get("V").is_some() {
                    return Err(SignError::NoField);
                }
                dict.set("V", Object::Ref(sig));
                for widget in &field.widgets {
                    let mut wdict = if widget.id == field.id {
                        dict.clone()
                    } else {
                        match self.get(widget.id) {
                            Some(Object::Dict(d)) => d,
                            _ => continue,
                        }
                    };
                    let ap = update.add(appearance(widget.rect, lines));
                    wdict.set("AP", Dict(vec![(Name::new("N"), Object::Ref(ap))]));
                    let flags = wdict.get("F").and_then(Object::as_int).unwrap_or(0);
                    wdict.set("F", flags | SIGNED_WIDGET_FLAGS);
                    if widget.id == field.id {
                        dict = wdict;
                    } else {
                        update.replace(widget.id, Object::Dict(wdict));
                    }
                }
                update.replace(field.id, Object::Dict(dict));
            }
            Placement::Invisible | Placement::Visible { .. } => {
                let (page, rect, lines) = match &request.placement {
                    Placement::Visible { page, rect, lines } => (*page, *rect, lines.as_slice()),
                    _ => (0, [0.0; 4], &[][..]),
                };
                let &page_ref = pages.get(page).ok_or(SignError::NoPage)?;
                let taken: Vec<String> = self.form().into_iter().map(|f| f.name).collect();
                let name = request.field_name.clone().unwrap_or_else(|| {
                    (1..)
                        .map(|n| format!("Signature{n}"))
                        .find(|n| !taken.contains(n))
                        .unwrap_or_default()
                });
                let mut widget = Dict::new();
                widget.set("Type", Object::name("Annot"));
                widget.set("Subtype", Object::name("Widget"));
                widget.set("FT", Object::name("Sig"));
                widget.set("T", text_object(&name));
                widget.set("F", SIGNED_WIDGET_FLAGS);
                widget.set("Rect", reals(&rect));
                widget.set("P", Object::Ref(page_ref));
                widget.set("V", Object::Ref(sig));
                if rect[2] > rect[0] && rect[3] > rect[1] {
                    let ap = update.add(appearance(rect, lines));
                    widget.set("AP", Dict(vec![(Name::new("N"), Object::Ref(ap))]));
                }
                let widget = update.add(Object::Dict(widget));
                fields.push(Object::Ref(widget));
                // On the page.
                let Some(Object::Dict(mut page_dict)) = self.get(page_ref) else {
                    return Err(SignError::NoPage);
                };
                let mut annots = match self.resolve_key(&page_dict, "Annots") {
                    Object::Array(a) => a,
                    _ => Vec::new(),
                };
                annots.push(Object::Ref(widget));
                match page_dict.get("Annots").and_then(Object::as_ref) {
                    Some(list) => update.replace(list, Object::Array(annots)),
                    None => {
                        page_dict.set("Annots", annots);
                        update.replace(page_ref, Object::Dict(page_dict));
                    }
                }
                // In the form.
                match form.get("Fields").and_then(Object::as_ref) {
                    Some(list) => update.replace(list, Object::Array(fields)),
                    None => form.set("Fields", fields),
                }
            }
        }
        // Signatures present, and changes appended only (ISO 32000-2
        // §12.7.3).
        form.set("SigFlags", 3);
        match form_ref {
            Some(r) => update.replace(r, Object::Dict(form)),
            None => {
                let catalog_ref = self
                    .trailer()
                    .get("Root")
                    .and_then(Object::as_ref)
                    .ok_or(SignError::Update(UpdateError::Repaired))?;
                let mut catalog = self.catalog().unwrap_or_default();
                catalog.set("AcroForm", Object::Dict(form));
                update.replace(catalog_ref, Object::Dict(catalog));
            }
        }

        let start = self.bytes().len();
        let mut bytes = update.finish();
        let mut placeholder = Vec::new();
        write_object(
            &mut placeholder,
            &Object::Array(vec![
                Object::Int(0),
                Object::Int(PLACEHOLDER),
                Object::Int(PLACEHOLDER),
                Object::Int(PLACEHOLDER),
            ]),
        );
        let range_at = find(&bytes, start, &placeholder).ok_or(SignError::NoField)?;
        let mut contents = vec![b'<'];
        contents.resize(1 + 2 * request.contents_len, b'0');
        contents.push(b'>');
        let contents_at = find(&bytes, start, &contents).ok_or(SignError::NoField)?;
        let after = contents_at + contents.len();
        let range = [0, contents_at, after, bytes.len() - after];
        let mut written = format!("[0 {} {} {}", range[1], range[2], range[3]).into_bytes();
        written.resize(placeholder.len() - 1, b' ');
        written.push(b']');
        bytes[range_at..range_at + placeholder.len()].copy_from_slice(&written);
        Ok(Prepared { bytes, range })
    }
}

/// Where `needle` first occurs in `haystack` from `from`.
fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// A DER signature stored with zeros after it: the zeros dropped, by the
/// lengths DER gives.
fn trim_padding(contents: &mut Vec<u8>) {
    if contents.iter().all(|&b| b == 0) {
        contents.clear();
        return;
    }
    let len = match contents.get(1..) {
        Some([n, ..]) if n & 0x80 == 0 => Some(2 + *n as usize),
        Some([n, rest @ ..]) => {
            let count = (n & 0x7f) as usize;
            rest.get(..count)
                .filter(|b| count <= 4 && !b.is_empty())
                .map(|b| 2 + count + b.iter().fold(0usize, |acc, &x| acc << 8 | x as usize))
        }
        _ => None,
    };
    if let Some(len) = len.filter(|&l| l <= contents.len()) {
        contents.truncate(len);
    }
}

/// A signature's appearance: `lines` of text fitted into `rect`.
fn appearance(rect: [f32; 4], lines: &[String]) -> Object {
    let (w, h) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
    let mut content = String::new();
    if !lines.is_empty() && w > 0.0 && h > 0.0 {
        // Helvetica's average width is about half its size.
        let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
        let pad = (h.min(w) * 0.06).clamp(1.0, 4.0);
        let size = ((h - 2.0 * pad) / (lines.len() as f32 * 1.2))
            .min((w - 2.0 * pad) / (longest.max(1.0) * 0.52))
            .clamp(2.0, 12.0);
        let lead = size * 1.2;
        content.push_str(&format!(
            "BT /Helv {size:.2} Tf 0 g {lead:.2} TL {pad:.2} {:.2} Td\n",
            h - pad - size
        ));
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                content.push_str("T* ");
            }
            content.push_str(&literal(line));
            content.push_str(" Tj\n");
        }
        content.push_str("ET\n");
    }
    let font = Dict(vec![
        (Name::new("Type"), Object::name("Font")),
        (Name::new("Subtype"), Object::name("Type1")),
        (Name::new("BaseFont"), Object::name("Helvetica")),
        (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
    ]);
    let resources = Dict(vec![(
        Name::new("Font"),
        Object::Dict(Dict(vec![(Name::new("Helv"), Object::Dict(font))])),
    )]);
    let dict = Dict(vec![
        (Name::new("Type"), Object::name("XObject")),
        (Name::new("Subtype"), Object::name("Form")),
        (Name::new("BBox"), reals(&[0.0, 0.0, w, h])),
        (Name::new("Resources"), Object::Dict(resources)),
    ]);
    Object::Stream(Stream {
        dict,
        data: content.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::trim_padding;

    #[test]
    fn padding_after_der_is_dropped() {
        let mut short = vec![0x30, 0x03, 1, 2, 3, 0, 0, 0];
        trim_padding(&mut short);
        assert_eq!(short, [0x30, 0x03, 1, 2, 3]);
        let mut long = vec![0x30, 0x82, 0x00, 0x02, 9, 9, 0, 0];
        trim_padding(&mut long);
        assert_eq!(long, [0x30, 0x82, 0x00, 0x02, 9, 9]);
        // Room kept, nothing signed.
        let mut zeros = vec![0; 4];
        trim_padding(&mut zeros);
        assert!(zeros.is_empty());
    }
}
