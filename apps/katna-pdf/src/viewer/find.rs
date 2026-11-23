// SPDX-License-Identifier: GPL-3.0-or-later

//! The find bar (Ctrl+F): what is looked for, how it matches (case,
//! whole words, accents, close spellings; a query in quotes is an exact
//! phrase), the count, and, when asked, every match listed with the words
//! around it, a click going to it. The matching is
//! `katna_pdf_render::text::PageText::find_with`; the search runs in the
//! background (`crate::search`).

use gpui::{AnyElement, Context, FontWeight, div, prelude::*};
use katna_pdf_render::text::FindOptions;
use katna_ui::px;
use katna_ui::tokens::{space, text};

use super::Viewer;
use crate::float::{self, FloatTip};
use crate::text::t;

/// Matches listed at most; the count still says how many there are.
const LISTED: usize = 300;

impl Viewer {
    /// The find bar under the title bar.
    pub(super) fn find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.search.as_ref()?;
        let th = self.theme;
        let found = &bar.found;
        let empty = bar.input.read(cx).text().trim().is_empty();
        let status = if found.hits.is_empty() {
            if empty {
                String::new()
            } else if found.done() {
                t("search-none", &[])
            } else {
                t("search-searching", &[])
            }
        } else {
            let current = found.current.map_or(0, |c| c + 1).to_string();
            let total = found.hits.len().to_string();
            let count = t("search-count", &[("current", &current), ("total", &total)]);
            if found.done() {
                count
            } else {
                format!("{count}…")
            }
        };
        let options = self.find_options;
        let option = |id: &'static str, words: &str, on: bool, flip: fn(&mut FindOptions)| {
            float::chip(id, t(words, &[]), on, &th)
                .ftip(t(&format!("{words}-tip"), &[]), &th)
                .on_click(cx.listener(move |this, _, _, cx| {
                    flip(&mut this.find_options);
                    this.start_search(cx);
                }))
        };
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(10.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(th.accent)
                    .bg(th.card)
                    .text_size(px(text::SMALL))
                    .text_color(th.text)
                    .child(float::icon("search", 15.0, th.dim))
                    .child(div().flex_1().min_w_0().child(bar.input.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(text::MICRO))
                            .text_color(th.dim)
                            .whitespace_nowrap()
                            .child(status),
                    ),
            )
            .child(
                float::ib(
                    "find-list",
                    "list",
                    t("search-list", &[]),
                    self.find_list,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.find_list = !this.find_list;
                    cx.notify();
                })),
            )
            .child(
                float::ib(
                    "find-previous",
                    "chevu",
                    t("search-previous", &[]),
                    false,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step_search(false, cx))),
            )
            .child(
                float::ib("find-next", "chev", t("search-next", &[]), false, &th)
                    .on_click(cx.listener(|this, _, _, cx| this.step_search(true, cx))),
            )
            .child(
                float::ib("find-close", "x", t("search-close", &[]), false, &th)
                    .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx))),
            );
        let options_row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::S2))
            .child(option(
                "find-case",
                "search-match-case",
                options.match_case,
                |o| o.match_case = !o.match_case,
            ))
            .child(option(
                "find-words",
                "search-whole-words",
                options.whole_words,
                |o| o.whole_words = !o.whole_words,
            ))
            .child(option(
                "find-accents",
                "search-accents",
                options.match_accents,
                |o| o.match_accents = !o.match_accents,
            ))
            .child(option("find-fuzzy", "search-fuzzy", options.fuzzy, |o| {
                o.fuzzy = !o.fuzzy
            }));
        let list = (self.find_list && !found.hits.is_empty()).then(|| {
            let rows = found.hits.iter().take(LISTED).enumerate().map(|(i, hit)| {
                let on = found.current == Some(i);
                let (before, matched, after) = hit.context.clone();
                let page = (hit.page + 1).to_string();
                div()
                    .id(("find-hit", i))
                    .flex()
                    .items_baseline()
                    .gap(px(space::S3))
                    .px(px(space::S2))
                    .py(px(space::S1))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .when(on, |d| d.bg(th.hover))
                    .hover(|s| s.bg(th.hover))
                    .text_size(px(text::CAPTION))
                    .child(
                        div()
                            .flex_none()
                            .w(px(40.0))
                            .text_color(th.dim)
                            .child(t("search-page", &[("page", &page)])),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(th.dim)
                            .child(before)
                            .child(
                                div()
                                    .flex_none()
                                    .px(px(2.0))
                                    .rounded(px(3.0))
                                    .bg(th.accent_soft)
                                    .text_color(th.text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(matched),
                            )
                            .child(after)
                            .flex()
                            .flex_row(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let Some(bar) = this.search.as_mut() else {
                            return;
                        };
                        let Some(hit) = bar.found.hits.get(i).cloned() else {
                            return;
                        };
                        bar.found.current = Some(i);
                        this.go_to_hit(&hit, cx);
                    }))
            });
            let more = found.hits.len().saturating_sub(LISTED);
            div()
                .id("find-hits")
                .max_h(px(320.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(th.line)
                .pt(px(space::S2))
                .children(rows)
                .when(more > 0, |d| {
                    d.child(
                        div()
                            .px(px(space::S2))
                            .py(px(space::S1))
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(t("search-more", &[("count", &more.to_string())])),
                    )
                })
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .p(px(space::S3))
                .child(input_row)
                .child(options_row)
                .children(list)
                .into_any_element(),
        )
    }
}
