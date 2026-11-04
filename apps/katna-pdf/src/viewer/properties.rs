// SPDX-License-Identifier: GPL-3.0-or-later

//! The document's properties (docs/PLAN.md §4.3, "metadata editor"):
//! a card, opened from the file's name in the toolbar or with Ctrl+D,
//! to change its title, author, subject and keywords, showing what made
//! it and when (`katna_pdf_cos::meta`), to put a password on it or
//! take one off (`katna_pdf_cos::lock`, AES-256), and to make it smaller
//! (`katna_pdf_cos::optimize`).

use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};

use katna_pdf_cos::{Metadata, Optimize, Permissions};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::{Content, Viewer};
use crate::text::t;

/// The card's fields: title, author, subject, keywords.
pub(super) struct Properties {
    /// As the file says, once read.
    read: Option<Metadata>,
    /// Whether the file has a password, and whether it was opened with
    /// its owner's (so it may be taken off), once read.
    locked: Option<(bool, bool)>,
    /// How large the file is, in bytes, once read.
    size: Option<u64>,
    fields: [Entity<TextInput>; 4],
    /// A new password to put on: the one that opens the file, and the
    /// owner's, which opens it fully.
    password: Entity<TextInput>,
    owner: Entity<TextInput>,
    /// What someone opening it without the owner's password may do.
    allow: Permissions,
    _fields: Vec<Subscription>,
}

/// Reaches one of the allowed actions.
type Allowed = fn(&mut Permissions) -> &mut bool;

/// The allowed actions the card offers, in order.
const ALLOWED: [(&str, Allowed); 6] = [
    ("properties-allow-print", |p| &mut p.print),
    ("properties-allow-change", |p| &mut p.change),
    ("properties-allow-copy", |p| &mut p.copy),
    ("properties-allow-comment", |p| &mut p.comment),
    ("properties-allow-fill", |p| &mut p.fill_forms),
    ("properties-allow-assemble", |p| &mut p.assemble),
];

const LABELS: [&str; 4] = [
    "properties-title",
    "properties-author",
    "properties-subject",
    "properties-keywords",
];

impl Viewer {
    /// Opens the properties card, or closes it.
    pub(super) fn toggle_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
            return;
        }
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        self.marking = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let fields = LABELS.map(|_| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_accent(accent);
                input
            })
        });
        let password = cx.new(|cx| {
            let mut input = TextInput::new(t("properties-password-new", &[]), cx);
            input.set_accent(accent);
            input.set_masked(true, cx);
            input
        });
        let owner = cx.new(|cx| {
            let mut input = TextInput::new(t("properties-password-owner-new", &[]), cx);
            input.set_accent(accent);
            input.set_masked(true, cx);
            input
        });
        let subs = fields
            .iter()
            .map(|f| {
                cx.subscribe(f, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.keep_properties(cx);
                    }
                })
            })
            .collect();
        self.properties = Some(Properties {
            read: None,
            locked: None,
            size: None,
            fields,
            password,
            owner,
            allow: Permissions::ALL,
            _fields: subs,
        });
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let meta = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let size = bytes.len() as u64;
                    super::cos(bytes, key.as_deref())
                        .ok()
                        .map(|d| (d.metadata(), (d.is_encrypted(), d.opened_as_owner()), size))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(properties) = this.properties.as_mut() else {
                    return;
                };
                let (meta, locked, size) = meta.unwrap_or_default();
                properties.locked = Some(locked);
                properties.size = Some(size);
                let values = [&meta.title, &meta.author, &meta.subject, &meta.keywords];
                for (field, value) in properties.fields.iter().zip(values) {
                    let value = value.clone().unwrap_or_default();
                    field.update(cx, |f, cx| f.set_text(value, cx));
                }
                properties.read = Some(meta);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Writes the card's fields into the file, when they changed.
    fn keep_properties(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.take() else {
            return;
        };
        cx.notify();
        let Some(read) = properties.read else {
            return;
        };
        let [title, author, subject, keywords] = properties
            .fields
            .clone()
            .map(|f| Some(f.read(cx).text().trim().to_owned()).filter(|v| !v.is_empty()));
        let meta = Metadata {
            title,
            author,
            subject,
            keywords,
            ..read.clone()
        };
        if meta == read {
            return;
        }
        self.change_document(
            Vec::new(),
            "properties-failed",
            move |doc| doc.set_metadata(&meta).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Puts the password typed on the file, its owner's too, so the same
    /// password opens it fully.
    fn lock_file(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.as_ref() else {
            return;
        };
        let password = properties.password.read(cx).text().to_owned();
        let owner = properties.owner.read(cx).text().to_owned();
        let allow = properties.allow;
        // One password typed serves both ways; the owner's alone leaves
        // the file open to anyone, with only what is allowed.
        let (user, owner) = match (password.is_empty(), owner.is_empty()) {
            (true, true) => return,
            (false, true) => (password.clone(), password),
            (true, false) => (String::new(), owner),
            (false, false) => (password, owner),
        };
        self.properties = None;
        let pages = (0..self.page_count()).collect();
        let after: Arc<str> = Arc::from(owner.as_str());
        self.change_document_keyed(
            pages,
            "properties-lock-failed",
            Some(after),
            move |doc| doc.lock(&user, &owner, allow).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Takes the file's password off.
    fn unlock_file(&mut self, cx: &mut Context<Self>) {
        self.properties = None;
        let pages = (0..self.page_count()).collect();
        self.change_document_keyed(
            pages,
            "properties-lock-failed",
            None,
            |doc| doc.without_lock().map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Takes the lock off a file opened without its owner's password,
    /// with the owner's password typed in the card.
    fn unlock_file_with(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.as_ref() else {
            return;
        };
        let owner = properties.owner.read(cx).text().to_owned();
        if owner.is_empty() {
            return;
        }
        // The change opens the file with the owner's password; the
        // result has none.
        let key = self.key.replace(Arc::from(owner.as_str()));
        self.unlock_file(cx);
        self.key = key;
    }

    /// Writes the file again as small as `how` allows, and says how much
    /// smaller it came out; it is not saved until it is.
    fn shrink(&mut self, how: Optimize, cx: &mut Context<Self>) {
        self.properties = None;
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        let current = self.unsaved();
        let key = self.key.clone();
        self.notice = Some(t("properties-shrinking", &[]));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let before = bytes.len() as u64;
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let done = doc.optimize(&how).map_err(|e| e.to_string())?;
                    Ok::<_, String>((before, done.bytes))
                })
                .await;
            this.update(cx, |this, cx| {
                match done {
                    Ok((before, bytes)) if (bytes.len() as u64) < before => {
                        let after = bytes.len() as u64;
                        let pages = (0..this.page_count()).collect();
                        this.change_document(
                            pages,
                            "properties-shrink-failed",
                            move |_| Ok(bytes),
                            cx,
                        );
                        this.notice = Some(t(
                            "properties-shrunk",
                            &[
                                ("before", &crate::library::size_text(before)),
                                ("after", &crate::library::size_text(after)),
                            ],
                        ));
                    }
                    Ok(_) => this.notice = Some(t("properties-shrink-none", &[])),
                    Err(error) => {
                        this.notice = Some(t("properties-shrink-failed", &[("error", &error)]))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The properties card, while it is open.
    pub(super) fn properties_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let properties = self.properties.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let mut card = div()
            .id("properties-card")
            .absolute()
            .top(px(space::S3))
            .right(px(space::S5))
            .w(px(360.0))
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
                    .child(t("properties-heading", &[])),
            );
        for (field, id) in properties.fields.iter().zip(LABELS) {
            card = card.child(label(id)).child(
                div()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(field.clone()),
            );
        }
        if let Some(read) = &properties.read {
            let facts = [
                ("properties-creator", read.creator.clone()),
                ("properties-producer", read.producer.clone()),
                (
                    "properties-created",
                    read.created.as_deref().and_then(shown_date),
                ),
                (
                    "properties-modified",
                    read.modified.as_deref().and_then(shown_date),
                ),
                ("properties-pages", Some(self.page_count().to_string())),
            ];
            for (id, value) in facts {
                let Some(value) = value else {
                    continue;
                };
                card = card.child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .text_size(px(text::SMALL))
                        .child(
                            div()
                                .w(px(96.0))
                                .flex_none()
                                .text_color(th.dim)
                                .child(t(id, &[])),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_color(th.text)
                                .child(value),
                        ),
                );
            }
        }
        if let Some((locked, owner)) = properties.locked {
            card = card.child(
                div()
                    .pt(px(space::S2))
                    .text_size(px(text::BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(t("properties-password", &[])),
            );
            let field = |input: &Entity<TextInput>| {
                div()
                    .flex_1()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(input.clone())
            };
            let mut allowed = div()
                .flex()
                .flex_wrap()
                .gap_x(px(space::S3))
                .gap_y(px(space::S1));
            for (i, (id, get)) in ALLOWED.into_iter().enumerate() {
                let mut allow = properties.allow;
                let on = *get(&mut allow);
                allowed = allowed.child(
                    div()
                        .id(("properties-allow", i))
                        .flex()
                        .items_center()
                        .gap(px(space::S1))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(p) = this.properties.as_mut() {
                                let flag = get(&mut p.allow);
                                *flag = !*flag;
                            }
                            cx.notify();
                        }))
                        .child(super::sign::check_box(on, th))
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(th.text)
                                .child(t(id, &[])),
                        ),
                );
            }
            card = match (locked, owner) {
                (false, _) => card
                    .child(label("properties-password-none"))
                    .child(
                        div()
                            .flex()
                            .gap(px(space::S2))
                            .child(field(&properties.password))
                            .child(field(&properties.owner)),
                    )
                    .child(label("properties-allow"))
                    .child(allowed)
                    .child(
                        div().flex().justify_end().child(
                            widgets::outlined_button(
                                "properties-lock",
                                t("properties-lock", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.lock_file(cx))),
                        ),
                    ),
                (true, true) => card.child(label("properties-password-on")).child(
                    div().flex().child(
                        widgets::outlined_button(
                            "properties-unlock",
                            t("properties-unlock", &[]),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.unlock_file(cx))),
                    ),
                ),
                (true, false) => card.child(label("properties-password-owner")).child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(field(&properties.owner))
                        .child(
                            widgets::outlined_button(
                                "properties-unlock",
                                t("properties-unlock", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.unlock_file_with(cx))),
                        ),
                ),
            };
        }
        if let Some(size) = properties.size {
            card = card
                .child(
                    div()
                        .pt(px(space::S2))
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.text)
                        .child(t("properties-size", &[])),
                )
                .child(div().text_size(px(text::SMALL)).text_color(th.dim).child(t(
                    "properties-size-now",
                    &[("size", &crate::library::size_text(size))],
                )))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::S2))
                        .child(
                            widgets::outlined_button(
                                "properties-shrink",
                                t("properties-shrink", &[]),
                                &th.katna,
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.shrink(Optimize::LOSSLESS, cx)),
                            ),
                        )
                        .child(
                            widgets::outlined_button(
                                "properties-shrink-pictures",
                                t("properties-shrink-pictures", &[]),
                                &th.katna,
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.shrink(Optimize::MEDIUM, cx)),
                            ),
                        ),
                );
        }
        // The form, and the comments: tinted, reset, and drawn in for good.
        let fields = self.form.as_ref().map_or(0, |f| f.fields.len());
        if fields > 0 {
            let count = fields.to_string();
            card = card
                .child(
                    div()
                        .pt(px(space::S2))
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.text)
                        .child(t("properties-form", &[])),
                )
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(t("properties-form-fields", &[("count", &count)])),
                )
                .child(
                    div()
                        .id("form-highlight")
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.highlight_fields = !this.highlight_fields;
                            cx.notify();
                        }))
                        .child(super::sign::check_box(self.highlight_fields, th))
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(t("form-highlight", &[])),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::S2))
                        .child(
                            widgets::outlined_button("form-reset", t("form-reset", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| this.reset_form(cx))),
                        )
                        .child(
                            widgets::outlined_button(
                                "form-flatten",
                                t("form-flatten", &[]),
                                &th.katna,
                            )
                            .tip(t("form-flatten-tip", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.flatten(
                                    katna_pdf_cos::Flatten {
                                        forms: true,
                                        comments: false,
                                    },
                                    cx,
                                )
                            })),
                        ),
                );
        }
        let comments = self
            .annots
            .as_ref()
            .map_or(0, |a| a.iter().map(Vec::len).sum::<usize>());
        if comments > 0 {
            let count = comments.to_string();
            card = card
                .child(
                    div()
                        .pt(px(space::S2))
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.text)
                        .child(t("properties-comments", &[])),
                )
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(t("properties-comments-count", &[("count", &count)])),
                )
                .child(
                    div().flex().child(
                        widgets::outlined_button(
                            "comments-flatten",
                            t("comments-flatten", &[]),
                            &th.katna,
                        )
                        .tip(t("comments-flatten-tip", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.flatten(
                                katna_pdf_cos::Flatten {
                                    forms: false,
                                    comments: true,
                                },
                                cx,
                            )
                        })),
                    ),
                );
        }
        card = card.child(
            div()
                .pt(px(space::S2))
                .flex()
                .justify_end()
                .gap(px(space::S2))
                .child(
                    widgets::outlined_button(
                        "properties-cancel",
                        t("properties-cancel", &[]),
                        &th.katna,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.properties = None;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::filled_button("properties-keep", t("properties-keep", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.keep_properties(cx))),
                ),
        );
        Some(card.into_any_element())
    }
}

/// A PDF date (`D:20261019103005+05'30'`) as people read it:
/// `2026-10-19 10:30`, with its zone when it has one.
fn shown_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let part = |from: usize| {
        d.get(from..from + 2)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
    };
    let year = d
        .get(0..4)
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))?;
    let mut out = format!(
        "{year}-{}-{}",
        part(4).unwrap_or("01"),
        part(6).unwrap_or("01")
    );
    if let (Some(h), Some(m)) = (part(8), part(10)) {
        out.push_str(&format!(" {h}:{m}"));
        match d.get(14..15) {
            Some("Z") => out.push_str(" UTC"),
            Some(sign @ ("+" | "-")) => {
                if let (Some(zh), Some(zm)) = (d.get(15..17), d.get(18..20)) {
                    out.push_str(&format!(" {sign}{zh}:{zm}"));
                }
            }
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::shown_date;

    #[test]
    fn dates_read_as_people_write_them() {
        assert_eq!(
            shown_date("D:20261019103005+05'30'").as_deref(),
            Some("2026-10-19 10:30 +05:30")
        );
        assert_eq!(
            shown_date("D:20261019103005Z").as_deref(),
            Some("2026-10-19 10:30 UTC")
        );
        assert_eq!(shown_date("D:20261019").as_deref(), Some("2026-10-19"));
        assert_eq!(shown_date("soon"), None);
    }
}
