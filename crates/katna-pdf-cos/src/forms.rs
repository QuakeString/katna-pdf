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
pub(crate) const READ_ONLY: i64 = 1;
pub(crate) const REQUIRED: i64 = 1 << 1;
pub(crate) const MULTILINE: i64 = 1 << 12;
pub(crate) const PASSWORD: i64 = 1 << 13;
pub(crate) const RADIO: i64 = 1 << 15;
pub(crate) const PUSH_BUTTON: i64 = 1 << 16;
pub(crate) const COMBO: i64 = 1 << 17;
pub(crate) const EDIT: i64 = 1 << 18;
pub(crate) const DO_NOT_SCROLL: i64 = 1 << 23;
pub(crate) const COMB: i64 = 1 << 24;

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
    /// It must be filled in before the form is sent.
    pub required: bool,
    /// The tip shown over it (`/TU`).
    pub tooltip: Option<String>,
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
        /// One letter to each of `max_len` cells, like a form's row of
        /// boxes.
        comb: bool,
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
    /// Not shown (`/F` hidden or no-view), as a Hide action leaves it.
    pub hidden: bool,
}

/// How a field is drawn: its border, background and text.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub border: Border,
    /// The border's width in points.
    pub width: f32,
    /// The border's colour, or none for no border.
    pub border_colour: Option<[f32; 3]>,
    /// The background, or none for see-through.
    pub fill: Option<[f32; 3]>,
    pub text_colour: [f32; 3],
    /// The text's size in points; 0 for one that fits the field.
    pub text_size: f32,
}

/// The grey new fields' borders are drawn in.
pub(crate) const BORDER_GREY: f32 = 0.45;

impl Default for Look {
    fn default() -> Self {
        Look {
            border: Border::Solid,
            width: 1.0,
            border_colour: Some([BORDER_GREY; 3]),
            fill: None,
            text_colour: [0.0; 3],
            text_size: 0.0,
        }
    }
}

/// A border's style (`/BS /S`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    Solid,
    Dashed,
    /// Raised, with a light top left and a dark bottom right inside.
    Beveled,
    /// Sunken, with a dark top left and a lighter bottom right inside.
    Inset,
    /// A line along the bottom only.
    Underline,
}

impl Border {
    pub const ALL: [Border; 5] = [
        Border::Solid,
        Border::Dashed,
        Border::Beveled,
        Border::Inset,
        Border::Underline,
    ];

    /// Its name in the file (`/S`).
    pub fn name(self) -> &'static str {
        match self {
            Border::Solid => "S",
            Border::Dashed => "D",
            Border::Beveled => "B",
            Border::Inset => "I",
            Border::Underline => "U",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<Border> {
        Some(match name {
            b"S" => Border::Solid,
            b"D" => Border::Dashed,
            b"B" => Border::Beveled,
            b"I" => Border::Inset,
            b"U" => Border::Underline,
            _ => return None,
        })
    }
}

/// A colour array (1, 3 or 4 numbers) as red, green and blue.
pub(crate) fn rgb_of(parts: &[f64]) -> Option<[f32; 3]> {
    Some(match *parts {
        [g] => [g as f32; 3],
        [r, g, b] => [r as f32, g as f32, b as f32],
        [c, m, y, k] => [
            ((1.0 - c) * (1.0 - k)) as f32,
            ((1.0 - m) * (1.0 - k)) as f32,
            ((1.0 - y) * (1.0 - k)) as f32,
        ],
        _ => return None,
    })
}

fn num3([r, g, b]: [f32; 3], op: &str) -> String {
    format!("{r:.3} {g:.3} {b:.3} {op}")
}

/// The background and border `look` asks for, drawn into a box `w` by
/// `h`.
pub(crate) fn border_content(w: f32, h: f32, look: &Look) -> String {
    let mut c = String::new();
    if let Some(fill) = look.fill {
        c.push_str(&format!(
            "q {} 0 0 {w:.2} {h:.2} re f Q\n",
            num3(fill, "rg")
        ));
    }
    let wd = look.width;
    let Some(colour) = look.border_colour.filter(|_| wd > 0.0) else {
        return c;
    };
    let stroke = num3(colour, "RG");
    let half = wd / 2.0;
    match look.border {
        Border::Underline => c.push_str(&format!(
            "q {stroke} {wd:.2} w 0 {half:.2} m {w:.2} {half:.2} l S Q\n"
        )),
        style => {
            let dash = if style == Border::Dashed {
                "[3] 0 d "
            } else {
                ""
            };
            c.push_str(&format!(
                "q {stroke} {wd:.2} w {dash}{half:.2} {half:.2} {:.2} {:.2} re S Q\n",
                (w - wd).max(0.0),
                (h - wd).max(0.0)
            ));
        }
    }
    let (top_left, bottom_right) = match look.border {
        Border::Beveled => ("1 g", "0.5 g"),
        Border::Inset => ("0.5 g", "0.75 g"),
        _ => return c,
    };
    let (w1, h1, w2, h2) = (w - wd, h - wd, w - 2.0 * wd, h - 2.0 * wd);
    let d = 2.0 * wd;
    c.push_str(&format!(
        "q {top_left} {wd:.2} {wd:.2} m {wd:.2} {h1:.2} l {w1:.2} {h1:.2} l {w2:.2} {h2:.2} l {d:.2} {h2:.2} l {d:.2} {d:.2} l h f Q\n"
    ));
    c.push_str(&format!(
        "q {bottom_right} {w1:.2} {h1:.2} m {w1:.2} {wd:.2} l {wd:.2} {wd:.2} l {d:.2} {d:.2} l {w2:.2} {d:.2} l {w2:.2} {h2:.2} l h f Q\n"
    ));
    c
}

/// How far in from the edge a look's text and marks start.
pub(crate) fn inset(look: &Look) -> f32 {
    let wd = if look.border_colour.is_some() {
        look.width
    } else {
        0.0
    };
    match look.border {
        Border::Beveled | Border::Inset => 2.0 * wd + 1.0,
        _ => wd + 1.0,
    }
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
    /// The form's fields, or nothing if the file has no form, or is locked
    /// and not opened with its password.
    pub fn form(&self) -> Vec<Field> {
        if self.is_locked() {
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
        let scripts = self.form_scripts(&fields);
        let changes = self.recalculate(&fields, &scripts, changes);
        for (id, value) in &changes {
            let Some(field) = fields.iter().find(|f| f.id == *id) else {
                continue;
            };
            let Some(Object::Dict(mut dict)) = self.get(field.id) else {
                continue;
            };
            match value {
                FieldValue::Text(text) => {
                    dict.set("V", text_object(text));
                    let shown = match (&field.kind, scripts.get(id).and_then(|s| s.format.as_ref()))
                    {
                        (FieldKind::Text { password: true, .. }, _) => {
                            "*".repeat(text.chars().count())
                        }
                        (_, Some(format)) => crate::calc::show(format, text),
                        _ => text.clone(),
                    };
                    let multiline = matches!(
                        field.kind,
                        FieldKind::Text {
                            multiline: true,
                            ..
                        }
                    );
                    let comb = match field.kind {
                        FieldKind::Text {
                            comb: true,
                            max_len: Some(n),
                            multiline: false,
                            password: false,
                        } => Some(n),
                        _ => None,
                    };
                    for widget in &field.widgets {
                        let mut target = if widget.id == field.id {
                            dict.clone()
                        } else {
                            match self.get(widget.id) {
                                Some(Object::Dict(w)) => w,
                                _ => continue,
                            }
                        };
                        let ap = match &field.kind {
                            FieldKind::Choice {
                                options,
                                combo: false,
                            } => crate::design::list_box(
                                widget.rect,
                                options,
                                Some(text),
                                &self.look([&target, &dict, &form]),
                            ),
                            _ => self.text_appearance(
                                [&target, &dict, &form],
                                widget,
                                &shown,
                                multiline,
                                comb,
                            ),
                        };
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

    /// Every field's value as the form started: its default (`/DV`), or
    /// nothing. [`Document::fill`] with these resets the form.
    pub fn reset_values(&self) -> Vec<(Ref, FieldValue)> {
        self.form()
            .into_iter()
            .filter_map(|f| {
                let default = self.inherited(f.id, "DV");
                Some(match f.kind {
                    FieldKind::PushButton | FieldKind::Signature => return None,
                    FieldKind::CheckBox | FieldKind::Radio => (
                        f.id,
                        FieldValue::State(match default {
                            Some(Object::Name(n)) if n.as_bytes() != b"Off" => {
                                Some(String::from_utf8_lossy(n.as_bytes()).into_owned())
                            }
                            _ => None,
                        }),
                    ),
                    _ => (
                        f.id,
                        FieldValue::Text(match default {
                            Some(Object::String { bytes, .. }) => {
                                crate::outline::text_value(&bytes)
                            }
                            _ => String::new(),
                        }),
                    ),
                })
            })
            .collect()
    }

    /// `key` of field `id`, or of the nearest field above it that has it.
    fn inherited(&self, id: Ref, key: &str) -> Option<Object> {
        let mut at = id;
        for _ in 0..MAX_DEPTH {
            let Some(Object::Dict(dict)) = self.get(at) else {
                return None;
            };
            if let Some(v) = dict.get(key) {
                return Some(self.resolve(v));
            }
            at = dict.get("Parent").and_then(Object::as_ref)?;
        }
        None
    }

    /// How field `field` is drawn, from its first widget.
    pub fn field_look(&self, field: &Field) -> Look {
        let widget = field
            .widgets
            .first()
            .and_then(|w| self.get(w.id))
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let dict = self
            .get(field.id)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let form = self.acro_form().unwrap_or_default();
        self.look([&widget, &dict, &form])
    }

    /// The look a widget's `/MK`, `/BS` and `/DA` (its own, its field's
    /// or the form's) ask for.
    pub(crate) fn look(&self, [widget, field, form]: [&Dict; 3]) -> Look {
        let mut look = Look::default();
        if let Object::Dict(mk) = self.resolve_key(widget, "MK") {
            look.border_colour = self.colour_parts(&mk, "BC").and_then(|p| rgb_of(&p));
            look.fill = self.colour_parts(&mk, "BG").and_then(|p| rgb_of(&p));
        } else {
            look.border_colour = None;
        }
        if let Object::Dict(bs) = self.resolve_key(widget, "BS") {
            if let Some(w) = self.resolve_key(&bs, "W").as_number() {
                look.width = w as f32;
            }
            if let Some(style) = bs.name("S").and_then(Border::from_name) {
                look.border = style;
            }
        }
        if let Some(da) = self.default_appearance([widget, field, form]) {
            let (_, size, colour) = parse_da(&da);
            look.text_size = size;
            let parts: Vec<f64> = colour
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if let Some(rgb) = rgb_of(&parts) {
                look.text_colour = rgb;
            }
        }
        look
    }

    fn default_appearance(&self, dicts: [&Dict; 3]) -> Option<String> {
        dicts
            .map(|d| d.get("DA"))
            .into_iter()
            .flatten()
            .find_map(|da| match self.resolve(da) {
                Object::String { bytes, .. } => Some(String::from_utf8_lossy(&bytes).into_owned()),
                _ => None,
            })
    }

    /// `changes` with the fields worked out from others (totals and the
    /// like) worked out again, in the form's order.
    fn recalculate(
        &self,
        fields: &[Field],
        scripts: &HashMap<Ref, crate::calc::Scripts>,
        changes: &[(Ref, FieldValue)],
    ) -> Vec<(Ref, FieldValue)> {
        let mut changes = changes.to_vec();
        let order = self.calculation_order(scripts);
        if order.is_empty() {
            return changes;
        }
        let mut values: HashMap<String, String> = fields
            .iter()
            .map(|f| (f.name.clone(), f.value.clone().unwrap_or_default()))
            .collect();
        let name_of = |id: Ref| fields.iter().find(|f| f.id == id).map(|f| f.name.clone());
        for (id, value) in &changes {
            if let (Some(name), FieldValue::Text(text)) = (name_of(*id), value) {
                values.insert(name, text.clone());
            }
        }
        for id in order {
            let (Some(name), Some(calc)) = (
                name_of(id),
                scripts.get(&id).and_then(|s| s.calculate.as_ref()),
            ) else {
                continue;
            };
            let result = crate::calc::calculate(calc, &values);
            if values.get(&name) == Some(&result) {
                continue;
            }
            values.insert(name, result.clone());
            changes.retain(|(r, _)| *r != id);
            changes.push((id, FieldValue::Text(result)));
        }
        changes
    }

    pub(crate) fn acro_form(&self) -> Option<Dict> {
        let catalog = self.catalog()?;
        match self.resolve_key(&catalog, "AcroForm") {
            Object::Dict(form) => Some(form),
            _ => None,
        }
    }

    /// Each annotation's page.
    pub(crate) fn annotation_pages(&self) -> HashMap<Ref, usize> {
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
    pub(crate) fn text_appearance(
        &self,
        [widget_dict, field_dict, form]: [&Dict; 3],
        widget: &Widget,
        text: &str,
        multiline: bool,
        comb: Option<usize>,
    ) -> Object {
        let [x0, y0, x1, y1] = widget.rect;
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let da = self
            .default_appearance([widget_dict, field_dict, form])
            .unwrap_or_else(|| "/Helv 0 Tf 0 g".into());
        let (font, size, colour) = parse_da(&da);
        let look = self.look([widget_dict, field_dict, form]);
        let pad = inset(&look);
        let size = if size > 0.0 {
            size
        } else if multiline {
            12.0_f32.min(h)
        } else {
            ((h - 2.0 * pad + 2.0) * 0.7).clamp(4.0, 12.0)
        };
        // The background and border the widget asks for (`/MK`, `/BS`).
        let mut content = border_content(w, h, &look);
        if let Some(cells) = comb.filter(|n| *n > 0) {
            // A letter to each cell, lines between the cells.
            let cell = w / cells as f32;
            if let Some(line) = look.border_colour.filter(|_| look.width > 0.0) {
                let wd = look.width;
                content.push_str(&format!("q {} {wd:.2} w\n", num3(line, "RG")));
                for i in 1..cells {
                    let x = cell * i as f32;
                    content.push_str(&format!("{x:.2} {wd:.2} m {x:.2} {:.2} l\n", h - wd));
                }
                content.push_str("S Q\n");
            }
            content.push_str(&format!("/Tx BMC q BT /{font} {size:.2} Tf {colour}\n"));
            let y = (h - size * 0.78) / 2.0;
            for (i, ch) in text.chars().take(cells).enumerate() {
                let glyph = ch.to_string();
                let x = cell * i as f32 + (cell - crate::design::text_width(&glyph, size)) / 2.0;
                content.push_str(&format!(
                    "1 0 0 1 {x:.2} {y:.2} Tm {} Tj\n",
                    literal(&glyph)
                ));
            }
            content.push_str("ET Q EMC\n");
        } else {
            content.push_str(&format!(
                "/Tx BMC q {pad:.2} {pad:.2} {:.2} {:.2} re W n BT /{font} {size:.2} Tf {colour}\n",
                (w - 2.0 * pad).max(0.0),
                (h - 2.0 * pad).max(0.0)
            ));
            let lines: Vec<&str> = if multiline {
                text.lines().collect()
            } else {
                vec![text.lines().next().unwrap_or("")]
            };
            let leading = size * 1.15;
            let first = if multiline {
                h - pad - 1.0 - size
            } else {
                (h - size * 0.78) / 2.0
            };
            content.push_str(&format!("{:.2} {first:.2} Td {leading:.2} TL\n", pad + 1.0));
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    content.push_str("T* ");
                }
                content.push_str(&literal(line));
                content.push_str(" Tj\n");
            }
            content.push_str("ET Q EMC\n");
        }
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
    /// The numbers of a colour array in `dict`; none for an empty one
    /// (which means no colour) or none at all.
    pub(crate) fn colour_parts(&self, dict: &Dict, key: &str) -> Option<Vec<f64>> {
        let Object::Array(parts) = self.resolve_key(dict, key) else {
            return None;
        };
        let parts: Vec<f64> = parts
            .iter()
            .filter_map(|p| self.resolve(p).as_number())
            .collect();
        (!parts.is_empty()).then_some(parts)
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
            (_, Some(Object::String { bytes, .. })) => {
                Some(crate::outline::text_value(bytes)).filter(|t| !t.is_empty())
            }
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
            required: here.flags & REQUIRED != 0,
            tooltip: match self.doc.resolve_key(&dict, "TU") {
                Object::String { bytes, .. } => Some(text_string(&bytes)),
                _ => None,
            },
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
        let flags = self.doc.resolve_key(dict, "F").as_int().unwrap_or(0);
        Some(Widget {
            id,
            page,
            rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32),
            on,
            hidden: flags & (2 | 32) != 0,
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
            comb: flags & COMB != 0,
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
