// SPDX-License-Identifier: GPL-3.0-or-later

//! Go to page (Ctrl+G): a small bar over the dock taking a page's number,
//! or its label as the file numbers it (`iv`, `A-3`), and going there.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::{Content, Viewer, said};
use crate::text::t;

/// The bar while it is open.
pub(super) struct GoingTo {
    input: Entity<TextInput>,
    _sub: Subscription,
}

impl Viewer {
    /// Opens the bar, or closes it.
    pub(super) fn toggle_goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.going_to.take().is_some() {
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let count = self.page_count().to_string();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("goto-placeholder", &[("count", &count)]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe_in(&input, window, |this, input, event, window, cx| {
            match event {
                InputEvent::Submit => {
                    let typed = input.read(cx).text().trim().to_owned();
                    match this.page_named(&typed) {
                        Some(page) => {
                            this.going_to = None;
                            window.focus(&this.focus, cx);
                            this.go_to_page(page, cx);
                        }
                        None => {
                            this.notice = Some(said(
                                "goto-no-page",
                                &[("typed", &typed), ("count", &this.page_count().to_string())],
                            ));
                        }
                    }
                }
                InputEvent::Cancel => {
                    this.going_to = None;
                    window.focus(&this.focus, cx);
                }
                InputEvent::Changed => {}
            }
            cx.notify();
        });
        let handle = input.focus_handle(cx);
        window.focus(&handle, cx);
        cx.defer_in(window, move |_, window, cx| window.focus(&handle, cx));
        self.going_to = Some(GoingTo { input, _sub: sub });
        cx.notify();
    }

    /// The page `typed` names: a label the file gives a page (first
    /// match, case aside), else a page number from 1.
    pub(super) fn page_named(&self, typed: &str) -> Option<usize> {
        let count = self.page_count();
        if typed.is_empty() {
            return None;
        }
        if !self.label_ranges.is_empty()
            && let Some(page) = (0..count).find(|&p| self.page_label(p).eq_ignore_ascii_case(typed))
        {
            return Some(page);
        }
        typed
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=count).contains(n))
            .map(|n| n - 1)
    }

    /// The bar over the dock.
    pub(super) fn goto_bar(&self) -> Option<AnyElement> {
        let going = self.going_to.as_ref()?;
        let th = self.theme;
        let labelled = !self.label_ranges.is_empty();
        Some(
            div()
                .h(px(44.0))
                .w(px(360.0))
                .max_w_full()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S4))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(th.text)
                        .child(t("goto-label", &[])),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(30.0))
                        .flex()
                        .items_center()
                        .px(px(space::S3))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(th.line)
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(going.input.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(if labelled {
                            t("goto-or-label", &[])
                        } else {
                            t("goto-of", &[("count", &self.page_count().to_string())])
                        }),
                )
                .into_any_element(),
        )
    }
}
