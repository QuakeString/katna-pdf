// SPDX-License-Identifier: GPL-3.0-or-later

//! Interactive forms (ISO 32000-2 §12.7): reading a file's fields, and
//! filling them in as an incremental update. Text fields, check boxes and
//! radio buttons can be filled; a filled text field gets a plain
//! appearance of its own, so every reader shows the new value.

use std::collections::{HashMap, HashSet};

use crate::{Dict, Document, Name, Object, Ref, Stream, UpdateError, text_string};

/// How deep the field tree may nest, and how many fields it may hold.
const MAX_DEPTH: usize = 32;
const MAX_FIELDS: usize = 20_000;

/// Field flags (§12.7.4.1, §12.7.5).
const READ_ONLY: i64 = 1;
const MULTILINE: i64 = 1 << 12;
const PASSWORD: i64 = 1 << 13;
const RADIO: i64 = 1 << 15;
const PUSH_BUTTON: i64 = 1 << 16;
const COMBO: i64 = 1 << 17;

/// One field of the form.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The field object, which holds the value.
    pub id: Ref,
    /// The fully qualified name, parts joined by dots.
    pub name: String,
    pub kind: FieldKind,
    /// The value: a text field's text, the "on" state of a check box or
    /// radio group that is on, a choice's chosen text.
    pub value: Option<String>,
    pub read_only: bool,
    /// Where it is shown: one widget for most fields, one a button for
    /// radio groups.
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text {
        multiline: bool,
        password: bool,
        max_len: Option<usize>,
    },
    CheckBox,
    Radio,
    PushButton,
    Choice {
        options: Vec<String>,
        combo: bool,
    },
    Signature,
}

/// One place a field is shown on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub id: Ref,
    pub page: usize,
    /// Left, bottom, right, top in the page's default user space.
    pub rect: [f32; 4],
    /// For a check box or radio button, the name of its "on" state.
    pub on: Option<String>,
}

/// A value to put in a field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Text(String),
    /// A check box or radio group: the "on" state to turn on, or `None`
    /// for off.
    State(Option<String>),
}

impl Document {
    /// The form's fields, or nothing if the file has no form. Encrypted
    /// files' forms cannot be read yet.
    pub fn form(&self) -> Vec<Field> {
        if self.is_encrypted() {
            return Vec::new();
        }
        let Some(form) = self.acro_form() else {
            return Vec::new();
        };
        let Object::Array(fields) = self.resolve_key(&form, "Fields") else {
            return Vec::new();
        };
        let pages = self.annotation_pages();
        let mut reader = FieldReader {
            doc: self,
            pages: &pages,
            seen: HashSet::new(),
            out: Vec::new(),
        };
        for field in fields.iter().filter_map(Object::as_ref) {
            reader.field(field, &Inherited::default(), 0);
        }
        reader.out
    }

    /// The file with `changes` made to its form, as an incremental
    /// update.
    pub fn fill(&self, changes: &[(Ref, FieldValue)]) -> Result<Vec<u8>, UpdateError> {
        let mut update = self.update()?;
        let fields = self.form();
        let form = self.acro_form().unwrap_or_default();
        for (id, value) in changes {
            let Some(field) = fields.iter().find(|f| f.id == *id) else {
                continue;
            };
            let Some(Object::Dict(mut dict)) = self.get(field.id) else {
                continue;
            };
            match value {
                FieldValue::Text(text) => {
                    dict.set("V", text_object(text));
                    let shown = match field.kind {
                        FieldKind::Text { password: true, .. } => "*".repeat(text.chars().count()),
                        _ => text.clone(),
                    };
                    let multiline = matches!(
                        field.kind,
                        FieldKind::Text {
                            multiline: true,
                            ..
                        }
                    );
                    for widget in &field.widgets {
                        let mut target = if widget.id == field.id {
                            dict.clone()
                        } else {
                            match self.get(widget.id) {
                                Some(Object::Dict(w)) => w,
                                _ => continue,
                            }
                        };
                        let ap = self.text_appearance(
                            [&target, &dict, &form],
                            widget,
                            &shown,
                            multiline,
                        );
                        let ap = Dict(vec![(Name::new("N"), Object::Ref(update.add(ap)))]);
                        if widget.id == field.id {
                            dict.set("AP", ap);
                        } else {
                            target.set("AP", ap);
                            update.replace(widget.id, Object::Dict(target));
                        }
                    }
                }
                FieldValue::State(state) => {
                    let state = state.as_deref().unwrap_or("Off");
                    dict.set("V", Object::name(state));
                    for widget in &field.widgets {
                        let shown = if widget.on.as_deref() == Some(state) {
                            state
                        } else {
                            "Off"
                        };
                        if widget.id == field.id {
                            dict.set("AS", Object::name(shown));
                        } else if let Some(Object::Dict(mut w)) = self.get(widget.id) {
                            w.set("AS", Object::name(shown));
                            update.replace(widget.id, Object::Dict(w));
                        }
                    }
                }
            }
            update.replace(field.id, Object::Dict(dict));
        }
        Ok(update.finish())
    }

    pub(crate) fn acro_form(&self) -> Option<Dict> {
        let catalog = self.catalog()?;
        match self.resolve_key(&catalog, "AcroForm") {
            Object::Dict(form) => Some(form),
            _ => None,
        }
    }

    /// Each annotation's page.
    fn annotation_pages(&self) -> HashMap<Ref, usize> {
        let mut out = HashMap::new();
        for (i, page) in self.pages().into_iter().enumerate() {
            let Some(Object::Dict(page)) = self.get(page) else {
                continue;
            };
            if let Object::Array(annots) = self.resolve_key(&page, "Annots") {
                for annot in annots.iter().filter_map(Object::as_ref) {
                    out.entry(annot).or_insert(i);
                }
            }
        }
        out
    }

    /// A plain appearance for a text field's widget showing `text`, in
    /// the font and size the widget, the field or the form asks for
    /// (`/DA`), or Helvetica sized to fit.
    fn text_appearance(
        &self,
        [widget_dict, field_dict, form]: [&Dict; 3],
        widget: &Widget,
        text: &str,
        multiline: bool,
    ) -> Object {
        let [x0, y0, x1, y1] = widget.rect;
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let da = [widget_dict, field_dict, form]
            .map(|d| d.get("DA"))
            .into_iter()
            .flatten()
            .find_map(|da| match self.resolve(da) {
                Object::String { bytes, .. } => Some(String::from_utf8_lossy(&bytes).into_owned()),
                _ => None,
            })
            .unwrap_or_else(|| "/Helv 0 Tf 0 g".into());
        let (font, size, colour) = parse_da(&da);
        let size = if size > 0.0 {
            size
        } else if multiline {
            12.0_f32.min(h)
        } else {
            (h * 0.7).clamp(4.0, 12.0)
        };
        // The background and border the widget asks for (`/MK`).
        let mut content = String::new();
        if let Object::Dict(mk) = self.resolve_key(widget_dict, "MK") {
            if let Some(fill) = self.colour(&mk, "BG", false) {
                content.push_str(&format!("q {fill} 0 0 {w:.2} {h:.2} re f Q\n"));
            }
            if let Some(stroke) = self.colour(&mk, "BC", true) {
                content.push_str(&format!(
                    "q {stroke} 1 w 0.5 0.5 {:.2} {:.2} re S Q\n",
                    w - 1.0,
                    h - 1.0
                ));
            }
        }
        content.push_str(&format!(
            "/Tx BMC q 1 1 {:.2} {:.2} re W n BT /{font} {size:.2} Tf {colour}\n",
            w - 2.0,
            h - 2.0
        ));
        let lines: Vec<&str> = if multiline {
            text.lines().collect()
        } else {
            vec![text.lines().next().unwrap_or("")]
        };
        let leading = size * 1.15;
        let first = if multiline {
            h - 2.0 - size
        } else {
            (h - size * 0.78) / 2.0
        };
        content.push_str(&format!("2 {first:.2} Td {leading:.2} TL\n"));
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                content.push_str("T* ");
            }
            content.push_str(&literal(line));
            content.push_str(" Tj\n");
        }
        content.push_str("ET Q EMC\n");
        // The form's default resources hold the font named in `/DA`;
        // Helvetica is added when they do not.
        let mut resources = match self.resolve_key(form, "DR") {
            Object::Dict(dr) => dr,
            _ => Dict::new(),
        };
        let mut fonts = match self.resolve_key(&resources, "Font") {
            Object::Dict(fonts) => fonts,
            _ => Dict::new(),
        };
        if fonts.get(&font).is_none() {
            fonts.set(
                &font,
                Dict(vec![
                    (Name::new("Type"), Object::name("Font")),
                    (Name::new("Subtype"), Object::name("Type1")),
                    (Name::new("BaseFont"), Object::name("Helvetica")),
                    (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
                ]),
            );
        }
        resources.set("Font", fonts);
        let data = content.into_bytes();
        Object::Stream(Stream {
            dict: Dict(vec![
                (Name::new("Type"), Object::name("XObject")),
                (Name::new("Subtype"), Object::name("Form")),
                (
                    Name::new("BBox"),
                    Object::Array(vec![
                        Object::Int(0),
                        Object::Int(0),
                        Object::Real(f64::from(w)),
                        Object::Real(f64::from(h)),
                    ]),
                ),
                (Name::new("Resources"), Object::Dict(resources)),
                (Name::new("Length"), Object::Int(data.len() as i64)),
            ]),
            data,
        })
    }
}

impl Document {
    /// A colour array in `dict` (1, 3 or 4 numbers) as the operator
    /// that sets it, for filling or `stroke`.
    fn colour(&self, dict: &Dict, key: &str, stroke: bool) -> Option<String> {
        let Object::Array(parts) = self.resolve_key(dict, key) else {
            return None;
        };
        let parts: Vec<String> = parts
            .iter()
            .filter_map(|p| self.resolve(p).as_number())
            .map(|n| format!("{n:.3}"))
            .collect();
        let op = match (parts.len(), stroke) {
            (1, false) => "g",
            (1, true) => "G",
            (3, false) => "rg",
            (3, true) => "RG",
            (4, false) => "k",
            (4, true) => "K",
            _ => return None,
        };
        Some(format!("{} {op}", parts.join(" ")))
    }
}

/// What a field takes from the fields above it (§12.7.4.1).
#[derive(Default, Clone)]
struct Inherited {
    name: String,
    kind: Option<Vec<u8>>,
    flags: i64,
    value: Option<Object>,
    options: Option<Object>,
    max_len: Option<i64>,
}

struct FieldReader<'a> {
    doc: &'a Document,
    pages: &'a HashMap<Ref, usize>,
    seen: HashSet<Ref>,
    out: Vec<Field>,
}

impl FieldReader<'_> {
    fn field(&mut self, id: Ref, parent: &Inherited, depth: usize) {
        if depth > MAX_DEPTH || self.out.len() >= MAX_FIELDS || !self.seen.insert(id) {
            return;
        }
        let Some(Object::Dict(dict)) = self.doc.get(id) else {
            return;
        };
        let mut here = parent.clone();
        if let Object::String { bytes, .. } = self.doc.resolve_key(&dict, "T") {
            let part = text_string(&bytes);
            here.name = if parent.name.is_empty() {
                part
            } else {
                format!("{}.{part}", parent.name)
            };
        }
        if let Some(kind) = dict.name("FT") {
            here.kind = Some(kind.to_vec());
        }
        if let Some(flags) = self.doc.resolve_key(&dict, "Ff").as_int() {
            here.flags = flags;
        }
        if let Some(value) = dict.get("V") {
            here.value = Some(self.doc.resolve(value));
        }
        if let Some(options) = dict.get("Opt") {
            here.options = Some(self.doc.resolve(options));
        }
        if let Some(max_len) = self.doc.resolve_key(&dict, "MaxLen").as_int() {
            here.max_len = Some(max_len);
        }
        let kids: Vec<Ref> = match self.doc.resolve_key(&dict, "Kids") {
            Object::Array(kids) => kids.iter().filter_map(Object::as_ref).collect(),
            _ => Vec::new(),
        };
        // Kids with a name are fields; kids without one are this field's
        // widgets.
        let mut widgets = Vec::new();
        let mut child_fields = Vec::new();
        for kid in kids {
            match self.doc.get(kid) {
                Some(Object::Dict(k)) if k.get("T").is_some() => child_fields.push(kid),
                Some(Object::Dict(k)) => widgets.extend(self.widget(kid, &k)),
                _ => {}
            }
        }
        if child_fields.is_empty() && widgets.is_empty() {
            widgets.extend(self.widget(id, &dict));
        }
        for child in child_fields {
            self.field(child, &here, depth + 1);
        }
        if widgets.is_empty() {
            return;
        }
        let Some(kind) = kind_of(&here) else {
            return;
        };
        let value = match (&kind, &here.value) {
            (_, Some(Object::String { bytes, .. })) => Some(text_string(bytes)),
            (_, Some(Object::Name(name))) if name.as_bytes() != b"Off" => {
                Some(String::from_utf8_lossy(name.as_bytes()).into_owned())
            }
            (FieldKind::Choice { .. }, Some(Object::Array(chosen))) => {
                chosen.iter().find_map(|c| match self.doc.resolve(c) {
                    Object::String { bytes, .. } => Some(text_string(&bytes)),
                    _ => None,
                })
            }
            _ => None,
        };
        self.out.push(Field {
            id,
            name: here.name,
            kind,
            value,
            read_only: here.flags & READ_ONLY != 0,
            widgets,
        });
    }

    fn widget(&self, id: Ref, dict: &Dict) -> Option<Widget> {
        let page = *self.pages.get(&id)?;
        let Object::Array(rect) = self.doc.resolve_key(dict, "Rect") else {
            return None;
        };
        let [x0, y0, x1, y1] = rect
            .iter()
            .map(|n| self.doc.resolve(n).as_number())
            .collect::<Option<Vec<_>>>()?[..]
        else {
            return None;
        };
        let on = match self.doc.resolve_key(dict, "AP") {
            Object::Dict(ap) => match self.doc.resolve_key(&ap, "N") {
                Object::Dict(normal) => normal
                    .iter()
                    .map(|(k, _)| k.as_bytes())
                    .find(|k| *k != b"Off")
                    .map(|k| String::from_utf8_lossy(k).into_owned()),
                _ => None,
            },
            _ => None,
        };
        Some(Widget {
            id,
            page,
            rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32),
            on,
        })
    }
}

fn kind_of(field: &Inherited) -> Option<FieldKind> {
    let flags = field.flags;
    Some(match field.kind.as_deref()? {
        b"Tx" => FieldKind::Text {
            multiline: flags & MULTILINE != 0,
            password: flags & PASSWORD != 0,
            max_len: field.max_len.and_then(|m| usize::try_from(m).ok()),
        },
        b"Btn" if flags & PUSH_BUTTON != 0 => FieldKind::PushButton,
        b"Btn" if flags & RADIO != 0 => FieldKind::Radio,
        b"Btn" => FieldKind::CheckBox,
        b"Ch" => FieldKind::Choice {
            options: match &field.options {
                Some(Object::Array(options)) => options
                    .iter()
                    .filter_map(|o| match o {
                        Object::String { bytes, .. } => Some(text_string(bytes)),
                        // `[export shown]` pairs: the text shown.
                        Object::Array(pair) => match pair.get(1) {
                            Some(Object::String { bytes, .. }) => Some(text_string(bytes)),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            combo: flags & COMBO != 0,
        },
        b"Sig" => FieldKind::Signature,
        _ => return None,
    })
}

/// The font name, size and colour operators of a default appearance
/// string such as `/Helv 12 Tf 0 g`.
fn parse_da(da: &str) -> (String, f32, String) {
    let tokens: Vec<&str> = da.split_whitespace().collect();
    let mut font = "Helv".to_owned();
    let mut size = 0.0;
    let mut colour = "0 g".to_owned();
    for (i, token) in tokens.iter().enumerate() {
        match *token {
            "Tf" if i >= 2 => {
                font = tokens[i - 2].trim_start_matches('/').to_owned();
                size = tokens[i - 1].parse().unwrap_or(0.0);
            }
            "g" if i >= 1 => colour = tokens[i - 1..=i].join(" "),
            "rg" if i >= 3 => colour = tokens[i - 3..=i].join(" "),
            "k" if i >= 4 => colour = tokens[i - 4..=i].join(" "),
            _ => {}
        }
    }
    (font, size, colour)
}

/// A text string to store: PDFDocEncoding when it is plain ASCII,
/// otherwise UTF-16BE with a byte-order mark.
pub(crate) fn text_object(text: &str) -> Object {
    let bytes = if text.chars().all(|c| (' '..='~').contains(&c) || c == '\n') {
        text.as_bytes().to_vec()
    } else {
        let mut bytes = vec![0xfe, 0xff];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        bytes
    };
    Object::String { bytes, hex: false }
}

/// `text` as a literal string in WinAnsi (Latin-1 for most letters);
/// characters it lacks show as `?`.
pub(crate) fn literal(text: &str) -> String {
    let mut out = String::from("(");
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            '\u{a0}'..='\u{ff}' => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_appearance_strings() {
        assert_eq!(
            parse_da("/Helv 12 Tf 0 g"),
            ("Helv".into(), 12.0, "0 g".into())
        );
        assert_eq!(
            parse_da("0.2 0.4 0.6 rg /F1 0 Tf"),
            ("F1".into(), 0.0, "0.2 0.4 0.6 rg".into())
        );
        assert_eq!(literal("a(b)\\é→"), "(a\\(b\\)\\\\\\351?)");
    }
}
