// SPDX-License-Identifier: GPL-3.0-or-later

//! Designing a form (docs/PLAN.md §4.3, "forms designer"): adding text,
//! check box, radio, combo, list, button and signature fields, changing
//! their names, tips, flags and choices, moving and resizing them,
//! copying them onto other pages, taking them out and setting the order
//! Tab goes through them, as an incremental update. New fields get
//! appearances of their own, so every reader shows them alike.

use std::collections::HashMap;

use crate::fonts::{stand_in, win_ansi_code, win_ansi_width};
use crate::forms::{
    COMBO, EDIT, MULTILINE, PASSWORD, PUSH_BUTTON, RADIO, READ_ONLY, REQUIRED, literal, text_object,
};
use crate::{
    Dict, Document, Field, FieldKind, Name, Object, Ref, Stream, Update, UpdateError, Widget,
};

/// A radio group whose buttons cannot all be turned off.
const NO_TOGGLE_OFF: i64 = 1 << 14;
/// Shown and printed.
const PRINT: i64 = 4;
/// The border new fields get, and a pushed button's face.
const BORDER: f32 = 0.45;
const FACE: f32 = 0.85;

/// A field to add.
#[derive(Debug, Clone, PartialEq)]
pub enum NewField {
    Text {
        multiline: bool,
        password: bool,
        max_len: Option<usize>,
    },
    CheckBox,
    /// One button of a radio group, standing for `choice`; buttons with
    /// the same name are one group, of which one can be on.
    Radio {
        choice: String,
    },
    Choice {
        options: Vec<String>,
        /// A drop-down list; otherwise a list box.
        combo: bool,
        /// A drop-down list people may also type into.
        editable: bool,
    },
    PushButton {
        caption: String,
    },
    Signature,
}

/// A new field and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSpec {
    pub page: usize,
    /// Left, bottom, right, top in the page's own space.
    pub rect: [f32; 4],
    /// Its name; a field of the same kind and name already there gets
    /// this as one more place it is shown.
    pub name: String,
    pub kind: NewField,
    pub tooltip: String,
    pub required: bool,
    pub read_only: bool,
    /// What it starts with: text, the chosen option, or for a check box
    /// or radio button anything to have it on.
    pub value: Option<String>,
}

/// Changes to a field; `None` leaves that part as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldEdit {
    pub field: Ref,
    /// Its own part of the name (the last, after any dots).
    pub name: Option<String>,
    /// An empty tip takes it away.
    pub tooltip: Option<String>,
    pub required: Option<bool>,
    pub read_only: Option<bool>,
    /// A text field's many lines.
    pub multiline: Option<bool>,
    /// A combo or list's choices.
    pub options: Option<Vec<String>>,
}

impl FieldEdit {
    pub fn new(field: Ref) -> FieldEdit {
        FieldEdit {
            field,
            name: None,
            tooltip: None,
            required: None,
            read_only: None,
            multiline: None,
            options: None,
        }
    }
}

/// What [`Document::design_form`] does, in this order: take out, change,
/// move, copy, add, then set the Tab order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormDesign {
    /// Fields to take out, with every place they are shown.
    pub remove: Vec<Ref>,
    pub edit: Vec<FieldEdit>,
    /// Widgets and their new places.
    pub place: Vec<(Ref, [f32; 4])>,
    /// Fields to show on more pages too, at the same place.
    pub copy: Vec<(Ref, Vec<usize>)>,
    pub add: Vec<FieldSpec>,
    /// Pages and their widgets in the order Tab goes through them; the
    /// page's other annotations keep their places.
    pub order: Vec<(usize, Vec<Ref>)>,
}

/// Why a form could not be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormError {
    NoPage(usize),
    NoField,
    /// Another kind of field already has this name.
    NameTaken(String),
    Update(UpdateError),
}

impl std::fmt::Display for FormError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormError::NoPage(page) => write!(f, "there is no page {}", page + 1),
            FormError::NoField => f.write_str("the field is not in the form"),
            FormError::NameTaken(name) => write!(f, "another kind of field is named {name}"),
            FormError::Update(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FormError {}

impl From<UpdateError> for FormError {
    fn from(e: UpdateError) -> FormError {
        FormError::Update(e)
    }
}

impl Document {
    /// The file with its form changed as `design` says, as an
    /// incremental update.
    pub fn design_form(&self, design: &FormDesign) -> Result<Vec<u8>, FormError> {
        let mut up = self.update()?;
        let pages = self.pages();
        let page_ref = |page: usize| pages.get(page).copied().ok_or(FormError::NoPage(page));
        let fields = self.form();
        let field = |id: Ref| fields.iter().find(|f| f.id == id).ok_or(FormError::NoField);
        let form_ref = self.form_object(&mut up)?;
        let mut d = Designer {
            doc: self,
            up,
            form_ref,
            split: HashMap::new(),
        };

        for &id in &design.remove {
            let f = field(id)?;
            for w in &f.widgets {
                d.unlist(page_ref(w.page)?, w.id);
            }
            d.drop_field(id);
        }

        for edit in &design.edit {
            let f = field(edit.field)?;
            d.edit(f, edit, &fields)?;
        }

        for &(widget, rect) in &design.place {
            let (f, w) = fields
                .iter()
                .find_map(|f| f.widgets.iter().find(|w| w.id == widget).map(|w| (f, w)))
                .ok_or(FormError::NoField)?;
            let mut dict = d.dict(widget);
            dict.set("Rect", reals(&normal(rect)));
            d.up.replace(widget, Object::Dict(dict));
            let moved = Widget {
                rect: normal(rect),
                ..w.clone()
            };
            d.redraw(f, &moved);
        }

        for (id, to) in &design.copy {
            let f = field(*id)?;
            let Some(first) = f.widgets.first() else {
                continue;
            };
            let model = d.dict(first.id);
            let kids = d.split(f);
            let mut kids = kids;
            for &page in to {
                let p = page_ref(page)?;
                let mut w = Dict::new();
                for key in [
                    "Type", "Subtype", "Rect", "F", "MK", "AP", "AS", "BS", "DA", "Q",
                ] {
                    if let Some(v) = model.get(key) {
                        w.set(key, v.clone());
                    }
                }
                w.set("Parent", Object::Ref(f.id));
                w.set("P", Object::Ref(p));
                let r = d.up.add(Object::Dict(w));
                d.list(p, r);
                kids.push(Object::Ref(r));
            }
            let mut fd = d.dict(f.id);
            fd.set("Kids", Object::Array(kids));
            d.up.replace(f.id, Object::Dict(fd));
        }

        // Fields made here, by name, so later specs can join them.
        let mut made: HashMap<String, (Ref, bool)> = HashMap::new();
        for spec in &design.add {
            let p = page_ref(spec.page)?;
            let name = spec.name.replace('.', "_");
            let radio = matches!(spec.kind, NewField::Radio { .. });
            let ft = ft(&spec.kind);
            let existing = match made.get(&name) {
                Some(&(r, was_radio)) => Some((r, was_radio, None)),
                None => fields.iter().find(|f| f.name == name).map(|f| {
                    let same =
                        matches!(f.kind, FieldKind::Radio) == radio && kind_ft(&f.kind) == ft;
                    (f.id, same && radio, Some((f, same)))
                }),
            };
            let widget = d.widget(spec, p);
            match existing {
                Some((_, _, Some((_, false)))) => return Err(FormError::NameTaken(name)),
                Some((parent, _, found)) => {
                    // One more place for a field there already.
                    let mut kids = match found {
                        Some((f, _)) => d.split(f),
                        None => match d.dict(parent).get("Kids") {
                            Some(Object::Array(k)) => k.clone(),
                            _ => Vec::new(),
                        },
                    };
                    let mut w = widget;
                    w.set("Parent", Object::Ref(parent));
                    let r = d.up.add(Object::Dict(w));
                    d.list(p, r);
                    kids.push(Object::Ref(r));
                    let mut fd = d.dict(parent);
                    fd.set("Kids", Object::Array(kids));
                    if let NewField::Radio { choice } = &spec.kind
                        && spec.value.is_some()
                    {
                        fd.set("V", Object::name(&state_name(choice)));
                    }
                    d.up.replace(parent, Object::Dict(fd));
                }
                None => {
                    let mut fd = Dict::new();
                    fd.set("FT", Object::name(ft));
                    fd.set("T", text_object(&name));
                    if !spec.tooltip.is_empty() {
                        fd.set("TU", text_object(&spec.tooltip));
                    }
                    let mut flags = flags(&spec.kind);
                    if spec.required {
                        flags |= REQUIRED;
                    }
                    if spec.read_only {
                        flags |= READ_ONLY;
                    }
                    if flags != 0 {
                        fd.set("Ff", Object::Int(flags));
                    }
                    match &spec.kind {
                        NewField::Text {
                            max_len: Some(n), ..
                        } => {
                            fd.set("MaxLen", Object::Int(*n as i64));
                        }
                        NewField::Choice { options, .. } => {
                            fd.set(
                                "Opt",
                                Object::Array(options.iter().map(|o| text_object(o)).collect()),
                            );
                        }
                        _ => {}
                    }
                    if !matches!(spec.kind, NewField::Signature | NewField::PushButton { .. }) {
                        fd.set("DA", text_object("/Helv 0 Tf 0 g"));
                    }
                    let value = match (&spec.kind, &spec.value) {
                        (_, None) => None,
                        (NewField::CheckBox, Some(_)) => Some(Object::name("Yes")),
                        (NewField::Radio { choice }, Some(_)) => {
                            Some(Object::name(&state_name(choice)))
                        }
                        (NewField::Text { .. } | NewField::Choice { .. }, Some(v)) => {
                            Some(text_object(v))
                        }
                        _ => None,
                    };
                    if let Some(v) = value {
                        fd.set("V", v);
                    } else if matches!(spec.kind, NewField::CheckBox | NewField::Radio { .. }) {
                        fd.set("V", Object::name("Off"));
                    }
                    let r = if radio {
                        let parent = d.up.add(Object::Dict(fd.clone()));
                        let mut w = widget;
                        w.set("Parent", Object::Ref(parent));
                        let kid = d.up.add(Object::Dict(w));
                        d.list(p, kid);
                        fd.set("Kids", Object::Array(vec![Object::Ref(kid)]));
                        d.up.replace(parent, Object::Dict(fd));
                        parent
                    } else {
                        for (k, v) in widget.iter() {
                            fd.set(&String::from_utf8_lossy(k.as_bytes()), v.clone());
                        }
                        let r = d.up.add(Object::Dict(fd));
                        d.list(p, r);
                        r
                    };
                    d.add_to_form(r);
                    made.insert(name, (r, radio));
                }
            }
        }

        for (page, widgets) in &design.order {
            d.reorder(page_ref(*page)?, widgets);
        }
        Ok(d.up.finish())
    }

    /// The form's object, made (with the fonts new fields use) when the
    /// file has none or holds it in the catalog itself.
    fn form_object(&self, up: &mut Update) -> Result<Ref, FormError> {
        let catalog_ref = self
            .trailer()
            .get("Root")
            .and_then(Object::as_ref)
            .ok_or(FormError::Update(UpdateError::Repaired))?;
        let mut catalog = self.catalog().unwrap_or_default();
        let existing = catalog.get("AcroForm").and_then(Object::as_ref);
        let mut form = self.acro_form().unwrap_or_default();
        if !matches!(self.resolve_key(&form, "Fields"), Object::Array(_)) {
            form.set("Fields", Object::Array(Vec::new()));
        }
        let mut dr = match self.resolve_key(&form, "DR") {
            Object::Dict(dr) => dr,
            _ => Dict::new(),
        };
        let mut fonts = match self.resolve_key(&dr, "Font") {
            Object::Dict(f) => f,
            _ => Dict::new(),
        };
        for (name, base) in [("Helv", "Helvetica"), ("ZaDb", "ZapfDingbats")] {
            if fonts.get(name).is_none() {
                fonts.set(name, font(base));
            }
        }
        dr.set("Font", Object::Dict(fonts));
        form.set("DR", Object::Dict(dr));
        if form.get("DA").is_none() {
            form.set("DA", text_object("/Helv 0 Tf 0 g"));
        }
        // Appearances are written for every field; readers needn't make
        // their own.
        form.remove("NeedAppearances");
        Ok(match existing {
            Some(r) => {
                up.replace(r, Object::Dict(form));
                r
            }
            None => {
                let r = up.add(Object::Dict(form));
                catalog.set("AcroForm", Object::Ref(r));
                up.replace(catalog_ref, Object::Dict(catalog));
                r
            }
        })
    }
}

struct Designer<'a> {
    doc: &'a Document,
    up: Update<'a>,
    form_ref: Ref,
    /// Fields that were their own widget, and the widget split off.
    split: HashMap<Ref, Ref>,
}

impl Designer<'_> {
    fn dict(&self, r: Ref) -> Dict {
        match self.up.get(r) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        }
    }

    fn resolve(&self, obj: Option<&Object>) -> Object {
        match obj {
            Some(Object::Ref(r)) => self.up.get(*r).unwrap_or(Object::Null),
            Some(o) => o.clone(),
            None => Object::Null,
        }
    }

    fn form(&self) -> Dict {
        self.dict(self.form_ref)
    }

    /// Page `page`'s annotations, and the object holding the list when
    /// it is one of its own.
    fn annots(&self, page: Ref) -> (Vec<Object>, Option<Ref>) {
        let dict = self.dict(page);
        let own = dict.get("Annots").and_then(Object::as_ref);
        match self.resolve(dict.get("Annots")) {
            Object::Array(a) => (a, own),
            _ => (Vec::new(), None),
        }
    }

    fn set_annots(&mut self, page: Ref, annots: Vec<Object>) {
        match self.annots(page).1 {
            Some(list) => self.up.replace(list, Object::Array(annots)),
            None => {
                let mut dict = self.dict(page);
                dict.set("Annots", Object::Array(annots));
                self.up.replace(page, Object::Dict(dict));
            }
        }
    }

    fn list(&mut self, page: Ref, widget: Ref) {
        let mut annots = self.annots(page).0;
        annots.push(Object::Ref(widget));
        self.set_annots(page, annots);
    }

    fn unlist(&mut self, page: Ref, widget: Ref) {
        let mut annots = self.annots(page).0;
        annots.retain(|a| a.as_ref() != Some(widget));
        self.set_annots(page, annots);
    }

    fn add_to_form(&mut self, field: Ref) {
        let mut form = self.form();
        let mut list = match self.resolve(form.get("Fields")) {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        list.push(Object::Ref(field));
        form.set("Fields", Object::Array(list));
        self.up.replace(self.form_ref, Object::Dict(form));
    }

    /// Takes field `id` out of its parent, or the form; a parent left
    /// with nothing goes too.
    fn drop_field(&mut self, id: Ref) {
        let mut id = id;
        for _ in 0..32 {
            let parent = self.dict(id).get("Parent").and_then(Object::as_ref);
            let (holder, key) = match parent {
                Some(p) => (p, "Kids"),
                None => (self.form_ref, "Fields"),
            };
            let mut dict = self.dict(holder);
            let mut list = match self.resolve(dict.get(key)) {
                Object::Array(a) => a,
                _ => Vec::new(),
            };
            list.retain(|o| o.as_ref() != Some(id));
            let empty = list.is_empty();
            dict.set(key, Object::Array(list));
            self.up.replace(holder, Object::Dict(dict));
            match parent {
                Some(p) if empty => id = p,
                _ => return,
            }
        }
    }

    /// Field `f`'s widgets as kids of their own, splitting a field that
    /// is its own widget in two.
    fn split(&mut self, f: &Field) -> Vec<Object> {
        let dict = self.dict(f.id);
        if let Object::Array(kids) = self.resolve(dict.get("Kids")) {
            return kids;
        }
        let mut field = Dict::new();
        let mut widget = Dict::new();
        for (k, v) in dict.iter() {
            let to_widget = matches!(
                k.as_bytes(),
                b"Type"
                    | b"Subtype"
                    | b"Rect"
                    | b"F"
                    | b"MK"
                    | b"AP"
                    | b"AS"
                    | b"BS"
                    | b"Border"
                    | b"P"
                    | b"A"
                    | b"AA"
                    | b"NM"
                    | b"M"
                    | b"StructParent"
                    | b"OC"
                    | b"Q"
            );
            if to_widget {
                widget.set(&String::from_utf8_lossy(k.as_bytes()), v.clone());
            } else {
                field.set(&String::from_utf8_lossy(k.as_bytes()), v.clone());
            }
        }
        widget.set("Parent", Object::Ref(f.id));
        let w = self.up.add(Object::Dict(widget));
        self.split.insert(f.id, w);
        if let Some(page) = f
            .widgets
            .first()
            .and_then(|w| self.doc.pages().get(w.page).copied())
        {
            let annots: Vec<Object> = self
                .annots(page)
                .0
                .into_iter()
                .map(|a| {
                    if a.as_ref() == Some(f.id) {
                        Object::Ref(w)
                    } else {
                        a
                    }
                })
                .collect();
            self.set_annots(page, annots);
        }
        let kids = vec![Object::Ref(w)];
        field.set("Kids", Object::Array(kids.clone()));
        self.up.replace(f.id, Object::Dict(field));
        kids
    }

    fn edit(&mut self, f: &Field, edit: &FieldEdit, fields: &[Field]) -> Result<(), FormError> {
        let mut dict = self.dict(f.id);
        if let Some(name) = &edit.name {
            let name = name.replace('.', "_");
            let full = match f.name.rsplit_once('.') {
                Some((prefix, _)) => format!("{prefix}.{name}"),
                None => name.clone(),
            };
            if fields.iter().any(|o| o.id != f.id && o.name == full) {
                return Err(FormError::NameTaken(full));
            }
            dict.set("T", text_object(&name));
        }
        match edit.tooltip.as_deref() {
            Some("") => {
                dict.remove("TU");
            }
            Some(tip) => dict.set("TU", text_object(tip)),
            None => {}
        }
        let mut flags = self.resolve(dict.get("Ff")).as_int().unwrap_or(0);
        for (on, bit) in [
            (edit.required, REQUIRED),
            (edit.read_only, READ_ONLY),
            (
                edit.multiline
                    .filter(|_| matches!(f.kind, FieldKind::Text { .. })),
                MULTILINE,
            ),
        ] {
            match on {
                Some(true) => flags |= bit,
                Some(false) => flags &= !bit,
                None => {}
            }
        }
        dict.set("Ff", Object::Int(flags));
        let mut kind = f.kind.clone();
        if let (Some(options), FieldKind::Choice { combo, .. }) = (&edit.options, &f.kind) {
            dict.set(
                "Opt",
                Object::Array(options.iter().map(|o| text_object(o)).collect()),
            );
            kind = FieldKind::Choice {
                options: options.clone(),
                combo: *combo,
            };
        }
        if let (
            Some(multiline),
            FieldKind::Text {
                password, max_len, ..
            },
        ) = (edit.multiline, &f.kind)
        {
            kind = FieldKind::Text {
                multiline,
                password: *password,
                max_len: *max_len,
            };
        }
        self.up.replace(f.id, Object::Dict(dict));
        let changed = Field { kind, ..f.clone() };
        for w in &f.widgets {
            self.redraw(&changed, w);
        }
        Ok(())
    }

    /// Draws widget `w` of field `f` again: text, choices and buttons,
    /// whose look depends on their size. Others keep theirs, which is
    /// stretched to the new place.
    fn redraw(&mut self, f: &Field, w: &Widget) {
        if !matches!(
            f.kind,
            FieldKind::Text { .. } | FieldKind::Choice { .. } | FieldKind::PushButton
        ) {
            return;
        }
        let mut widget = self.dict(w.id);
        let field = self.dict(f.id);
        let caption = match self.resolve(widget.get("MK")) {
            Object::Dict(mk) => match self.resolve(mk.get("CA")) {
                Object::String { bytes, .. } => crate::text_string(&bytes),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let shown = match &f.kind {
            FieldKind::Text { password: true, .. } => {
                "*".repeat(f.value.as_deref().unwrap_or("").chars().count())
            }
            _ => f.value.clone().unwrap_or_default(),
        };
        let form = self.form();
        let normal = match &f.kind {
            FieldKind::Text { multiline, .. } => {
                self.doc
                    .text_appearance([&widget, &field, &form], w, &shown, *multiline)
            }
            FieldKind::Choice { combo: true, .. } => {
                self.doc
                    .text_appearance([&widget, &field, &form], w, &shown, false)
            }
            FieldKind::Choice { options, .. } => list_box(w.rect, options, f.value.as_deref()),
            _ => button(w.rect, &caption),
        };
        let r = self.up.add(normal);
        let mut ap = match self.resolve(widget.get("AP")) {
            Object::Dict(ap) => ap,
            _ => Dict::new(),
        };
        ap.set("N", Object::Ref(r));
        ap.remove("D");
        widget.set("AP", Object::Dict(ap));
        self.up.replace(w.id, Object::Dict(widget));
    }

    /// The widget part of a new field: its place, look and appearances.
    fn widget(&mut self, spec: &FieldSpec, page: Ref) -> Dict {
        let rect = normal(spec.rect);
        let mut w = Dict::new();
        w.set("Type", Object::name("Annot"));
        w.set("Subtype", Object::name("Widget"));
        w.set("Rect", reals(&rect));
        w.set("F", Object::Int(PRINT));
        w.set("P", Object::Ref(page));
        let mut mk = Dict::new();
        mk.set("BC", Object::Array(vec![Object::Real(f64::from(BORDER))]));
        let on = spec.value.is_some();
        let ap = match &spec.kind {
            NewField::CheckBox | NewField::Radio { .. } => {
                let (state, mark, round) = match &spec.kind {
                    NewField::Radio { choice } => (state_name(choice), "l", true),
                    _ => ("Yes".to_string(), "4", false),
                };
                mk.set("CA", text_object(mark));
                let yes = self.up.add(toggle(rect, Some(mark), round));
                let off = self.up.add(toggle(rect, None, round));
                w.set("AS", Object::name(if on { &state } else { "Off" }));
                let mut states = Dict::new();
                states.set(&state, Object::Ref(yes));
                states.set("Off", Object::Ref(off));
                states
            }
            kind => {
                let shown = match kind {
                    NewField::Text { password: true, .. } => {
                        "*".repeat(spec.value.as_deref().unwrap_or("").chars().count())
                    }
                    _ => spec.value.clone().unwrap_or_default(),
                };
                let stream = match kind {
                    NewField::PushButton { caption } => {
                        mk.set("BG", Object::Array(vec![Object::Real(f64::from(FACE))]));
                        mk.set("CA", text_object(caption));
                        button(rect, caption)
                    }
                    NewField::Choice {
                        options,
                        combo: false,
                        ..
                    } => list_box(rect, options, spec.value.as_deref()),
                    NewField::Signature => frame(rect, ""),
                    _ => {
                        let widget = Widget {
                            id: Ref::new(0, 0),
                            page: spec.page,
                            rect,
                            on: None,
                        };
                        let mut here = Dict::new();
                        here.set("MK", Object::Dict(mk.clone()));
                        let form = self.form();
                        let multiline = matches!(
                            kind,
                            NewField::Text {
                                multiline: true,
                                ..
                            }
                        );
                        self.doc.text_appearance(
                            [&here, &Dict::new(), &form],
                            &widget,
                            &shown,
                            multiline,
                        )
                    }
                };
                let r = self.up.add(stream);
                let mut n = Dict::new();
                n.set("N", Object::Ref(r));
                w.set("AP", Object::Dict(n));
                w.set("MK", Object::Dict(mk));
                return w;
            }
        };
        let mut n = Dict::new();
        n.set("N", Object::Dict(ap));
        w.set("AP", Object::Dict(n));
        w.set("MK", Object::Dict(mk));
        w
    }

    /// Puts `widgets` in this order among the page's annotations, and has
    /// Tab follow that order.
    fn reorder(&mut self, page: Ref, widgets: &[Ref]) {
        let annots = self.annots(page).0;
        let mapped: Vec<Ref> = widgets
            .iter()
            .map(|w| self.split.get(w).copied().unwrap_or(*w))
            .collect();
        let listed = |a: &Object| a.as_ref().is_some_and(|r| mapped.contains(&r));
        let present: Vec<Object> = mapped
            .iter()
            .filter(|w| annots.iter().any(|a| a.as_ref() == Some(**w)))
            .map(|w| Object::Ref(*w))
            .collect();
        // The widgets listed go together, in that order, where the first
        // of them stood; the rest keep theirs.
        let Some(first) = annots.iter().position(listed) else {
            return;
        };
        let mut annots: Vec<Object> = annots.into_iter().filter(|a| !listed(a)).collect();
        let at = first.min(annots.len());
        annots.splice(at..at, present);
        self.set_annots(page, annots);
        let mut dict = self.dict(page);
        // Row or column order would override the list's.
        if dict.get("Tabs").is_some() {
            dict.remove("Tabs");
            self.up.replace(page, Object::Dict(dict));
        }
    }
}

fn ft(kind: &NewField) -> &'static str {
    match kind {
        NewField::Text { .. } => "Tx",
        NewField::CheckBox | NewField::Radio { .. } | NewField::PushButton { .. } => "Btn",
        NewField::Choice { .. } => "Ch",
        NewField::Signature => "Sig",
    }
}

fn kind_ft(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text { .. } => "Tx",
        FieldKind::CheckBox | FieldKind::Radio | FieldKind::PushButton => "Btn",
        FieldKind::Choice { .. } => "Ch",
        FieldKind::Signature => "Sig",
    }
}

fn flags(kind: &NewField) -> i64 {
    match kind {
        NewField::Text {
            multiline,
            password,
            ..
        } => (if *multiline { MULTILINE } else { 0 }) | if *password { PASSWORD } else { 0 },
        NewField::Radio { .. } => RADIO | NO_TOGGLE_OFF,
        NewField::PushButton { .. } => PUSH_BUTTON,
        NewField::Choice {
            combo, editable, ..
        } => (if *combo { COMBO } else { 0 }) | if *combo && *editable { EDIT } else { 0 },
        _ => 0,
    }
}

/// A radio choice as the name of its "on" state.
fn state_name(choice: &str) -> String {
    let name: String = choice
        .chars()
        .filter(|c| c.is_ascii_graphic() && !"()<>[]{}/%#".contains(*c))
        .collect();
    if name.is_empty() || name == "Off" {
        "Choice".into()
    } else {
        name
    }
}

fn normal([x0, y0, x1, y1]: [f32; 4]) -> [f32; 4] {
    [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
}

fn reals(rect: &[f32; 4]) -> Object {
    Object::Array(rect.iter().map(|v| Object::Real(f64::from(*v))).collect())
}

fn font(base: &str) -> Object {
    let mut f = Dict::new();
    f.set("Type", Object::name("Font"));
    f.set("Subtype", Object::name("Type1"));
    f.set("BaseFont", Object::name(base));
    if base == "Helvetica" {
        f.set("Encoding", Object::name("WinAnsiEncoding"));
    }
    Object::Dict(f)
}

/// A form XObject the size of `rect` drawing `content`, with the fonts
/// new fields use.
fn xobject(rect: [f32; 4], content: String) -> Object {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut fonts = Dict::new();
    fonts.set("Helv", font("Helvetica"));
    fonts.set("ZaDb", font("ZapfDingbats"));
    let mut resources = Dict::new();
    resources.set("Font", Object::Dict(fonts));
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

/// The box's border, behind `inside`.
fn frame(rect: [f32; 4], inside: &str) -> Object {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    xobject(
        rect,
        format!(
            "q {BORDER} G 1 w 0.5 0.5 {:.2} {:.2} re S Q\n{inside}",
            (w - 1.0).max(0.0),
            (h - 1.0).max(0.0)
        ),
    )
}

/// How wide `text` is in Helvetica at `size`.
fn text_width(text: &str, size: f32) -> f32 {
    let (_, metrics) = stand_in("Helvetica");
    let units: f64 = text
        .chars()
        .map(|c| win_ansi_code(c).map_or(500.0, |(code, _)| win_ansi_width(metrics, code)))
        .sum();
    units as f32 / 1000.0 * size
}

/// A check box or radio button, `mark` drawn when it is on.
fn toggle(rect: [f32; 4], mark: Option<&str>, round: bool) -> Object {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut content = String::new();
    if round {
        let (cx, cy) = (w / 2.0, h / 2.0);
        let r = (w.min(h) / 2.0 - 0.5).max(0.5);
        content.push_str(&format!("q {BORDER} G 1 w {} S Q\n", circle(cx, cy, r)));
        if mark.is_some() {
            content.push_str(&format!("q 0 g {} f Q\n", circle(cx, cy, r * 0.45)));
        }
    } else {
        content.push_str(&format!(
            "q {BORDER} G 1 w 0.5 0.5 {:.2} {:.2} re S Q\n",
            (w - 1.0).max(0.0),
            (h - 1.0).max(0.0)
        ));
        if mark.is_some() {
            // Drawn as a path rather than ZapfDingbats' check, which not
            // every system has a font for.
            let line = (w.min(h) * 0.12).max(1.0);
            content.push_str(&format!(
                "q 0 G {line:.2} w 1 J 1 j {:.2} {:.2} m {:.2} {:.2} l {:.2} {:.2} l S Q\n",
                w * 0.22,
                h * 0.52,
                w * 0.42,
                h * 0.28,
                w * 0.78,
                h * 0.76
            ));
        }
    }
    xobject(rect, content)
}

/// A circle's path.
fn circle(cx: f32, cy: f32, r: f32) -> String {
    let k = r * 0.5523;
    format!(
        "{:.2} {cy:.2} m {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} {cy:.2} c {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} {cy:.2} c h",
        cx + r,
        cx + r,
        cy + k,
        cx + k,
        cy + r,
        cy + r,
        cx - k,
        cy + r,
        cx - r,
        cy + k,
        cx - r,
        cx - r,
        cy - k,
        cx - k,
        cy - r,
        cy - r,
        cx + k,
        cy - r,
        cx + r,
        cy - k,
        cx + r,
    )
}

/// A push button: a grey face with `caption` in the middle.
fn button(rect: [f32; 4], caption: &str) -> Object {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let mut size = (h * 0.6).clamp(4.0, 12.0);
    let wide = text_width(caption, size);
    if wide > w - 4.0 && wide > 0.0 {
        size = (size * (w - 4.0) / wide).max(4.0);
    }
    let x = (w - text_width(caption, size)) / 2.0;
    let y = (h - size * 0.7) / 2.0;
    xobject(
        rect,
        format!(
            "q {FACE} g 0 0 {w:.2} {h:.2} re f Q\nq {BORDER} G 1 w 0.5 0.5 {:.2} {:.2} re S Q\nq BT /Helv {size:.2} Tf 0 g {x:.2} {y:.2} Td {} Tj ET Q\n",
            (w - 1.0).max(0.0),
            (h - 1.0).max(0.0),
            literal(caption)
        ),
    )
}

/// A list box: its options a line each from the top, the chosen one
/// marked.
fn list_box(rect: [f32; 4], options: &[String], chosen: Option<&str>) -> Object {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let size = 10.0_f32.min(h - 2.0).max(4.0);
    let line = size * 1.15;
    let mut content = format!(
        "q {BORDER} G 1 w 0.5 0.5 {:.2} {:.2} re S Q\n/Tx BMC q 1 1 {:.2} {:.2} re W n\n",
        (w - 1.0).max(0.0),
        (h - 1.0).max(0.0),
        (w - 2.0).max(0.0),
        (h - 2.0).max(0.0)
    );
    let mut top = h - 1.0;
    for option in options {
        if top < line * 0.5 {
            break;
        }
        if chosen == Some(option.as_str()) {
            content.push_str(&format!(
                "q 0.6 0.75 0.95 rg 1 {:.2} {:.2} {line:.2} re f Q\n",
                top - line,
                w - 2.0
            ));
        }
        content.push_str(&format!(
            "BT /Helv {size:.2} Tf 0 g 3 {:.2} Td {} Tj ET\n",
            top - size,
            literal(option)
        ));
        top -= line;
    }
    content.push_str("Q EMC\n");
    xobject(rect, content)
}
