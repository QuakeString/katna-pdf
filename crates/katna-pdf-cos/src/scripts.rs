// SPDX-License-Identifier: GPL-3.0-or-later

//! Every action the document carries, listed so people can see what a
//! file would do in a reader that runs them (none is run here): the
//! document's own scripts (`/Names` `/JavaScript`), what happens when it
//! opens (`/OpenAction`), closes, is saved or printed (the catalog's
//! `/AA`), when a page opens or closes (its `/AA`), and what fields do
//! when clicked, typed in, formatted, checked or calculated (`/A` and
//! `/AA`). Actions chained with `/Next` are listed one after another.

use std::collections::HashSet;

use crate::actions::FieldAction;
use crate::document::Document;
use crate::object::{Dict, Object};
use crate::outline::text_string;

/// Where an action is attached, and on what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionPlace {
    /// A script of the document's own, by name: run when it opens.
    Document(String),
    /// When the document opens.
    Open,
    /// The catalog's `/AA`: `WC`, `WS`, `DS`, `WP`, `DP`.
    DocumentEvent(DocEvent),
    /// A page's `/AA`: opened or closed.
    Page { page: usize, opened: bool },
    /// A field's `/A` or `/AA`, by the field's name.
    Field { name: String, event: FieldEvent },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocEvent {
    WillClose,
    WillSave,
    DidSave,
    WillPrint,
    DidPrint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldEvent {
    Click,
    Keystroke,
    Format,
    Validate,
    Calculate,
    Focus,
    Blur,
    MouseEnter,
    MouseExit,
    MouseDown,
    MouseUp,
}

impl DocEvent {
    const ALL: [(&'static str, DocEvent); 5] = [
        ("WC", DocEvent::WillClose),
        ("WS", DocEvent::WillSave),
        ("DS", DocEvent::DidSave),
        ("WP", DocEvent::WillPrint),
        ("DP", DocEvent::DidPrint),
    ];

    /// A short name for the event, for scripts and JSON.
    pub fn name(self) -> &'static str {
        match self {
            DocEvent::WillClose => "will-close",
            DocEvent::WillSave => "will-save",
            DocEvent::DidSave => "did-save",
            DocEvent::WillPrint => "will-print",
            DocEvent::DidPrint => "did-print",
        }
    }
}

impl FieldEvent {
    const AA: [(&'static str, FieldEvent); 10] = [
        ("K", FieldEvent::Keystroke),
        ("F", FieldEvent::Format),
        ("V", FieldEvent::Validate),
        ("C", FieldEvent::Calculate),
        ("Fo", FieldEvent::Focus),
        ("Bl", FieldEvent::Blur),
        ("E", FieldEvent::MouseEnter),
        ("X", FieldEvent::MouseExit),
        ("D", FieldEvent::MouseDown),
        ("U", FieldEvent::MouseUp),
    ];

    pub fn name(self) -> &'static str {
        match self {
            FieldEvent::Click => "click",
            FieldEvent::Keystroke => "keystroke",
            FieldEvent::Format => "format",
            FieldEvent::Validate => "validate",
            FieldEvent::Calculate => "calculate",
            FieldEvent::Focus => "focus",
            FieldEvent::Blur => "blur",
            FieldEvent::MouseEnter => "mouse-enter",
            FieldEvent::MouseExit => "mouse-exit",
            FieldEvent::MouseDown => "mouse-down",
            FieldEvent::MouseUp => "mouse-up",
        }
    }
}

/// An action the document carries.
#[derive(Debug, Clone, PartialEq)]
pub struct DocAction {
    pub place: ActionPlace,
    pub action: FieldAction,
}

impl FieldAction {
    /// What kind of action it is, in a word.
    pub fn kind(&self) -> &'static str {
        match self {
            FieldAction::GoTo(_) => "go-to",
            FieldAction::Uri(_) => "open-address",
            FieldAction::Reset { .. } => "reset",
            FieldAction::Hide { .. } => "hide",
            FieldAction::Submit { .. } => "submit",
            FieldAction::Named(_) => "named",
            FieldAction::JavaScript(_) => "javascript",
        }
    }

    /// What it does, in a line: the script, address, page or fields.
    pub fn detail(&self) -> String {
        match self {
            FieldAction::GoTo(d) => format!("page {}", d.page + 1),
            FieldAction::Uri(u) => u.clone(),
            FieldAction::Reset { fields, others } => match (fields.is_empty(), others) {
                (true, _) => "every field".to_owned(),
                (false, false) => fields.join(", "),
                (false, true) => format!("every field but {}", fields.join(", ")),
            },
            FieldAction::Hide { fields, hide } => format!(
                "{} {}",
                if *hide { "hide" } else { "show" },
                fields.join(", ")
            ),
            FieldAction::Submit { url, .. } => url.clone(),
            FieldAction::Named(n) => n.clone(),
            FieldAction::JavaScript(js) => js.clone(),
        }
    }
}

/// How many actions one `/Next` chain is followed for.
const MAX_CHAIN: usize = 16;

impl Document {
    /// Every action the document carries, document first, then pages in
    /// order, then fields in the form's order.
    pub fn document_actions(&self) -> Vec<DocAction> {
        let mut out = Vec::new();
        if self.is_locked() {
            return out;
        }
        let Some(catalog) = self.catalog() else {
            return out;
        };
        // The document's own scripts, in the name tree.
        if let Object::Dict(names) = self.resolve_key(&catalog, "Names")
            && let Object::Dict(root) = self.resolve_key(&names, "JavaScript")
        {
            let mut seen = HashSet::new();
            let mut pairs = Vec::new();
            self.tree_pairs_raw(&root, 0, &mut seen, &mut pairs);
            for (name, action) in pairs {
                if let Object::Dict(action) = self.resolve(&action) {
                    self.chain(&action, &mut |a| {
                        out.push(DocAction {
                            place: ActionPlace::Document(text_string(&name)),
                            action: a,
                        });
                    });
                }
            }
        }
        match self.resolve_key(&catalog, "OpenAction") {
            Object::Dict(action) => self.chain(&action, &mut |a| {
                out.push(DocAction {
                    place: ActionPlace::Open,
                    action: a,
                });
            }),
            Object::Array(dest) => {
                let mut reader = crate::outline::OutlineReader::new(self, catalog.clone());
                if let Some(dest) = reader.destination(&Object::Array(dest), 0) {
                    out.push(DocAction {
                        place: ActionPlace::Open,
                        action: FieldAction::GoTo(dest),
                    });
                }
            }
            _ => {}
        }
        if let Object::Dict(aa) = self.resolve_key(&catalog, "AA") {
            for (key, event) in DocEvent::ALL {
                if let Object::Dict(action) = self.resolve_key(&aa, key) {
                    self.chain(&action, &mut |a| {
                        out.push(DocAction {
                            place: ActionPlace::DocumentEvent(event),
                            action: a,
                        });
                    });
                }
            }
        }
        for (page, page_ref) in self.pages().into_iter().enumerate() {
            let Some(Object::Dict(dict)) = self.get(page_ref) else {
                continue;
            };
            let Object::Dict(aa) = self.resolve_key(&dict, "AA") else {
                continue;
            };
            for (key, opened) in [("O", true), ("C", false)] {
                if let Object::Dict(action) = self.resolve_key(&aa, key) {
                    self.chain(&action, &mut |a| {
                        out.push(DocAction {
                            place: ActionPlace::Page { page, opened },
                            action: a,
                        });
                    });
                }
            }
        }
        for field in self.form() {
            let mut dicts = Vec::new();
            if let Some(Object::Dict(f)) = self.get(field.id) {
                dicts.push(f);
            }
            if let Some(w) = field.widgets.first()
                && w.id != field.id
                && let Some(Object::Dict(w)) = self.get(w.id)
            {
                dicts.push(w);
            }
            for dict in &dicts {
                if let Object::Dict(action) = self.resolve_key(dict, "A") {
                    self.chain(&action, &mut |a| {
                        out.push(DocAction {
                            place: ActionPlace::Field {
                                name: field.name.clone(),
                                event: FieldEvent::Click,
                            },
                            action: a,
                        });
                    });
                }
                if let Object::Dict(aa) = self.resolve_key(dict, "AA") {
                    for (key, event) in FieldEvent::AA {
                        if let Object::Dict(action) = self.resolve_key(&aa, key) {
                            self.chain(&action, &mut |a| {
                                out.push(DocAction {
                                    place: ActionPlace::Field {
                                        name: field.name.clone(),
                                        event,
                                    },
                                    action: a,
                                });
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// `action` and those chained after it with `/Next`, each given to
    /// `each` when it is a kind that is read.
    fn chain(&self, action: &Dict, each: &mut dyn FnMut(FieldAction)) {
        let mut next = Some(action.clone());
        let mut n = 0;
        while let Some(action) = next.take() {
            n += 1;
            if n > MAX_CHAIN {
                break;
            }
            if let Some(a) = self.read_action(&action) {
                each(a);
            }
            match self.resolve_key(&action, "Next") {
                Object::Dict(d) => next = Some(d),
                Object::Array(items) => {
                    for item in items {
                        if let Object::Dict(d) = self.resolve(&item) {
                            self.chain(&d, each);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
