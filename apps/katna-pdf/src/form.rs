// SPDX-License-Identifier: GPL-3.0-or-later

//! Filling in a PDF's form (docs/PLAN.md, Phase 1, form filling and
//! save): text fields, check boxes and radio buttons. Each change is
//! written into the file as an incremental update right away and the
//! pages are drawn again from it, so what shows is what other readers
//! will show; Save writes that file over the one opened.
//!
//! A form that carries its own JavaScript has it run (`katna-pdf-js`):
//! checks on what is typed, totals, formats, fields shown and hidden,
//! and buttons' scripts. Otherwise the totals and formats Acrobat's tools
//! write are done in Rust (cos `calc.rs`).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Entity, Subscription};
use katna_pdf_cos::{
    Calculation, Display, Document, Field, FieldAction, FieldKind, FieldValue, FillExtras, Format,
    Ref, Widget,
};
use katna_pdf_js::{Outcome, Request};
use katna_ui::text_input::TextInput;

pub struct Form {
    /// The file as last opened or saved; changes are made to it.
    pub source: Arc<Document>,
    pub fields: Vec<Field>,
    /// For each page, its fillable widgets: indices into `fields` and
    /// their widgets.
    on_page: HashMap<usize, Vec<(usize, usize)>>,
    /// Fields worked out from others (totals), with how.
    calculated: Vec<(usize, Calculation)>,
    /// Fields that hold a date, with how it is written (`dd/mm/yyyy`).
    dates: HashMap<usize, String>,
    /// What buttons do when clicked.
    actions: HashMap<usize, FieldAction>,
    /// Values changed since the file was opened or saved.
    pub changes: Vec<(Ref, FieldValue)>,
    /// The file with the changes made, once written.
    pub filled: Option<Arc<Vec<u8>>>,
    /// The text field being typed in.
    pub editing: Option<Editing>,
    /// Bumped for each change, so an older fill that ends later is
    /// dropped.
    pub generation: u64,
    /// The form's own scripts, when it has any and they may run.
    scripts: Option<katna_pdf_js::Form>,
    /// What the scripts did since the file was opened or saved, for the
    /// fields they showed, hid or locked.
    outcomes: Vec<Outcome>,
    /// The fields the last change touched, whose pages are drawn again.
    touched: Vec<usize>,
}

/// What setting a value or clicking did, for the viewer to show and act
/// on.
#[derive(Debug, Default)]
pub struct Done {
    /// The form changed and its pages need filling again.
    pub changed: bool,
    /// What the form's scripts said (`app.alert`).
    pub alerts: Vec<String>,
    /// What they asked for: an address opened, the form sent, a page.
    pub requests: Vec<Request>,
}

pub struct Editing {
    pub field: usize,
    pub widget: usize,
    pub input: Entity<TextInput>,
    pub _input: Subscription,
    /// For a date field, the month its calendar shows (year, month).
    pub month: Option<(i16, i8)>,
}

impl Form {
    /// The form of `source`, if it has any field that can be filled.
    /// With `scripts`, its own JavaScript runs.
    pub fn new(
        source: Arc<Document>,
        fields: Vec<Field>,
        scripts: bool,
        file: &str,
    ) -> Option<Self> {
        let js = scripts
            .then(|| katna_pdf_js::Form::new(&source, &fields, file))
            .flatten();
        let actions: HashMap<usize, FieldAction> = fields
            .iter()
            .enumerate()
            .filter(|(_, f)| !f.read_only)
            .filter_map(|(i, f)| Some((i, source.field_action(f)?)))
            .collect();
        let on_page = widgets_on_pages(&fields, &actions, js.as_ref());
        let found = source.form_scripts(&fields);
        // Scripts that run work totals out themselves.
        let calculated = if js.is_some() {
            Vec::new()
        } else {
            fields
                .iter()
                .enumerate()
                .filter_map(|(f, field)| Some((f, found.get(&field.id)?.calculate.clone()?)))
                .collect()
        };
        let dates = fields
            .iter()
            .enumerate()
            .filter_map(|(f, field)| match found.get(&field.id)?.format.as_ref()? {
                Format::Date(pattern) => Some((f, pattern.clone())),
                _ => None,
            })
            .collect();
        (!on_page.is_empty()).then_some(Self {
            source,
            fields,
            on_page,
            calculated,
            dates,
            actions,
            changes: Vec::new(),
            filled: None,
            editing: None,
            generation: 0,
            scripts: js,
            outcomes: Vec::new(),
            touched: Vec::new(),
        })
    }

    /// The fillable widgets on `page`.
    pub fn on_page(&self, page: usize) -> impl Iterator<Item = (usize, usize, &Widget)> {
        self.on_page
            .get(&page)
            .into_iter()
            .flatten()
            .map(|&(f, w)| (f, w, &self.fields[f].widgets[w]))
    }

    /// What field `field` does when clicked, if anything.
    pub fn action(&self, field: usize) -> Option<&FieldAction> {
        self.actions.get(&field)
    }

    /// Whether clicking field `field` runs a script of the form's.
    pub fn clicks(&self, field: usize) -> bool {
        self.scripts
            .as_ref()
            .is_some_and(|js| js.clicks(&self.fields[field].name))
    }

    /// Field `field` was clicked: its scripts run.
    pub fn click(&mut self, field: usize) -> Done {
        let name = self.fields[field].name.clone();
        let Some(js) = self.scripts.as_mut() else {
            return Done::default();
        };
        let outcome = js.click(&name);
        self.apply(outcome, Some(field))
    }

    /// How field `field` writes its date, if it holds one.
    pub fn date_pattern(&self, field: usize) -> Option<&str> {
        self.dates.get(&field).map(String::as_str)
    }

    /// Whether there are changes not saved.
    pub fn dirty(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Sets field `field` to `value`: the form's scripts may turn it
    /// down, and work out what follows from it.
    pub fn set(&mut self, field: usize, value: FieldValue) -> Done {
        let f = &self.fields[field];
        let new = match &value {
            FieldValue::Text(text) => Some(text.clone()).filter(|t| !t.is_empty()),
            FieldValue::State(state) => state.clone(),
        };
        if f.value == new {
            return Done::default();
        }
        if let Some(js) = self.scripts.as_mut() {
            let text = match &value {
                FieldValue::Text(t) => t.clone(),
                FieldValue::State(s) => s.clone().unwrap_or_else(|| "Off".into()),
            };
            let name = f.name.clone();
            let outcome = js.commit(&name, &text);
            if !outcome.accepted {
                return Done {
                    changed: false,
                    alerts: outcome.alerts,
                    requests: outcome.requests,
                };
            }
            return self.apply(outcome, Some(field));
        }
        let f = &mut self.fields[field];
        f.value = new;
        let id = f.id;
        self.changes.retain(|(r, _)| *r != id);
        self.changes.push((id, value));
        self.generation += 1;
        self.touched = vec![field];
        self.recalculate();
        Done {
            changed: true,
            ..Done::default()
        }
    }

    /// Takes what the form's scripts did into the fields here.
    fn apply(&mut self, outcome: Outcome, field: Option<usize>) -> Done {
        let index = |fields: &[Field], name: &str| fields.iter().position(|f| f.name == name);
        let mut touched: Vec<usize> = field.into_iter().collect();
        for e in &outcome.errors {
            eprintln!("katna-pdf: form script: {e}");
        }
        for (name, value) in &outcome.values {
            let Some(i) = index(&self.fields, name) else {
                continue;
            };
            let f = &mut self.fields[i];
            let value = katna_pdf_js::field_value(&f.kind, value);
            f.value = match &value {
                FieldValue::Text(t) => Some(t.clone()).filter(|t| !t.is_empty()),
                FieldValue::State(s) => s.clone(),
            };
            let id = f.id;
            self.changes.retain(|(r, _)| *r != id);
            self.changes.push((id, value));
            touched.push(i);
        }
        let mut shown = false;
        for (name, display) in &outcome.display {
            if let Some(i) = index(&self.fields, name) {
                let hidden = matches!(display, Display::Hidden | Display::NoView);
                for w in &mut self.fields[i].widgets {
                    w.hidden = hidden;
                }
                touched.push(i);
                shown = true;
            }
        }
        for (name, locked) in &outcome.read_only {
            if let Some(i) = index(&self.fields, name) {
                self.fields[i].read_only = *locked;
                touched.push(i);
                shown = true;
            }
        }
        if shown {
            self.on_page = widgets_on_pages(&self.fields, &self.actions, self.scripts.as_ref());
        }
        let changed = !outcome.values.is_empty() || shown;
        let done = Done {
            changed,
            alerts: outcome.alerts.clone(),
            requests: outcome.requests.clone(),
        };
        if changed {
            if shown && self.changes.is_empty() {
                // Shown or hidden only: still a change to the file.
                if let Some(f) = field {
                    let f = &self.fields[f];
                    let value = match f.kind {
                        FieldKind::CheckBox | FieldKind::Radio => {
                            FieldValue::State(f.value.clone())
                        }
                        _ => FieldValue::Text(f.value.clone().unwrap_or_default()),
                    };
                    self.changes.push((f.id, value));
                }
            }
            self.outcomes.push(outcome);
            self.generation += 1;
            touched.sort_unstable();
            touched.dedup();
            self.touched = touched;
        }
        done
    }

    /// What the form's scripts showed, hid and locked, for filling the
    /// file.
    pub fn extras(&mut self) -> FillExtras {
        match self.scripts.as_mut() {
            Some(js) => js.extras(&self.outcomes),
            None => FillExtras::default(),
        }
    }

    /// Works totals out again from the values now, as filling the file
    /// does, so what is shown and typed over agrees with the pages.
    fn recalculate(&mut self) {
        // Twice, so a total of totals catches up whatever their order.
        for _ in 0..2 {
            for (f, calc) in &self.calculated {
                let values = self
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), f.value.clone().unwrap_or_default()))
                    .collect();
                let result = katna_pdf_cos::calculate(calc, &values);
                self.fields[*f].value = Some(result).filter(|r| !r.is_empty());
            }
        }
    }

    /// The value a click on widget `widget` of check box or radio group
    /// `field` gives it: a check box turns over, a radio button turns on.
    pub fn clicked(&self, field: usize, widget: usize) -> Option<FieldValue> {
        let f = &self.fields[field];
        let on = f.widgets[widget].on.clone()?;
        match f.kind {
            FieldKind::CheckBox if f.value.as_deref() == Some(on.as_str()) => {
                Some(FieldValue::State(None))
            }
            FieldKind::CheckBox | FieldKind::Radio => Some(FieldValue::State(Some(on))),
            _ => None,
        }
    }

    /// The pages the last change shows on: the field's, and those of
    /// the fields worked out from it.
    pub fn pages_of(&self, field: usize) -> Vec<usize> {
        let mut pages: Vec<usize> = std::iter::once(field)
            .chain(self.touched.iter().copied())
            .chain(self.calculated.iter().map(|(f, _)| *f))
            .filter(|&f| f < self.fields.len())
            .flat_map(|f| self.fields[f].widgets.iter().map(|w| w.page))
            .collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    /// The file was saved as `source`: it is the new starting point.
    pub fn saved(&mut self, source: Arc<Document>) {
        self.source = source;
        self.changes.clear();
        self.outcomes.clear();
        self.filled = None;
    }
}

/// For each page, the widgets that can be filled or clicked there.
fn widgets_on_pages(
    fields: &[Field],
    actions: &HashMap<usize, FieldAction>,
    js: Option<&katna_pdf_js::Form>,
) -> HashMap<usize, Vec<(usize, usize)>> {
    let mut on_page: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (f, field) in fields.iter().enumerate() {
        let clicks = js.is_some_and(|js| js.clicks(&field.name));
        if !fillable(field) && !actions.contains_key(&f) && !clicks {
            continue;
        }
        for (w, widget) in field.widgets.iter().enumerate() {
            if !widget.hidden {
                on_page.entry(widget.page).or_default().push((f, w));
            }
        }
    }
    on_page
}

/// Whether a field can be filled in here.
fn fillable(field: &Field) -> bool {
    !field.read_only
        && match field.kind {
            FieldKind::Text { .. } => true,
            FieldKind::CheckBox | FieldKind::Radio => field.widgets.iter().all(|w| w.on.is_some()),
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(id: u32, kind: FieldKind, value: Option<&str>, on: &[&str]) -> Field {
        Field {
            id: Ref::new(id, 0),
            name: format!("f{id}"),
            kind,
            value: value.map(Into::into),
            read_only: false,
            required: false,
            tooltip: None,
            widgets: on
                .iter()
                .map(|on| Widget {
                    id: Ref::new(id + 100, 0),
                    page: 0,
                    rect: [0.0; 4],
                    on: Some((*on).into()),
                    hidden: false,
                })
                .collect(),
        }
    }

    #[test]
    fn clicks_turn_boxes_over_and_buttons_on() {
        let source = Arc::new(Document::load(minimal()).unwrap());
        let fields = vec![
            field(1, FieldKind::CheckBox, Some("Yes"), &["Yes"]),
            field(2, FieldKind::Radio, None, &["A", "B"]),
        ];
        let mut form = Form::new(source, fields, true, "t.pdf").unwrap();
        assert_eq!(form.on_page(0).count(), 3);
        let off = form.clicked(0, 0).unwrap();
        assert_eq!(off, FieldValue::State(None));
        assert!(form.set(0, off).changed);
        assert_eq!(
            form.clicked(0, 0),
            Some(FieldValue::State(Some("Yes".into())))
        );
        let b = form.clicked(1, 1).unwrap();
        assert!(form.set(1, b.clone()).changed);
        // The same again changes nothing.
        assert!(!form.set(1, b).changed);
        assert_eq!(form.changes.len(), 2);
        assert!(form.dirty());
    }

    fn minimal() -> Vec<u8> {
        let body = "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Count 0 /Kids [] >>\nendobj\n";
        let xref = body.len();
        format!(
            "{body}xref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n\
             trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .into_bytes()
    }
}
