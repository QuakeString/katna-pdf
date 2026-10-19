// SPDX-License-Identifier: GPL-3.0-or-later

//! Filling in a PDF's form (docs/PLAN.md, Phase 1, form filling and
//! save): text fields, check boxes and radio buttons. Each change is
//! written into the file as an incremental update right away and the
//! pages are drawn again from it, so what shows is what other readers
//! will show; Save writes that file over the one opened.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{Entity, Subscription};
use katna_pdf_cos::{Calculation, Document, Field, FieldKind, FieldValue, Format, Ref, Widget};
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
    /// Values changed since the file was opened or saved.
    pub changes: Vec<(Ref, FieldValue)>,
    /// The file with the changes made, once written.
    pub filled: Option<Arc<Vec<u8>>>,
    /// The text field being typed in.
    pub editing: Option<Editing>,
    /// Bumped for each change, so an older fill that ends later is
    /// dropped.
    pub generation: u64,
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
    pub fn new(source: Arc<Document>, fields: Vec<Field>) -> Option<Self> {
        let mut on_page: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
        for (f, field) in fields.iter().enumerate() {
            if !fillable(field) {
                continue;
            }
            for (w, widget) in field.widgets.iter().enumerate() {
                on_page.entry(widget.page).or_default().push((f, w));
            }
        }
        let scripts = source.form_scripts(&fields);
        let calculated = fields
            .iter()
            .enumerate()
            .filter_map(|(f, field)| Some((f, scripts.get(&field.id)?.calculate.clone()?)))
            .collect();
        let dates = fields
            .iter()
            .enumerate()
            .filter_map(
                |(f, field)| match scripts.get(&field.id)?.format.as_ref()? {
                    Format::Date(pattern) => Some((f, pattern.clone())),
                    _ => None,
                },
            )
            .collect();
        (!on_page.is_empty()).then_some(Self {
            source,
            fields,
            on_page,
            calculated,
            dates,
            changes: Vec::new(),
            filled: None,
            editing: None,
            generation: 0,
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

    /// How field `field` writes its date, if it holds one.
    pub fn date_pattern(&self, field: usize) -> Option<&str> {
        self.dates.get(&field).map(String::as_str)
    }

    /// Whether there are changes not saved.
    pub fn dirty(&self) -> bool {
        !self.changes.is_empty()
    }

    /// Sets field `field` to `value`. Returns whether that changed it.
    pub fn set(&mut self, field: usize, value: FieldValue) -> bool {
        let f = &mut self.fields[field];
        let new = match &value {
            FieldValue::Text(text) => Some(text.clone()).filter(|t| !t.is_empty()),
            FieldValue::State(state) => state.clone(),
        };
        if f.value == new {
            return false;
        }
        f.value = new;
        let id = f.id;
        self.changes.retain(|(r, _)| *r != id);
        self.changes.push((id, value));
        self.generation += 1;
        self.recalculate();
        true
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

    /// The pages the changes since the last fill show on.
    /// Totals change with the fields they add up, so their pages too.
    pub fn pages_of(&self, field: usize) -> Vec<usize> {
        let mut pages: Vec<usize> = std::iter::once(field)
            .chain(self.calculated.iter().map(|(f, _)| *f))
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
        self.filled = None;
    }
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
        let mut form = Form::new(source, fields).unwrap();
        assert_eq!(form.on_page(0).count(), 3);
        let off = form.clicked(0, 0).unwrap();
        assert_eq!(off, FieldValue::State(None));
        assert!(form.set(0, off));
        assert_eq!(
            form.clicked(0, 0),
            Some(FieldValue::State(Some("Yes".into())))
        );
        let b = form.clicked(1, 1).unwrap();
        assert!(form.set(1, b.clone()));
        // The same again changes nothing.
        assert!(!form.set(1, b));
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
