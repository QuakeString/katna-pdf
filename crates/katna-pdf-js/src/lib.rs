// SPDX-License-Identifier: GPL-3.0-or-later

//! A PDF form's own JavaScript, run the way Acrobat runs it
//! (docs/PLAN.md §4.3, forms): keystroke and validate scripts check what
//! is typed, calculate scripts work out totals in the form's order,
//! format scripts say how values show, and buttons' scripts run when
//! clicked. Scripts see the parts of Acrobat's object model forms use
//! (`acrobat.js`): the document's fields, the event, `app`, `util`,
//! `color`, `display` and the `AF*` helpers.
//!
//! The scripts run in [boa](https://boajs.dev), pure Rust, in a sandbox:
//! nothing reaches files, the network or other programs. What a script
//! asks of the world (open an address, print, send the data, go to a
//! page) comes back as a [`Request`] for the app to decide on, and each
//! run is cut off after a bounded number of loop turns.

use std::collections::{HashMap, HashSet};

use boa_engine::{
    Context, JsResult, JsValue, NativeFunction, Source, js_string, object::ObjectInitializer,
    property::Attribute,
};
use katna_pdf_cos::{
    ActionPlace, Display, Document, Field, FieldAction, FieldEvent, FieldKind, FieldValue,
    FillExtras, Ref,
};
use serde_json::{Value, json};

const PRELUDE: &str = include_str!("acrobat.js");

/// How many turns any one loop of a script may take.
const MAX_LOOP: u64 = 1_000_000;
/// How deep a script's calls may nest.
const MAX_DEPTH: usize = 256;
/// How many times calculations run over the form after one change, so
/// totals of totals settle without looping for ever.
const ROUNDS: usize = 3;

/// The scripts of one field, by event.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldScripts {
    pub keystroke: Option<String>,
    pub format: Option<String>,
    pub validate: Option<String>,
    pub calculate: Option<String>,
    /// What clicking does: the widget's or field's `/A`, then `/AA /U`.
    pub click: Vec<String>,
    pub focus: Option<String>,
    pub blur: Option<String>,
}

/// Something a script asked for that the app decides on.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Open a web or mail address.
    Url(String),
    /// Send the form's data to an address.
    Submit(String),
    Print,
    /// Show page `n` (from 0).
    Page(usize),
    /// Go to a named destination.
    Dest(String),
    /// Put the typing cursor in a field.
    Focus(String),
    /// A menu command, by Acrobat's name (`NextPage`, `Print`…).
    Menu(String),
    /// Send the file by mail.
    Mail,
}

/// What one change, click or reset did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// Whether the value typed was taken: a keystroke or validate script
    /// may turn it down, and then nothing changed.
    pub accepted: bool,
    /// Fields whose value changed, by full name.
    pub values: Vec<(String, String)>,
    /// Fields whose display changed.
    pub display: Vec<(String, Display)>,
    /// Fields locked or unlocked.
    pub read_only: Vec<(String, bool)>,
    /// What `app.alert` said, in order.
    pub alerts: Vec<String>,
    pub requests: Vec<Request>,
    /// Scripts that failed, with why (for the console, not the person).
    pub errors: Vec<String>,
}

/// The kind of a field as scripts name it.
fn type_name(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text { .. } => "text",
        FieldKind::CheckBox => "checkbox",
        FieldKind::Radio => "radiobutton",
        FieldKind::PushButton => "button",
        FieldKind::Choice { combo: true, .. } => "combobox",
        FieldKind::Choice { combo: false, .. } => "listbox",
        FieldKind::Signature => "signature",
    }
}

/// A form with its scripts ready to run.
pub struct Form {
    ctx: Context,
    names: HashMap<Ref, String>,
    ids: HashMap<String, Ref>,
    scripts: HashMap<String, FieldScripts>,
    /// Fields with a calculate script, in the order they are worked out.
    order: Vec<String>,
    /// Scripts of the document's own that failed when it opened.
    pub errors: Vec<String>,
}

impl std::fmt::Debug for Form {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Form")
            .field("fields", &self.names.len())
            .field("scripts", &self.scripts.len())
            .finish()
    }
}

/// The scripts of `doc`'s fields, by field name, and its document-level
/// scripts, in order.
pub fn scripts_of(doc: &Document) -> (HashMap<String, FieldScripts>, Vec<String>) {
    let mut fields: HashMap<String, FieldScripts> = HashMap::new();
    let mut document = Vec::new();
    for action in doc.document_actions() {
        let FieldAction::JavaScript(js) = action.action else {
            continue;
        };
        match action.place {
            ActionPlace::Document(_) => document.push(js),
            ActionPlace::Field { name, event } => {
                let s = fields.entry(name).or_default();
                match event {
                    FieldEvent::Keystroke => s.keystroke = Some(js),
                    FieldEvent::Format => s.format = Some(js),
                    FieldEvent::Validate => s.validate = Some(js),
                    FieldEvent::Calculate => s.calculate = Some(js),
                    FieldEvent::Click | FieldEvent::MouseUp => s.click.push(js),
                    FieldEvent::Focus => s.focus = Some(js),
                    FieldEvent::Blur => s.blur = Some(js),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (fields, document)
}

impl Form {
    /// The form of `doc`, if it carries any script a field or the
    /// document runs. `file` is the file's name, as scripts see it.
    pub fn new(doc: &Document, fields: &[Field], file: &str) -> Option<Self> {
        let (scripts, document) = scripts_of(doc);
        if scripts.is_empty() {
            return None;
        }
        let mut ctx = Context::default();
        ctx.runtime_limits_mut().set_loop_iteration_limit(MAX_LOOP);
        ctx.runtime_limits_mut().set_recursion_limit(MAX_DEPTH);
        if let Err(e) = ctx.eval(Source::from_bytes(PRELUDE)) {
            // Our own prelude failing is a bug; run nothing rather than half.
            debug_assert!(false, "acrobat.js: {e}");
            return None;
        }
        install_host(&mut ctx).ok()?;
        let defaults: HashMap<Ref, String> = doc
            .reset_values()
            .into_iter()
            .map(|(id, v)| (id, value_text(&v)))
            .collect();
        let mut states = serde_json::Map::new();
        for f in fields {
            let value = match (&f.kind, &f.value) {
                (FieldKind::CheckBox | FieldKind::Radio, None) => "Off".to_owned(),
                (_, v) => v.clone().unwrap_or_default(),
            };
            let items: Vec<Value> = match &f.kind {
                FieldKind::Choice { options, .. } => {
                    options.iter().map(|o| json!([o, o])).collect()
                }
                _ => Vec::new(),
            };
            let exports: Vec<String> = f.widgets.iter().filter_map(|w| w.on.clone()).collect();
            let (multiline, password, limit, comb) = match f.kind {
                FieldKind::Text {
                    multiline,
                    password,
                    max_len,
                    comb,
                } => (multiline, password, max_len.unwrap_or(0), comb),
                _ => (false, false, 0, false),
            };
            let pages: Vec<usize> = f.widgets.iter().map(|w| w.page).collect();
            let rect = f.widgets.first().map(|w| w.rect).unwrap_or_default();
            let default = defaults.get(&f.id).cloned().unwrap_or_else(|| {
                if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) {
                    "Off".into()
                } else {
                    String::new()
                }
            });
            states.insert(
                f.name.clone(),
                json!({
                    "type": type_name(&f.kind),
                    "value": value,
                    "defaultValue": default,
                    "display": if f.widgets.iter().all(|w| w.hidden) && !f.widgets.is_empty() { 1 } else { 0 },
                    "readonly": f.read_only,
                    "required": f.required,
                    "items": items,
                    "exports": exports,
                    "multiline": multiline,
                    "password": password,
                    "charLimit": limit,
                    "comb": comb,
                    "pages": pages,
                    "rect": rect,
                }),
            );
        }
        let meta = doc.metadata();
        let setup = format!(
            "__fields = {}; __numPages = {}; __fileName = {}; __info = {};",
            Value::Object(states),
            doc.pages().len().max(1),
            json!(file),
            json!({
                "Title": meta.title, "Author": meta.author, "Subject": meta.subject,
                "Keywords": meta.keywords, "Creator": meta.creator, "Producer": meta.producer,
            }),
        );
        ctx.eval(Source::from_bytes(&setup)).ok()?;
        let names: HashMap<Ref, String> = fields.iter().map(|f| (f.id, f.name.clone())).collect();
        let ids = fields.iter().map(|f| (f.name.clone(), f.id)).collect();
        // The form's /CO first, then any others with a calculation.
        let mut order: Vec<String> = doc
            .calculation_list()
            .into_iter()
            .filter_map(|r| names.get(&r).cloned())
            .filter(|n| scripts.get(n).is_some_and(|s| s.calculate.is_some()))
            .collect();
        let mut rest: Vec<String> = scripts
            .iter()
            .filter(|(n, s)| s.calculate.is_some() && !order.contains(n))
            .map(|(n, _)| n.clone())
            .collect();
        rest.sort();
        order.extend(rest);
        let mut form = Self {
            ctx,
            names,
            ids,
            scripts,
            order,
            errors: Vec::new(),
        };
        for js in document {
            if let Err(e) = form.ctx.eval(Source::from_bytes(&js)) {
                form.errors.push(e.to_string());
            }
        }
        Some(form)
    }

    /// The full name of field `id`.
    pub fn name_of(&self, id: Ref) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Whether field `name` has a script for clicks.
    pub fn clicks(&self, name: &str) -> bool {
        self.scripts.get(name).is_some_and(|s| !s.click.is_empty())
    }

    /// Whether field `name` checks what is typed in it.
    pub fn checks(&self, name: &str) -> bool {
        self.scripts
            .get(name)
            .is_some_and(|s| s.keystroke.is_some() || s.validate.is_some())
    }

    /// Field `name` is given `value` (typed and finished, or a box
    /// ticked): its keystroke and validate scripts may turn it down;
    /// if taken, calculations run over the form.
    pub fn commit(&mut self, name: &str, value: &str) -> Outcome {
        let mut out = Outcome::default();
        let scripts = self.scripts.get(name).cloned().unwrap_or_default();
        let mut value = value.to_owned();
        for js in [&scripts.keystroke, &scripts.validate]
            .into_iter()
            .flatten()
        {
            let ev = json!({
                "name": if Some(js) == scripts.keystroke.as_ref() { "Keystroke" } else { "Validate" },
                "type": "Field", "targetName": name, "value": value, "change": "",
                "changeEx": "", "willCommit": true, "selStart": 0, "selEnd": 0,
                "keyDown": false, "modifier": false, "shift": false, "commitKey": 2,
            });
            let Some(run) = self.run(&ev, js, &mut out) else {
                continue;
            };
            if !run.rc {
                // Turned down: what the script changed besides stays, as
                // in Acrobat; the value does not.
                out.accepted = false;
                return out;
            }
            value = run.value;
        }
        out.accepted = true;
        self.set(name, &value, &mut out);
        self.calculate(&mut out);
        out
    }

    /// Field `name`'s value as its own scripts would leave it, without
    /// asking them: used when a value comes from elsewhere (a reset, data
    /// brought in).
    pub fn put(&mut self, name: &str, value: &str) -> Outcome {
        let mut out = Outcome {
            accepted: true,
            ..Outcome::default()
        };
        self.set(name, value, &mut out);
        self.calculate(&mut out);
        out
    }

    /// Button or box `name` was clicked: its click scripts run.
    pub fn click(&mut self, name: &str) -> Outcome {
        let mut out = Outcome {
            accepted: true,
            ..Outcome::default()
        };
        let scripts = self
            .scripts
            .get(name)
            .map(|s| s.click.clone())
            .unwrap_or_default();
        for js in &scripts {
            let ev = json!({ "name": "Mouse Up", "type": "Field", "targetName": name, "value": self.value(name) });
            self.run(&ev, js, &mut out);
        }
        self.calculate(&mut out);
        out
    }

    /// Works every calculation out again, as Acrobat does when a form
    /// opens or `calculateNow()` is called.
    pub fn recalculate(&mut self) -> Outcome {
        let mut out = Outcome {
            accepted: true,
            ..Outcome::default()
        };
        self.calculate(&mut out);
        out
    }

    /// The text each field with a format script shows for its value now.
    pub fn shown(&mut self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        let mut scratch = Outcome::default();
        let formats: Vec<(String, String)> = self
            .scripts
            .iter()
            .filter_map(|(n, s)| Some((n.clone(), s.format.clone()?)))
            .collect();
        for (name, js) in formats {
            let value = self.value(&name);
            let ev = json!({ "name": "Format", "type": "Field", "targetName": name, "value": value, "willCommit": true });
            if let Some(run) = self.run(&ev, &js, &mut scratch) {
                out.insert(name, run.value);
            }
        }
        out
    }

    /// The fill extras for `doc`'s [`Document::fill_with`]: what format
    /// scripts show for each field, and the display and locks `outcomes`
    /// gave.
    pub fn extras(&mut self, outcomes: &[Outcome]) -> FillExtras {
        let mut extras = FillExtras {
            calculated: true,
            ..FillExtras::default()
        };
        for (name, text) in self.shown() {
            if let Some(&id) = self.ids.get(&name) {
                extras.shown.insert(id, text);
            }
        }
        let mut seen = HashSet::new();
        for o in outcomes.iter().rev() {
            for (name, d) in &o.display {
                if let Some(&id) = self.ids.get(name)
                    && seen.insert((id, "d"))
                {
                    extras.display.push((id, *d));
                }
            }
            for (name, r) in &o.read_only {
                if let Some(&id) = self.ids.get(name)
                    && seen.insert((id, "r"))
                {
                    extras.read_only.push((id, *r));
                }
            }
        }
        extras
    }

    /// The value scripts see in field `name` now.
    pub fn value(&mut self, name: &str) -> String {
        let js = format!(
            "(function(){{ var s = __fields[{}]; return s ? String(s.value) : ''; }})()",
            json!(name)
        );
        self.ctx
            .eval(Source::from_bytes(&js))
            .ok()
            .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
            .unwrap_or_default()
    }

    /// Each field's id with the value it should get, for `changes` in
    /// [`Document::fill_with`].
    pub fn changes(&self, outcome: &Outcome, fields: &[Field]) -> Vec<(Ref, FieldValue)> {
        outcome
            .values
            .iter()
            .filter_map(|(name, v)| {
                let f = fields.iter().find(|f| &f.name == name)?;
                Some((f.id, field_value(&f.kind, v)))
            })
            .collect()
    }

    fn set(&mut self, name: &str, value: &str, out: &mut Outcome) {
        let js = format!(
            "(function(){{ var s = __fields[{n}]; if (s && s.value !== {v}) {{ s.value = {v}; return true; }} return false; }})()",
            n = json!(name),
            v = json!(value),
        );
        if self
            .ctx
            .eval(Source::from_bytes(&js))
            .ok()
            .and_then(|v| v.as_boolean())
            == Some(true)
        {
            push_value(out, name, value);
        }
    }

    fn calculate(&mut self, out: &mut Outcome) {
        for _ in 0..ROUNDS {
            let mut changed = false;
            for name in self.order.clone() {
                let Some(js) = self.scripts.get(&name).and_then(|s| s.calculate.clone()) else {
                    continue;
                };
                let before = self.value(&name);
                let ev = json!({ "name": "Calculate", "type": "Field", "targetName": name, "value": before });
                let Some(run) = self.run(&ev, &js, out) else {
                    continue;
                };
                if !run.rc || run.value == before {
                    continue;
                }
                // A worked-out value is checked as a typed one is; forms
                // show and hide fields from their validate scripts.
                let validate = self.scripts.get(&name).and_then(|s| s.validate.clone());
                if let Some(v) = validate {
                    let ev = json!({ "name": "Validate", "type": "Field", "targetName": name, "value": run.value, "willCommit": true });
                    if self.run(&ev, &v, out).is_some_and(|r| !r.rc) {
                        continue;
                    }
                }
                self.set(&name, &run.value, out);
                changed = true;
            }
            if !changed {
                break;
            }
        }
    }

    /// Runs `js` for event `ev`, folding what it changed into `out`.
    fn run(&mut self, ev: &Value, js: &str, out: &mut Outcome) -> Option<Run> {
        let begin = format!("__begin({ev});");
        self.ctx.eval(Source::from_bytes(&begin)).ok()?;
        if let Err(e) = self.ctx.eval(Source::from_bytes(js)) {
            out.errors.push(e.to_string());
            // What it changed before failing still stands, as in Acrobat.
        }
        let end = self.ctx.eval(Source::from_bytes("__end()")).ok()?;
        let text = end.as_string()?.to_std_string_escaped();
        let v: Value = serde_json::from_str(&text).ok()?;
        for c in v["changed"].as_array().into_iter().flatten() {
            let name = c["name"].as_str().unwrap_or_default().to_owned();
            if let Some(value) = c["value"].as_str() {
                push_value(out, &name, value);
            }
            if let Some(d) = c["display"].as_i64() {
                let d = match d {
                    1 => Display::Hidden,
                    2 => Display::NoPrint,
                    3 => Display::NoView,
                    _ => Display::Visible,
                };
                out.display.retain(|(n, _)| *n != name);
                out.display.push((name.clone(), d));
            }
            if let Some(r) = c["readonly"].as_bool() {
                out.read_only.retain(|(n, _)| *n != name);
                out.read_only.push((name, r));
            }
        }
        for a in v["alerts"].as_array().into_iter().flatten() {
            out.alerts.push(a.as_str().unwrap_or_default().to_owned());
        }
        for r in v["requests"].as_array().into_iter().flatten() {
            let s = |k: &str| r[k].as_str().unwrap_or_default().to_owned();
            let request = match r["kind"].as_str() {
                Some("url") => Request::Url(s("url")),
                Some("submit") => Request::Submit(s("url")),
                Some("print") => Request::Print,
                Some("page") => Request::Page(r["page"].as_u64().unwrap_or(0) as usize),
                Some("dest") => Request::Dest(s("name")),
                Some("focus") => Request::Focus(s("name")),
                Some("menu") => Request::Menu(s("name")),
                Some("mail") => Request::Mail,
                _ => continue,
            };
            if !out.requests.contains(&request) {
                out.requests.push(request);
            }
        }
        Some(Run {
            rc: v["rc"].as_bool().unwrap_or(true),
            value: v["value"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

struct Run {
    rc: bool,
    value: String,
}

fn push_value(out: &mut Outcome, name: &str, value: &str) {
    out.values.retain(|(n, _)| n != name);
    out.values.push((name.to_owned(), value.to_owned()));
}

/// A cos value as scripts hold it.
fn value_text(v: &FieldValue) -> String {
    match v {
        FieldValue::Text(t) => t.clone(),
        FieldValue::State(s) => s.clone().unwrap_or_else(|| "Off".into()),
    }
}

/// A script's value as cos holds it for a field of `kind`.
pub fn field_value(kind: &FieldKind, v: &str) -> FieldValue {
    match kind {
        FieldKind::CheckBox | FieldKind::Radio => {
            FieldValue::State(Some(v.to_owned()).filter(|v| !v.is_empty() && v != "Off"))
        }
        _ => FieldValue::Text(v.to_owned()),
    }
}

/// The natives `acrobat.js` calls as `__host.*`: Rust's own number and
/// date formats, so scripts show values exactly as the rest of Katna PDF.
fn install_host(ctx: &mut Context) -> JsResult<()> {
    fn arg(args: &[JsValue], i: usize, ctx: &mut Context) -> JsResult<String> {
        Ok(args
            .get(i)
            .cloned()
            .unwrap_or_default()
            .to_string(ctx)?
            .to_std_string_escaped())
    }
    let show = NativeFunction::from_fn_ptr(|_, args, ctx| {
        let call = arg(args, 0, ctx)?;
        let value = arg(args, 1, ctx)?;
        let shown = match katna_pdf_cos::format(&call) {
            Some(f) => katna_pdf_cos::show(&f, &value),
            None => value,
        };
        Ok(JsValue::from(js_string!(shown)))
    });
    let parse = NativeFunction::from_fn_ptr(|_, args, ctx| {
        let value = arg(args, 0, ctx)?;
        let pattern = arg(args, 1, ctx)?;
        Ok(match katna_pdf_cos::parse_date(&value, &pattern) {
            Some((y, mo, d, h, mi)) => {
                let list = format!("[{y},{mo},{d},{h},{mi}]");
                ctx.eval(Source::from_bytes(&list))?
            }
            None => JsValue::null(),
        })
    });
    let pattern = NativeFunction::from_fn_ptr(|_, args, ctx| {
        let n = arg(args, 0, ctx)?;
        let p = match katna_pdf_cos::format(&format!("AFDate_Format({n})")) {
            Some(katna_pdf_cos::Format::Date(p)) => p,
            _ => "mm/dd/yyyy".to_owned(),
        };
        Ok(JsValue::from(js_string!(p)))
    });
    let host = ObjectInitializer::new(ctx)
        .function(show, js_string!("show"), 2)
        .function(parse, js_string!("parse"), 2)
        .function(pattern, js_string!("pattern"), 1)
        .build();
    ctx.register_global_property(js_string!("__host"), host, Attribute::all())
}

#[cfg(test)]
mod tests;
