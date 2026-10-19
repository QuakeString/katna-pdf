// SPDX-License-Identifier: GPL-3.0-or-later

//! The library's mail attachments: the PDFs attached to mail in Katna
//! Mail, newest first, found by their name, subject or sender, and taken
//! out of their mail to open.

use std::ops::Range;

use gpui::{AnyElement, Context, FontWeight, div, prelude::*, svg};
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::{CELL_HEIGHT, CELL_WIDTH, COVER_BOX, LibraryEvent, LibraryView, size_text};
use crate::mail::{self, MailPdf};
use crate::text::t;

/// The mail attachments shown, and which is chosen or being opened.
#[derive(Default)]
pub(super) struct MailView {
    /// Those the search box's words find.
    shown: Vec<MailPdf>,
    chosen: Option<usize>,
    opening: Option<usize>,
}

impl LibraryView {
    /// Reads Katna Mail's attachments again, in the background.
    pub(super) fn load_mail(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async { mail::pdfs() }).await;
            this.update(cx, |this, cx| {
                // Without Katna Mail, or when its store can't be read,
                // there is no place for them.
                this.mail_pdfs = found.unwrap_or_default();
                if this.mail.is_some() {
                    this.filter_mail(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows those the search box's words find.
    pub(super) fn filter_mail(&mut self, cx: &mut Context<Self>) {
        let words: Vec<String> = self
            .search
            .read(cx)
            .text()
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let shown: Vec<MailPdf> = self
            .mail_pdfs
            .iter()
            .filter(|pdf| pdf.matches(&words))
            .cloned()
            .collect();
        let view = self.mail.get_or_insert_with(MailView::default);
        if view.shown != shown {
            view.chosen = None;
        }
        view.shown = shown;
        cx.notify();
    }

    fn open_mail_pdf(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(view) = self.mail.as_mut() else {
            return;
        };
        let Some(pdf) = view.shown.get(i).cloned() else {
            return;
        };
        if view.opening.is_some() {
            return;
        }
        view.opening = Some(i);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move { mail::open(&pdf) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.mail.as_mut() {
                    view.opening = None;
                }
                match opened {
                    Ok(path) => cx.emit(LibraryEvent::Open(path, None)),
                    Err(e) => this.notice = Some(t("library-mail-open-failed", &[("error", &e)])),
                }
                // It may have been downloaded just now.
                this.load_mail(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn mail_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.mail.as_ref().map_or(0, |v| v.shown.len());
        if count == 0 {
            return self.message(t("library-mail-none", &[]), None);
        }
        let columns = self.columns();
        let rows = count.div_ceil(columns);
        self.grid_frame(
            "mail-grid",
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(count);
                        let cells: Vec<AnyElement> = (start..end)
                            .map(|i| this.mail_cell(i, cx).into_any_element())
                            .collect();
                        div()
                            .h(px(CELL_HEIGHT))
                            .flex()
                            .children(cells)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .into_any_element()
    }

    fn mail_cell(&mut self, i: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let Some(view) = self.mail.as_ref() else {
            return div().into_any_element();
        };
        let Some(pdf) = view.shown.get(i) else {
            return div().into_any_element();
        };
        let chosen = view.chosen == Some(i);
        let line = if view.opening == Some(i) {
            t("library-drive-downloading", &[])
        } else {
            pdf.from.clone()
        };
        div()
            .id(("mail-pdf", i))
            .w(px(CELL_WIDTH))
            .h(px(CELL_HEIGHT))
            .flex_none()
            .p(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .when(chosen, |d| d.bg(gpui::rgba(th.katna.row_selected)))
            .when(!chosen, |d| d.hover(move |s| s.bg(th.hover)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.open_mail_pdf(i, cx);
                } else if let Some(view) = this.mail.as_mut() {
                    view.chosen = Some(i);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .h(px(COVER_BOX))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .bg(th.backdrop)
                    .child(
                        svg()
                            .path("icons/document.svg")
                            .size(px(48.0))
                            .text_color(th.dim),
                    ),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(pdf.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(line),
            )
            .into_any_element()
    }

    /// The chosen attachment's strip: its name, its mail, and Open.
    pub(super) fn mail_details(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let th = self.theme;
        let view = self.mail.as_ref()?;
        let i = view.chosen?;
        let pdf = view.shown.get(i)?;
        let opening = view.opening.is_some();
        let about = if pdf.subject.trim().is_empty() {
            t("library-mail-from", &[("from", &pdf.from)])
        } else {
            t(
                "library-mail-about",
                &[("from", &pdf.from), ("subject", pdf.subject.trim())],
            )
        };
        let mut size = size_text(pdf.size);
        if !pdf.downloaded {
            size = t("library-mail-not-downloaded", &[("size", &size)]);
        }
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S4))
                .py(px(space::S2))
                .border_t_1()
                .border_color(th.line)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(pdf.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child(format!("{about} · {size}")),
                        ),
                )
                .child(
                    widgets::filled_button(
                        "library-mail-open",
                        if opening {
                            t("library-drive-downloading", &[])
                        } else {
                            t("library-open", &[])
                        },
                        &th.katna,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_mail_pdf(i, cx))),
                )
                .into_any_element(),
        )
    }
}
