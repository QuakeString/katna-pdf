// SPDX-License-Identifier: GPL-3.0-or-later

//! What a form's buttons do when clicked (`/A` on their widgets): go to
//! a page, open an address, reset the form, hide or show fields, send
//! the form's data somewhere; read, written, and the parts the reader
//! can do done here (which fields to reset, which to hide).

use crate::forms::text_object;
use crate::object::{Dict, Object, Ref};
use crate::outline::{Destination, OutlineReader, text_string};
use crate::{Document, Field, FieldValue, NavError, UpdateError};

/// What a field does when clicked.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldAction {
    GoTo(Destination),
    /// A web or mail address to open.
    Uri(String),
    /// Puts fields back to their defaults: those named, or with
    /// `others` every field but those; none named means every field.
    Reset {
        fields: Vec<String>,
        others: bool,
    },
    /// Hides (or with `hide` false shows) the fields named.
    Hide {
        fields: Vec<String>,
        hide: bool,
    },
    /// Sends the form's data to `url`.
    Submit {
        url: String,
        format: SubmitFormat,
    },
    /// A named action: `NextPage`, `PrevPage`, `FirstPage`, `LastPage`.
    Named(String),
    /// JavaScript, which is not run.
    JavaScript(String),
}

/// How a submit action sends the data (`/Flags` of SubmitForm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitFormat {
    Fdf,
    Html,
    Xfdf,
    /// The whole file.
    Pdf,
}

const INCLUDE_EXCLUDE: i64 = 1;
const EXPORT_HTML: i64 = 1 << 2;
const EXPORT_XFDF: i64 = 1 << 5;
const SUBMIT_PDF: i64 = 1 << 8;
/// Hidden (`/F`).
const HIDDEN: i64 = 2;

impl Document {
    /// What field `field` does when clicked, from its first widget.
    pub fn field_action(&self, field: &Field) -> Option<FieldAction> {
        let widget = field.widgets.first()?;
        let Some(Object::Dict(w)) = self.get(widget.id) else {
            return None;
        };
        let action = match self.resolve_key(&w, "A") {
            Object::Dict(a) => a,
            _ => match self.get(field.id) {
                Some(Object::Dict(f)) => match self.resolve_key(&f, "A") {
                    Object::Dict(a) => a,
                    _ => return None,
                },
                _ => return None,
            },
        };
        self.read_action(&action)
    }

    pub(crate) fn read_action(&self, action: &Dict) -> Option<FieldAction> {
        let names = |key: &str| -> Vec<String> {
            match self.resolve_key(action, key) {
                Object::Array(items) => items
                    .iter()
                    .filter_map(|i| match self.resolve(i) {
                        Object::String { bytes, .. } => Some(text_string(&bytes)),
                        Object::Ref(r) => self.field_name(r),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            }
        };
        let flags = self.resolve_key(action, "Flags").as_int().unwrap_or(0);
        Some(match action.name("S")? {
            b"GoTo" => {
                let catalog = self.catalog()?;
                let mut reader = OutlineReader::new(self, catalog);
                let dest = self.resolve_key(action, "D");
                FieldAction::GoTo(reader.destination(&dest, 0)?)
            }
            b"URI" => match self.resolve_key(action, "URI") {
                Object::String { bytes, .. } => FieldAction::Uri(text_string(&bytes)),
                _ => return None,
            },
            b"ResetForm" => FieldAction::Reset {
                fields: names("Fields"),
                others: flags & INCLUDE_EXCLUDE != 0,
            },
            b"Hide" => {
                let fields = match self.resolve_key(action, "T") {
                    Object::String { bytes, .. } => vec![text_string(&bytes)],
                    Object::Ref(r) => self.field_name(r).into_iter().collect(),
                    Object::Array(_) => names("T"),
                    _ => Vec::new(),
                };
                FieldAction::Hide {
                    fields,
                    hide: !matches!(self.resolve_key(action, "H"), Object::Bool(false)),
                }
            }
            b"SubmitForm" => {
                let url = match self.resolve_key(action, "F") {
                    Object::String { bytes, .. } => text_string(&bytes),
                    Object::Dict(fs) => match self.resolve_key(&fs, "F") {
                        Object::String { bytes, .. } => text_string(&bytes),
                        _ => return None,
                    },
                    _ => return None,
                };
                let format = if flags & SUBMIT_PDF != 0 {
                    SubmitFormat::Pdf
                } else if flags & EXPORT_XFDF != 0 {
                    SubmitFormat::Xfdf
                } else if flags & EXPORT_HTML != 0 {
                    SubmitFormat::Html
                } else {
                    SubmitFormat::Fdf
                };
                FieldAction::Submit { url, format }
            }
            b"Named" => FieldAction::Named(String::from_utf8_lossy(action.name("N")?).into_owned()),
            b"JavaScript" => FieldAction::JavaScript(match self.resolve_key(action, "JS") {
                Object::String { bytes, .. } => text_string(&bytes),
                Object::Stream(s) => String::from_utf8_lossy(
                    &crate::filter::decode(&s.dict, &s.data).unwrap_or_default(),
                )
                .into_owned(),
                _ => String::new(),
            }),
            _ => return None,
        })
    }

    /// The full name of the field object `r` names.
    fn field_name(&self, r: Ref) -> Option<String> {
        self.form().into_iter().find(|f| f.id == r).map(|f| f.name)
    }

    /// `action` as the dictionary a widget's `/A` holds.
    pub(crate) fn action_object(&self, action: &FieldAction) -> Result<Object, NavError> {
        let mut a = Dict::new();
        let strings =
            |names: &[String]| Object::Array(names.iter().map(|n| text_object(n)).collect());
        match action {
            FieldAction::GoTo(dest) => {
                a.set("S", Object::name("GoTo"));
                a.set("D", crate::nav::dest_array(&self.pages(), dest)?);
            }
            FieldAction::Uri(uri) => {
                a.set("S", Object::name("URI"));
                a.set(
                    "URI",
                    Object::String {
                        bytes: uri.as_bytes().to_vec(),
                        hex: false,
                    },
                );
            }
            FieldAction::Reset { fields, others } => {
                a.set("S", Object::name("ResetForm"));
                if !fields.is_empty() {
                    a.set("Fields", strings(fields));
                    if *others {
                        a.set("Flags", Object::Int(INCLUDE_EXCLUDE));
                    }
                }
            }
            FieldAction::Hide { fields, hide } => {
                a.set("S", Object::name("Hide"));
                a.set("T", strings(fields));
                a.set("H", Object::Bool(*hide));
            }
            FieldAction::Submit { url, format } => {
                a.set("S", Object::name("SubmitForm"));
                let mut fs = Dict::new();
                fs.set("FS", Object::name("URL"));
                fs.set(
                    "F",
                    Object::String {
                        bytes: url.as_bytes().to_vec(),
                        hex: false,
                    },
                );
                a.set("F", Object::Dict(fs));
                let flags = match format {
                    SubmitFormat::Fdf => 0,
                    SubmitFormat::Html => EXPORT_HTML,
                    SubmitFormat::Xfdf => EXPORT_XFDF,
                    SubmitFormat::Pdf => SUBMIT_PDF,
                };
                if flags != 0 {
                    a.set("Flags", Object::Int(flags));
                }
            }
            FieldAction::Named(name) => {
                a.set("S", Object::name("Named"));
                a.set("N", Object::name(name));
            }
            FieldAction::JavaScript(js) => {
                a.set("S", Object::name("JavaScript"));
                a.set("JS", text_object(js));
            }
        }
        Ok(Object::Dict(a))
    }

    /// The values a reset action puts in: those of the fields named, or
    /// with `others` of every field but those (none named: every field).
    pub fn reset_values_of(&self, fields: &[String], others: bool) -> Vec<(Ref, FieldValue)> {
        let form = self.form();
        let named = |id: Ref| {
            form.iter().any(|f| {
                f.id == id
                    && fields
                        .iter()
                        .any(|n| f.name == *n || f.name.starts_with(&format!("{n}.")))
            })
        };
        self.reset_values()
            .into_iter()
            .filter(|(id, _)| fields.is_empty() || named(*id) != others)
            .collect()
    }

    /// The file with the fields named hidden (or shown), as an
    /// incremental update.
    pub fn hide_fields(&self, fields: &[String], hide: bool) -> Result<Vec<u8>, UpdateError> {
        let mut update = self.update()?;
        for f in self.form() {
            if !fields
                .iter()
                .any(|n| f.name == *n || f.name.starts_with(&format!("{n}.")))
            {
                continue;
            }
            for w in &f.widgets {
                let Some(Object::Dict(mut dict)) = self.get(w.id) else {
                    continue;
                };
                let flags = self.resolve_key(&dict, "F").as_int().unwrap_or(0);
                let flags = if hide {
                    flags | HIDDEN
                } else {
                    flags & !HIDDEN
                };
                dict.set("F", Object::Int(flags));
                update.replace(w.id, Object::Dict(dict));
            }
        }
        Ok(update.finish())
    }

    /// The form's data as a submit action sends it: the bytes and their
    /// media type.
    pub fn submit_body(&self, format: SubmitFormat, file: &str) -> (Vec<u8>, &'static str) {
        match format {
            SubmitFormat::Fdf => (self.form_fdf(file), "application/vnd.fdf"),
            SubmitFormat::Xfdf => (
                crate::formdata::data_to_xfdf(&self.form_data(), file).into_bytes(),
                "application/vnd.adobe.xfdf",
            ),
            SubmitFormat::Html => {
                let encode = |s: &str| {
                    let mut out = String::new();
                    for b in s.bytes() {
                        match b {
                            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                                out.push(b as char)
                            }
                            b' ' => out.push('+'),
                            _ => out.push_str(&format!("%{b:02X}")),
                        }
                    }
                    out
                };
                let body = self
                    .form_data()
                    .iter()
                    .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                    .collect::<Vec<_>>()
                    .join("&");
                (body.into_bytes(), "application/x-www-form-urlencoded")
            }
            SubmitFormat::Pdf => (self.bytes().to_vec(), "application/pdf"),
        }
    }
}
