// SPDX-License-Identifier: GPL-3.0-or-later

//! The forms designer (docs/PLAN.md §4.3, Phase 4): with the form tool
//! from the edit row, fields of each kind are dragged out on the page,
//! clicked to change their name, tip, choices and flags, dragged to move
//! or by their corner to resize, shown on every page, taken out, and put
//! in the order Tab goes through them (`katna_pdf_cos::design`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{
    CalcOp, Calculation, Field, FieldEdit, FieldKind, FieldSpec, FormDesign, Format, NewField,
    Object, Ref, Scripts,
};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::annots::Draft;
use super::sign::check_box;
use super::{Content, Viewer};
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;
/// How near the bottom right corner of the field chosen, in points, a
/// press resizes it.
const HANDLE: f32 = 8.0;

/// The kinds of field, their names in the card, the start of a new
/// one's name, and the size a click (rather than a drag) gives it.
const KINDS: [(&str, &str, [f32; 2]); 8] = [
    ("form-kind-text", "Text", [160.0, 20.0]),
    ("form-kind-multiline", "Text", [200.0, 60.0]),
    ("form-kind-check", "Check", [14.0, 14.0]),
    ("form-kind-radio", "Group", [14.0, 14.0]),
    ("form-kind-combo", "Dropdown", [160.0, 20.0]),
    ("form-kind-list", "List", [160.0, 60.0]),
    ("form-kind-button", "Button", [80.0, 24.0]),
    ("form-kind-signature", "Signature", [180.0, 50.0]),
];

/// The ways a text field's value can be shown: plain, a number, an
/// amount in rupees, a percentage and a date.
const SHOWN: [&str; 5] = [
    "form-shown-plain",
    "form-shown-number",
    "form-shown-amount",
    "form-shown-percent",
    "form-shown-date",
];

fn shown_format(shown: usize) -> Option<Format> {
    let number = |currency: &str| Format::Number {
        decimals: 2,
        separators: 0,
        negative: 0,
        currency: currency.into(),
        currency_first: true,
    };
    match shown {
        1 => Some(number("")),
        2 => Some(number("Rs. ")),
        3 => Some(Format::Percent {
            decimals: 0,
            separators: 0,
        }),
        4 => Some(Format::Date("dd/mm/yyyy".into())),
        _ => None,
    }
}

/// Which of [`SHOWN`] `format` is, if one.
fn shown_index(format: Option<&Format>) -> Option<usize> {
    match format {
        None => Some(0),
        Some(Format::Number { currency, .. }) if currency.is_empty() => Some(1),
        Some(Format::Number { .. }) => Some(2),
        Some(Format::Percent { .. }) => Some(3),
        Some(Format::Date(_)) => Some(4),
        Some(Format::Special(_)) => None,
    }
}

/// The form tool, while in hand.
pub(super) struct Designing {
    /// Every field of the file shown, and each page's widgets in the
    /// order Tab goes through them.
    fields: Vec<Field>,
    order: HashMap<usize, Vec<Ref>>,
    scripts: HashMap<Ref, Scripts>,
    /// The kind of field (an index into [`KINDS`]) to drag out next.
    adding: Option<usize>,
    drawing: Option<Draft>,
    /// The field chosen, and the name of one to choose once the file is
    /// read again.
    chosen: Option<Ref>,
    choose_next: Option<String>,
    dragging: Option<Drag>,
    name: Entity<TextInput>,
    tip: Entity<TextInput>,
    options: Entity<TextInput>,
    /// The fields a total adds up, by name.
    total: Entity<TextInput>,
    _inputs: Vec<Subscription>,
}

/// A field being moved, or resized by its corner.
struct Drag {
    widget: Ref,
    page: usize,
    /// Where the press was and the pointer is, in points from the page's
    /// top left.
    from: [f32; 2],
    to: [f32; 2],
    /// The widget's box as shown, from the page's top left.
    shown: [f32; 4],
    resize: bool,
}

impl Drag {
    /// The box as dragged so far, from the page's top left.
    fn rect(&self) -> [f32; 4] {
        let (dx, dy) = (self.to[0] - self.from[0], self.to[1] - self.from[1]);
        let b = self.shown;
        if self.resize {
            [
                b[0],
                b[1],
                (b[2] + dx).max(b[0] + 6.0),
                (b[3] + dy).max(b[1] + 6.0),
            ]
        } else {
            [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy]
        }
    }
}

/// A file's fields as the form tool shows them.
pub(super) struct FormRead {
    fields: Vec<Field>,
    order: HashMap<usize, Vec<Ref>>,
    scripts: HashMap<Ref, Scripts>,
}

/// The fields of `doc`, each page's widgets in their order, and the
/// totals and formats fields have.
pub(super) fn read_form(doc: &katna_pdf_cos::Document) -> FormRead {
    let fields = doc.form();
    let widgets: std::collections::HashSet<Ref> = fields
        .iter()
        .flat_map(|f| f.widgets.iter().map(|w| w.id))
        .collect();
    let mut order = HashMap::new();
    for (i, page) in doc.pages().into_iter().enumerate() {
        let Some(Object::Dict(dict)) = doc.get(page) else {
            continue;
        };
        let Some(annots) = dict.get("Annots") else {
            continue;
        };
        if let Object::Array(annots) = doc.resolve(annots) {
            let listed: Vec<Ref> = annots
                .iter()
                .filter_map(Object::as_ref)
                .filter(|r| widgets.contains(r))
                .collect();
            if !listed.is_empty() {
                order.insert(i, listed);
            }
        }
    }
    let scripts = doc.form_scripts(&fields);
    FormRead {
        fields,
        order,
        scripts,
    }
}

impl Viewer {
    /// Takes the form tool in hand, or puts it down.
    pub(super) fn toggle_designing(&mut self, cx: &mut Context<Self>) {
        if self.designing.take().is_some() {
            cx.notify();
            return;
        }
        self.pick_adding(None, cx);
        self.marking = None;
        self.properties = None;
        self.redacting = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = |placeholder: &str, cx: &mut Context<Self>| {
            let placeholder = t(placeholder, &[]);
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input
            })
        };
        let name = input("form-name", cx);
        let tip = input("form-tip", cx);
        let options = input("form-options", cx);
        let total = input("form-total-hint", cx);
        let inputs = [&name, &tip, &options, &total]
            .into_iter()
            .map(|i| {
                cx.subscribe(i, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.apply_field_edit(cx);
                    }
                })
            })
            .collect();
        let read = match &self.form {
            Some(form) => read_form(&form.source),
            None => FormRead {
                fields: Vec::new(),
                order: HashMap::new(),
                scripts: HashMap::new(),
            },
        };
        self.designing = Some(Designing {
            fields: read.fields,
            order: read.order,
            scripts: read.scripts,
            adding: None,
            drawing: None,
            chosen: None,
            choose_next: None,
            dragging: None,
            name,
            tip,
            options,
            total,
            _inputs: inputs,
        });
        // Fields nobody can fill (buttons, signatures) are only read when
        // the file is.
        if self.form.is_none() {
            self.read_design(cx);
        }
        cx.notify();
    }

    /// Reads the fields of the file shown into the form tool.
    fn read_design(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    Ok::<_, String>(read_form(&doc))
                })
                .await;
            this.update(cx, |this, cx| {
                if let (Some(designing), Ok(read)) = (this.designing.as_mut(), read) {
                    designing.fields = read.fields;
                    designing.order = read.order;
                    designing.scripts = read.scripts;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The file shown changed: the form tool reads its fields again, and
    /// chooses the one just added.
    pub(super) fn design_changed(&mut self, read: FormRead, cx: &mut Context<Self>) {
        let Some(designing) = self.designing.as_mut() else {
            return;
        };
        designing.fields = read.fields;
        designing.order = read.order;
        designing.scripts = read.scripts;
        if let Some(name) = designing.choose_next.take() {
            designing.chosen = designing
                .fields
                .iter()
                .find(|f| f.name == name)
                .map(|f| f.id);
        }
        if designing
            .chosen
            .is_some_and(|c| !designing.fields.iter().any(|f| f.id == c))
        {
            designing.chosen = None;
        }
        self.fill_field_inputs(cx);
    }

    /// Puts the chosen field's name, tip and choices in the card.
    fn fill_field_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(designing) = self.designing.as_ref() else {
            return;
        };
        let field = designing
            .chosen
            .and_then(|c| designing.fields.iter().find(|f| f.id == c));
        let name = field
            .map(|f| f.name.rsplit('.').next().unwrap_or_default().to_owned())
            .unwrap_or_default();
        let tip = field.and_then(|f| f.tooltip.clone()).unwrap_or_default();
        let options = match field.map(|f| &f.kind) {
            Some(FieldKind::Choice { options, .. }) => options.join(", "),
            _ => String::new(),
        };
        let total = match field
            .and_then(|f| designing.scripts.get(&f.id))
            .and_then(|s| s.calculate.as_ref())
        {
            Some(Calculation::Simple { fields, .. }) => fields.join(", "),
            None => String::new(),
        };
        for (input, value) in [
            (designing.name.clone(), name),
            (designing.tip.clone(), tip),
            (designing.options.clone(), options),
            (designing.total.clone(), total),
        ] {
            input.update(cx, |i, cx| i.set_text(value, cx));
        }
    }

    fn chosen_field(&self) -> Option<&Field> {
        let designing = self.designing.as_ref()?;
        let chosen = designing.chosen?;
        designing.fields.iter().find(|f| f.id == chosen)
    }

    /// The widget on page `page` under `point` (from the page's top
    /// left), its field, and its box as shown.
    fn widget_at(&self, page: usize, point: [f32; 2]) -> Option<(Ref, Ref, [f32; 4])> {
        let designing = self.designing.as_ref()?;
        let transform = self.design_transform(page)?;
        // The field chosen first, then those drawn last.
        let chosen = designing.chosen;
        let mut hits = designing.fields.iter().flat_map(|f| {
            f.widgets
                .iter()
                .filter(|w| w.page == page)
                .map(move |w| (f.id, w.id, katna_pdf_render::map_rect(transform, w.rect)))
        });
        let inside = |b: &[f32; 4]| {
            point[0] >= b[0] - 2.0
                && point[0] <= b[2] + 2.0
                && point[1] >= b[1] - 2.0
                && point[1] <= b[3] + 2.0
        };
        let all: Vec<_> = hits.by_ref().filter(|(_, _, b)| inside(b)).collect();
        all.iter()
            .find(|(f, _, _)| Some(*f) == chosen)
            .or(all.last())
            .map(|&(f, w, b)| (f, w, b))
    }

    /// A press on the pages with the form tool in hand: starts dragging
    /// out a new field, or chooses the one under it to move or resize.
    /// Returns whether the tool is in hand.
    pub(super) fn design_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.designing.is_none() {
            return false;
        }
        let Some((page, point)) = self.page_point(position) else {
            return true;
        };
        let adding = self.designing.as_ref().and_then(|d| d.adding);
        if adding.is_some() {
            if let Some(designing) = self.designing.as_mut() {
                designing.drawing = Some(Draft {
                    page,
                    path: vec![point],
                });
            }
            cx.notify();
            return true;
        }
        let hit = self.widget_at(page, point);
        let before = self.designing.as_ref().and_then(|d| d.chosen);
        if let Some(designing) = self.designing.as_mut() {
            match hit {
                Some((field, widget, shown)) => {
                    let resize = before == Some(field)
                        && (point[0] - shown[2]).abs() <= HANDLE
                        && (point[1] - shown[3]).abs() <= HANDLE;
                    designing.chosen = Some(field);
                    designing.dragging = Some(Drag {
                        widget,
                        page,
                        from: point,
                        to: point,
                        shown,
                        resize,
                    });
                }
                None => designing.chosen = None,
            }
        }
        if before != hit.map(|h| h.0) {
            self.fill_field_inputs(cx);
        }
        cx.notify();
        true
    }

    pub(super) fn design_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(designing) = self.designing.as_ref() else {
            return false;
        };
        let page = match (&designing.drawing, &designing.dragging) {
            (Some(d), _) => d.page,
            (_, Some(d)) => d.page,
            _ => return false,
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            ((position.x - b.left()) / shown).clamp(0.0, (b.size.width / shown).max(0.0)),
            ((position.y - b.top()) / shown).clamp(0.0, (b.size.height / shown).max(0.0)),
        ];
        if let Some(designing) = self.designing.as_mut() {
            if let Some(draft) = designing.drawing.as_mut() {
                draft.path.truncate(1);
                draft.path.push(point);
            }
            if let Some(drag) = designing.dragging.as_mut() {
                drag.to = point;
            }
        }
        cx.notify();
        true
    }

    /// The pointer let go: a field dragged out is added, one dragged is
    /// moved or resized.
    pub(super) fn design_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(designing) = self.designing.as_mut() else {
            return false;
        };
        let (drawing, dragging) = (designing.drawing.take(), designing.dragging.take());
        cx.notify();
        if let Some(draft) = drawing {
            self.add_field(draft, cx);
            return true;
        }
        let Some(drag) = dragging else {
            return false;
        };
        let moved = (drag.to[0] - drag.from[0]).abs() >= CLICK
            || (drag.to[1] - drag.from[1]).abs() >= CLICK;
        let Some(inv) = self.inverse(drag.page).filter(|_| moved) else {
            return true;
        };
        let design = FormDesign {
            place: vec![(drag.widget, inv.rect(drag.rect()))],
            ..FormDesign::default()
        };
        self.design(vec![drag.page], design, cx);
        true
    }

    /// Adds a field of the kind in hand where `draft` was dragged out (or
    /// clicked, at its usual size).
    fn add_field(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let Some(designing) = self.designing.as_ref() else {
            return;
        };
        let Some(kind) = designing.adding else {
            return;
        };
        let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last()) else {
            return;
        };
        let (_, base, size) = KINDS[kind];
        let shown = if (to[0] - from[0]).abs() < CLICK || (to[1] - from[1]).abs() < CLICK {
            [from[0], from[1], from[0] + size[0], from[1] + size[1]]
        } else {
            [
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ]
        };
        let Some(inv) = self.inverse(draft.page) else {
            return;
        };
        let taken = |name: &str| designing.fields.iter().any(|f| f.name == name);
        let fresh = || {
            (1..)
                .map(|n| format!("{base}{n}"))
                .find(|n| !taken(n))
                .unwrap_or_default()
        };
        // A radio button joins the group chosen, as one more choice.
        let group = self
            .chosen_field()
            .filter(|f| kind == 3 && f.kind == FieldKind::Radio);
        let (name, choice) = match group {
            Some(f) => (f.name.clone(), format!("Choice{}", f.widgets.len() + 1)),
            None => (fresh(), "Choice1".to_string()),
        };
        let options = || {
            vec![
                t("form-option", &[("n", "1")]),
                t("form-option", &[("n", "2")]),
            ]
        };
        let kind = match kind {
            0 | 1 => NewField::Text {
                multiline: kind == 1,
                password: false,
                max_len: None,
            },
            2 => NewField::CheckBox,
            3 => NewField::Radio { choice },
            4 | 5 => NewField::Choice {
                options: options(),
                combo: kind == 4,
                editable: false,
            },
            6 => NewField::PushButton {
                caption: t("form-button-caption", &[]),
            },
            _ => NewField::Signature,
        };
        let design = FormDesign {
            add: vec![FieldSpec {
                page: draft.page,
                rect: inv.rect(shown),
                name: name.clone(),
                kind,
                tooltip: String::new(),
                required: false,
                read_only: false,
                value: None,
                scripts: katna_pdf_cos::Scripts::default(),
            }],
            ..FormDesign::default()
        };
        if let Some(designing) = self.designing.as_mut() {
            designing.adding = None;
            designing.choose_next = Some(name);
        }
        self.design(vec![draft.page], design, cx);
    }

    /// Changes the file's form as `design` says, `pages` drawn again.
    fn design(&mut self, pages: Vec<usize>, design: FormDesign, cx: &mut Context<Self>) {
        self.change_document(
            pages,
            "form-design-failed",
            move |doc| doc.design_form(&design).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// The pages field `field` is shown on.
    fn pages_of(field: &Field) -> Vec<usize> {
        let mut pages: Vec<usize> = field.widgets.iter().map(|w| w.page).collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    /// Saves the name, tip and choices typed for the field chosen.
    fn apply_field_edit(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.chosen_field().cloned() else {
            return;
        };
        let Some(designing) = self.designing.as_ref() else {
            return;
        };
        let name = designing.name.read(cx).text().trim().to_owned();
        let tip = designing.tip.read(cx).text().trim().to_owned();
        let options: Vec<String> = designing
            .options
            .read(cx)
            .text()
            .split(',')
            .map(|o| o.trim().to_owned())
            .filter(|o| !o.is_empty())
            .collect();
        let mut edit = FieldEdit::new(field.id);
        let own = field.name.rsplit('.').next().unwrap_or_default();
        if !name.is_empty() && name != own {
            edit.name = Some(name);
        }
        if field.tooltip.as_deref().unwrap_or("") != tip {
            edit.tooltip = Some(tip);
        }
        if let FieldKind::Choice { options: was, .. } = &field.kind
            && !options.is_empty()
            && *was != options
        {
            edit.options = Some(options);
        }
        if matches!(field.kind, FieldKind::Text { .. }) {
            let adds: Vec<String> = designing
                .total
                .read(cx)
                .text()
                .split(',')
                .map(|o| o.trim().to_owned())
                .filter(|o| !o.is_empty())
                .collect();
            let mut scripts = designing
                .scripts
                .get(&field.id)
                .cloned()
                .unwrap_or_default();
            let was = scripts.calculate.clone();
            scripts.calculate = (!adds.is_empty()).then_some(Calculation::Simple {
                op: match &was {
                    Some(Calculation::Simple { op, .. }) => *op,
                    None => CalcOp::Sum,
                },
                fields: adds,
            });
            if scripts.calculate != was {
                edit.scripts = Some(scripts);
            }
        }
        if edit == FieldEdit::new(field.id) {
            return;
        }
        self.edit_field(&field, edit, cx);
    }

    /// Shows the field chosen as format `shown` (an index into
    /// [`SHOWN`]).
    fn set_shown(&mut self, shown: usize, cx: &mut Context<Self>) {
        let Some(field) = self.chosen_field().cloned() else {
            return;
        };
        let Some(designing) = self.designing.as_ref() else {
            return;
        };
        let mut scripts = designing
            .scripts
            .get(&field.id)
            .cloned()
            .unwrap_or_default();
        let format = shown_format(shown);
        if scripts.format == format {
            return;
        }
        scripts.format = format;
        let mut edit = FieldEdit::new(field.id);
        edit.scripts = Some(scripts);
        self.edit_field(&field, edit, cx);
    }

    fn edit_field(&mut self, field: &Field, edit: FieldEdit, cx: &mut Context<Self>) {
        let design = FormDesign {
            edit: vec![edit],
            ..FormDesign::default()
        };
        self.design(Self::pages_of(field), design, cx);
    }

    /// Turns a flag of the field chosen on or off.
    fn toggle_flag(&mut self, flag: usize, cx: &mut Context<Self>) {
        let Some(field) = self.chosen_field().cloned() else {
            return;
        };
        let mut edit = FieldEdit::new(field.id);
        match flag {
            0 => edit.required = Some(!field.required),
            1 => edit.read_only = Some(!field.read_only),
            _ => {
                if let FieldKind::Text { multiline, .. } = field.kind {
                    edit.multiline = Some(!multiline);
                }
            }
        }
        self.edit_field(&field, edit, cx);
    }

    /// Shows the field chosen on every page it is not on yet, at the same
    /// place.
    fn copy_to_every_page(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.chosen_field().cloned() else {
            return;
        };
        let count = match &self.content {
            Content::Ready(opened) => opened.doc.page_count(),
            _ => return,
        };
        let on = Self::pages_of(&field);
        let to: Vec<usize> = (0..count).filter(|p| !on.contains(p)).collect();
        if to.is_empty() {
            return;
        }
        let design = FormDesign {
            copy: vec![(field.id, to.clone())],
            ..FormDesign::default()
        };
        self.design(to, design, cx);
    }

    fn remove_field(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.chosen_field().cloned() else {
            return;
        };
        if let Some(designing) = self.designing.as_mut() {
            designing.chosen = None;
        }
        let design = FormDesign {
            remove: vec![field.id],
            ..FormDesign::default()
        };
        self.design(Self::pages_of(&field), design, cx);
    }

    /// Adds the fields every page of a flat form seems to ask for:
    /// blanks, boxes and squares to tick, named from the words beside
    /// them.
    fn find_fields(&mut self, cx: &mut Context<Self>) {
        let count = match &self.content {
            Content::Ready(opened) => opened.doc.page_count(),
            _ => return,
        };
        let none = t("form-none-found", &[]);
        self.change_document(
            (0..count).collect(),
            "form-design-failed",
            move |doc| {
                let mut taken: Vec<String> = doc.form().into_iter().map(|f| f.name).collect();
                let mut add = Vec::new();
                for page in 0..doc.pages().len() {
                    let found = doc.detect_fields(page, &taken);
                    taken.extend(found.iter().map(|s| s.name.clone()));
                    add.extend(found);
                }
                if add.is_empty() {
                    return Err(none);
                }
                doc.design_form(&FormDesign {
                    add,
                    ..FormDesign::default()
                })
                .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Moves widget `widget` one place earlier in page `page`'s Tab
    /// order.
    fn tab_earlier(&mut self, page: usize, widget: Ref, cx: &mut Context<Self>) {
        let Some(mut order) = self
            .designing
            .as_ref()
            .and_then(|d| d.order.get(&page).cloned())
        else {
            return;
        };
        let Some(at) = order.iter().position(|w| *w == widget).filter(|&i| i > 0) else {
            return;
        };
        order.swap(at, at - 1);
        let design = FormDesign {
            order: vec![(page, order)],
            ..FormDesign::default()
        };
        self.design(vec![page], design, cx);
    }

    /// Page `page`'s fields, outlined with their names; the one chosen
    /// with a corner to resize it by; and one being dragged out.
    pub(super) fn design_overlay(&self, page: usize) -> Vec<AnyElement> {
        let Some(designing) = self.designing.as_ref() else {
            return Vec::new();
        };
        let Some(transform) = self.design_transform(page) else {
            return Vec::new();
        };
        let shown = self.shown;
        let th = self.theme;
        let accent = th.katna.accent & 0xffff_ff00;
        let place = |b: [f32; 4]| {
            div()
                .absolute()
                .left(desktop_px(b[0] * shown))
                .top(desktop_px(b[1] * shown))
                .w(desktop_px((b[2] - b[0]) * shown))
                .h(desktop_px((b[3] - b[1]) * shown))
        };
        let mut out = Vec::new();
        for field in &designing.fields {
            let chosen = designing.chosen == Some(field.id);
            for w in field.widgets.iter().filter(|w| w.page == page) {
                let mut b = katna_pdf_render::map_rect(transform, w.rect);
                if let Some(drag) = designing.dragging.as_ref().filter(|d| d.widget == w.id) {
                    b = drag.rect();
                }
                let mut area = place(b)
                    .border_color(gpui::rgba(accent | 0xff))
                    .bg(gpui::rgba(accent | if chosen { 0x30 } else { 0x14 }));
                area = if chosen {
                    area.border_2()
                } else {
                    area.border_1()
                };
                out.push(
                    area.child(
                        div()
                            .absolute()
                            .bottom_full()
                            .left_0()
                            .px(px(space::S1))
                            .rounded(px(radius::XS))
                            .bg(gpui::rgba(accent | 0xff))
                            .text_color(gpui::white())
                            .text_size(px(text::CAPTION))
                            .whitespace_nowrap()
                            .child(field.name.clone()),
                    )
                    .when(chosen, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(-5.0))
                                .bottom(px(-5.0))
                                .size(px(10.0))
                                .rounded(px(radius::XS))
                                .bg(gpui::rgba(accent | 0xff))
                                .cursor(gpui::CursorStyle::ResizeUpLeftDownRight),
                        )
                    })
                    .into_any_element(),
                );
            }
        }
        if let Some(draft) = designing.drawing.as_ref().filter(|d| d.page == page)
            && let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last())
        {
            let b = [
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ];
            out.push(
                place(b)
                    .border_1()
                    .border_dashed()
                    .border_color(gpui::rgba(accent | 0xff))
                    .into_any_element(),
            );
        }
        out
    }

    fn design_transform(&self, page: usize) -> Option<[f64; 6]> {
        match &self.content {
            Content::Ready(opened) => opened.doc.page_transform(page),
            _ => None,
        }
    }

    /// The form card, while the tool is in hand.
    pub(super) fn design_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let designing = self.designing.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let chip = |id: (&'static str, usize), on: bool, label: String| {
            div()
                .id(id)
                .h(px(30.0))
                .px(px(space::S3))
                .flex()
                .items_center()
                .rounded(px(radius::FULL))
                .border_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .when(on, |d| {
                    d.bg(th.field).border_color(gpui::rgba(th.katna.accent))
                })
                .hover(move |s| s.bg(th.hover))
                .cursor_pointer()
                .child(label)
        };
        let mut kinds = div().flex().flex_wrap().gap(px(space::S1));
        for (i, (id, _, _)) in KINDS.iter().enumerate() {
            kinds = kinds.child(
                chip(("form-kind", i), designing.adding == Some(i), t(id, &[])).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.designing.as_mut() {
                            d.adding = if d.adding == Some(i) { None } else { Some(i) };
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        let input = |entity: Entity<TextInput>| {
            div()
                .h(px(34.0))
                .flex()
                .items_center()
                .px(px(space::S3))
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::BODY))
                .text_color(th.text)
                .child(entity)
        };
        let toggle = |i: usize, on: bool, id: &str| {
            div()
                .id(("form-flag", i))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_flag(i, cx)))
                .child(check_box(on, th))
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(t(id, &[])),
                )
        };
        let mut card = div()
            .id("form-card")
            .absolute()
            .top(px(space::S3))
            .right(px(space::S5))
            .w(px(340.0))
            .max_h(px(620.0))
            .overflow_y_scroll()
            .p(px(space::S4))
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow(widgets::elevation(&th.katna, 2.0))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(text::BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(t("form-title", &[])),
            )
            .child(label(if designing.adding.is_some() {
                "form-hint-adding"
            } else {
                "form-hint"
            }))
            .child(kinds)
            .child(
                div().flex().child(
                    widgets::outlined_button("form-find", t("form-find", &[]), &th.katna)
                        .tip(t("form-find-tip", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.find_fields(cx))),
                ),
            );
        if let Some(field) = self.chosen_field() {
            card = card
                .child(div().h(px(1.0)).my(px(space::S1)).bg(th.line))
                .child(label("form-name"))
                .child(input(designing.name.clone()))
                .child(label("form-tip"))
                .child(input(designing.tip.clone()));
            if matches!(field.kind, FieldKind::Choice { .. }) {
                card = card
                    .child(label("form-options"))
                    .child(input(designing.options.clone()));
            }
            card = card
                .child(toggle(0, field.required, "form-required"))
                .child(toggle(1, field.read_only, "form-read-only"));
            if let FieldKind::Text { multiline, .. } = field.kind {
                card = card.child(toggle(2, multiline, "form-multiline"));
                let scripts = designing.scripts.get(&field.id);
                let now = scripts.and_then(|s| s.format.as_ref());
                let mut shown = div().flex().flex_wrap().gap(px(space::S1));
                for (i, id) in SHOWN.iter().enumerate() {
                    shown = shown.child(
                        chip(("form-shown", i), shown_index(now) == Some(i), t(id, &[]))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_shown(i, cx))),
                    );
                }
                card = card
                    .child(label("form-shown"))
                    .child(shown)
                    .child(label("form-total"))
                    .child(input(designing.total.clone()));
            }
            card = card.child(
                div()
                    .pt(px(space::S1))
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S2))
                    .child(
                        widgets::outlined_button(
                            "form-every-page",
                            t("form-every-page", &[]),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.copy_to_every_page(cx))),
                    )
                    .child(
                        widgets::outlined_button("form-delete", t("form-delete", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.remove_field(cx))),
                    )
                    .child(
                        widgets::filled_button("form-save", t("form-save", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_field_edit(cx))),
                    ),
            );
        }
        // The Tab order of the page shown.
        let page = self.current_page();
        let names: HashMap<Ref, &str> = designing
            .fields
            .iter()
            .flat_map(|f| f.widgets.iter().map(move |w| (w.id, f.name.as_str())))
            .collect();
        if let Some(order) = designing.order.get(&page).filter(|o| o.len() > 1) {
            card = card
                .child(div().h(px(1.0)).my(px(space::S1)).bg(th.line))
                .child(label("form-tab-order"));
            for (i, &widget) in order.iter().enumerate() {
                let field = designing
                    .fields
                    .iter()
                    .find(|f| f.widgets.iter().any(|w| w.id == widget))
                    .map(|f| f.id);
                let chosen = field.is_some() && field == designing.chosen;
                card = card.child(
                    div()
                        .id(("form-tab", i))
                        .h(px(30.0))
                        .px(px(space::S2))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .rounded(px(radius::SM))
                        .when(chosen, |d| d.bg(th.field))
                        .hover(move |s| s.bg(th.hover))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(d) = this.designing.as_mut() {
                                d.chosen = field;
                            }
                            this.fill_field_inputs(cx);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .w(px(20.0))
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child((i + 1).to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(names.get(&widget).copied().unwrap_or("").to_owned()),
                        )
                        .when(i > 0, |d| {
                            d.child(
                                widgets::icon_button_colored(
                                    ("form-tab-up", i),
                                    "arrow-up",
                                    16.0,
                                    th.katna.text_dim,
                                    &th.katna,
                                )
                                .tip(t("form-tab-earlier", &[]), &th.katna)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.tab_earlier(page, widget, cx);
                                    },
                                )),
                            )
                        }),
                );
            }
        }
        card = card.child(
            div()
                .pt(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S1))
                .child(
                    widgets::icon_button("form-import", "upload", 18.0, &th.katna)
                        .tip(t("form-import", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.import_form_data(cx))),
                )
                .child(
                    widgets::icon_button("form-export", "download", 18.0, &th.katna)
                        .tip(t("form-export", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.export_form_data(cx))),
                )
                .child(div().flex_1())
                .child(
                    widgets::outlined_button("form-done", t("form-done", &[]), &th.katna).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.designing = None;
                            cx.notify();
                        }),
                    ),
                ),
        );
        Some(card.into_any_element())
    }

    /// Asks where to save the form's data, as JSON (or CSV, XFDF or FDF,
    /// when the name given ends so), and saves it there.
    fn export_form_data(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&folder, Some(&format!("{stem}.json")));
        let pdf = super::file_name(&path);
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    let text = doc.write_data(&target.to_string_lossy(), &pdf);
                    std::fs::write(&target, text).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t("form-exported", &[("name", &super::file_name(&target))]),
                    Err(error) => t("form-design-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a file of form data (JSON, CSV, XFDF or FDF) and fills
    /// the form in
    /// from it.
    fn import_form_data(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| this.fill_from(file, cx)).ok();
        })
        .detach();
    }

    /// Fills the form in from the form data file `file`.
    pub(super) fn fill_from(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let pages = self
            .designing
            .as_ref()
            .map(|d| {
                let mut pages: Vec<usize> = d
                    .fields
                    .iter()
                    .flat_map(|f| f.widgets.iter().map(|w| w.page))
                    .collect();
                pages.sort_unstable();
                pages.dedup();
                pages
            })
            .unwrap_or_default();
        self.change_document(
            pages,
            "form-design-failed",
            move |doc| {
                let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                let data = katna_pdf_cos::read_data(&file.to_string_lossy(), &bytes)
                    .map_err(|e| e.to_string())?;
                doc.fill_data(&data).map_err(|e| e.to_string())
            },
            cx,
        );
    }
}
