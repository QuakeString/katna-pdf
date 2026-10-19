// SPDX-License-Identifier: GPL-3.0-or-later

//! The document's properties (docs/PLAN.md §4.3, "metadata editor"):
//! a card, opened from the file's name in the toolbar or with Ctrl+D,
//! to change its title, author, subject and keywords, showing what made
//! it and when (`katna_pdf_cos::meta`), and to put a password on it or
//! take one off (`katna_pdf_cos::lock`, AES-256).

use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};

use katna_pdf_cos::{Metadata, Permissions};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::{Content, Viewer};
use crate::text::t;

/// The card's fields: title, author, subject, keywords.
pub(super) struct Properties {
    /// As the file says, once read.
    read: Option<Metadata>,
    /// Whether the file has a password, and whether it was opened with
    /// its owner's (so it may be taken off), once read.
    locked: Option<(bool, bool)>,
    fields: [Entity<TextInput>; 4],
    /// A new password to put on.
    password: Entity<TextInput>,
    _fields: Vec<Subscription>,
}

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
            fields,
            password,
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
                    super::cos(bytes, key.as_deref())
                        .ok()
                        .map(|d| (d.metadata(), (d.is_encrypted(), d.opened_as_owner())))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(properties) = this.properties.as_mut() else {
                    return;
                };
                let (meta, locked) = meta.unwrap_or_default();
                properties.locked = Some(locked);
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
        if password.is_empty() {
            return;
        }
        self.properties = None;
        let pages = (0..self.page_count()).collect();
        let after: Arc<str> = Arc::from(password.as_str());
        self.change_document_keyed(
            pages,
            "properties-lock-failed",
            Some(after),
            move |doc| {
                doc.lock(&password, &password, Permissions::ALL)
                    .map_err(|e| e.to_string())
            },
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
            card = match (locked, owner) {
                (false, _) => card.child(label("properties-password-none")).child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(
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
                                .child(properties.password.clone()),
                        )
                        .child(
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
                (true, false) => card.child(label("properties-password-owner")),
            };
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
