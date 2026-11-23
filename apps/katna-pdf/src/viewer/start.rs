// SPDX-License-Identifier: GPL-3.0-or-later

//! The start screen (S01): Open and the library on the left, with the
//! files open when the app last ran; the files opened lately on the
//! right, each at the page last read.

use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};
use katna_ui::px;
use katna_ui::tokens::{space, text};

use super::{Viewer, ViewerEvent, file_name, short_folder};
use crate::float::{self, button};
use crate::history::History;
use crate::text::t;

impl Viewer {
    pub(super) fn start_screen(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let (last, recent): (Vec<String>, Vec<_>) = {
            let shown = cx.global::<crate::prefs::Prefs>().recent;
            let history = cx.global::<History>();
            (
                history
                    .last_session()
                    .iter()
                    .map(|p| file_name(p).trim_end_matches(".pdf").to_owned())
                    .collect(),
                history.recent().iter().take(shown).cloned().collect(),
            )
        };
        let any_recent = !recent.is_empty();
        // Two pages, one leaning, as the storyboard draws them.
        let sheet = |lean: bool| {
            div()
                .w(px(54.0))
                .h(px(70.0))
                .rounded(px(3.0))
                .bg(th.card)
                .shadow(th.shadow())
                .when(lean, |d| d.mt(px(4.0)))
        };
        let mut left = div()
            .w(px(340.0))
            .flex()
            .flex_col()
            .gap(px(space::S4))
            .child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .child(sheet(true))
                    .child(sheet(false)),
            )
            .child(
                div()
                    .mt(px(space::S4))
                    .text_size(px(text::DISPLAY))
                    .font_weight(FontWeight::BOLD)
                    .text_color(th.text)
                    .child(t("empty-title", &[])),
            )
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("start-detail", &[])),
            )
            .child(
                div()
                    .mt(px(space::S3))
                    .flex()
                    .gap(px(space::S3))
                    .child(
                        button("start-open", t("empty-open", &[]), true, &th)
                            .h(px(36.0))
                            .child(
                                div()
                                    .px(px(5.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(th.on_accent)
                                    .text_size(px(text::MICRO))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Ctrl O"),
                            )
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ViewerEvent::ChooseFiles))),
                    )
                    .child(
                        button("start-library", t("empty-library", &[]), false, &th)
                            .h(px(36.0))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ViewerEvent::ShowLibrary))),
                    ),
            );
        let left_over = cx.global::<crate::recovery::Left>().0.clone();
        if !left_over.is_empty() {
            let rows = left_over.into_iter().enumerate().map(|(i, copy)| {
                let name = file_name(&copy.path);
                let folder = copy.path.parent().map(short_folder).unwrap_or_default();
                let thrown = copy.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(text::SMALL))
                                    .text_color(th.text)
                                    .truncate()
                                    .child(SharedString::from(name)),
                            )
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .truncate()
                                    .child(folder),
                            ),
                    )
                    .child(
                        button(("recover-drop", i), t("recovery-drop", &[]), false, &th)
                            .h(px(28.0))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                crate::recovery::forget(&thrown.path);
                                cx.global_mut::<crate::recovery::Left>()
                                    .0
                                    .retain(|c| *c != thrown);
                                cx.notify();
                            })),
                    )
                    .child(
                        button(("recover-open", i), t("recovery-open", &[]), true, &th)
                            .h(px(28.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.global_mut::<crate::recovery::Left>()
                                    .0
                                    .retain(|c| *c != copy);
                                this.recover(copy.clone(), cx);
                            })),
                    )
            });
            left = left.child(
                div()
                    .mt(px(space::S6))
                    .p(px(space::S5))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .rounded(px(10.0))
                    .bg(th.card)
                    .shadow(th.shadow())
                    .border_1()
                    .border_color(th.accent)
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("recovery-title", &[])),
                    )
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(t("recovery-detail", &[])),
                    )
                    .children(rows),
            );
        }
        if !last.is_empty() {
            let count = last.len().to_string();
            left = left.child(
                div()
                    .mt(px(space::S6))
                    .p(px(space::S5))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .rounded(px(10.0))
                    .bg(th.card)
                    .shadow(th.shadow())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(text::SMALL))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(t("empty-session", &[])),
                            )
                            .child(
                                button(
                                    "start-session",
                                    t("start-open-all", &[("count", &count)]),
                                    false,
                                    &th,
                                )
                                .h(px(28.0))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(ViewerEvent::ReopenSession)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(last.join(" · ")),
                    ),
            );
        }
        let rows = recent.into_iter().enumerate().map(|(i, entry)| {
            let folder = entry.path.parent().map(short_folder).unwrap_or_default();
            let name = file_name(&entry.path);
            let path = entry.path;
            let where_ = if entry.page > 0 {
                t(
                    "start-at-page",
                    &[("folder", &folder), ("page", &(entry.page + 1).to_string())],
                )
            } else {
                folder
            };
            let hover = th.card;
            div()
                .id(("recent", i))
                .flex()
                .items_center()
                .gap(px(space::S4))
                .px(px(10.0))
                .py(px(space::S3))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| this.open(path.clone(), cx)))
                .child(
                    div()
                        .w(px(30.0))
                        .h(px(40.0))
                        .flex_none()
                        .rounded(px(3.0))
                        .bg(th.card)
                        .border_1()
                        .border_color(th.line_soft)
                        .p(px(5.0))
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(div().h(px(2.0)).w(px(16.0)).bg(th.line))
                        .child(div().h(px(2.0)).w(px(12.0)).bg(th.line)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(th.text)
                                .truncate()
                                .child(SharedString::from(name)),
                        )
                        .child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(th.dim)
                                .truncate()
                                .child(where_),
                        ),
                )
        });
        let right = div()
            .w(px(480.0))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(float::heading(t("start-recent", &[]), &th))
            .children(rows);
        div()
            .id("start")
            .flex_1()
            .size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(64.0))
            .bg(th.page)
            .child(left)
            .when(any_recent, |d| d.child(right))
            .children(self.crashed_card(cx))
            .into_any_element()
    }

    /// The card over the start screen when the app closed unexpectedly
    /// last time (S11): where the report is, the files that were open,
    /// and opening them again.
    fn crashed_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !cx.global::<crate::crashed::Crashed>().0 {
            return None;
        }
        let th = self.theme;
        let kept: Vec<std::path::PathBuf> = cx
            .global::<crate::recovery::Left>()
            .0
            .iter()
            .map(|k| k.path.clone())
            .collect();
        let files: Vec<String> = cx
            .global::<History>()
            .last_session()
            .iter()
            .map(|p| {
                let name = file_name(p).trim_end_matches(".pdf").to_owned();
                if kept.contains(p) {
                    t("crashed-kept", &[("name", &name)])
                } else {
                    name
                }
            })
            .collect();
        let n = files.len();
        let dismiss = |cx: &mut gpui::App| {
            cx.global_mut::<crate::crashed::Crashed>().0 = false;
            cx.refresh_windows();
        };
        let card = div()
            .id("crashed-card")
            .occlude()
            .w(px(420.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(th.card)
            .shadow(th.lifted())
            .when(th.dark, |d| d.border_1().border_color(th.line))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(space::S4))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(float::icon(
                                "alert",
                                18.0,
                                gpui::rgba(th.katna.warning).into(),
                            ))
                            .child(
                                div()
                                    .text_size(px(text::SUBTITLE))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(t("crashed-title", &[])),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(t("crashed-detail", &[])),
                    )
                    .when(n > 0, |d| {
                        d.child(
                            div()
                                .p(px(space::S3))
                                .rounded(px(8.0))
                                .bg(th.field)
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(text::SMALL))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(th.text)
                                        .child(match n {
                                            1 => t("crashed-one-open", &[]),
                                            n => t("crashed-open", &[("n", &n.to_string())]),
                                        }),
                                )
                                .child(
                                    div()
                                        .text_size(px(text::CAPTION))
                                        .text_color(th.dim)
                                        .child(files.join(", ")),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .border_t_1()
                    .border_color(th.line_soft)
                    .child(
                        button("crashed-report", t("crashed-report", &[]), false, &th).on_click(
                            |_, _, cx| {
                                if let Some(dir) = crate::crashed::reports() {
                                    cx.open_with_system(&dir);
                                }
                            },
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        button("crashed-fresh", t("crashed-fresh", &[]), false, &th)
                            .on_click(move |_, _, cx| dismiss(cx)),
                    )
                    .when(n > 0, |d| {
                        d.child(
                            button(
                                "crashed-reopen",
                                match n {
                                    1 => t("crashed-reopen-one", &[]),
                                    n => t("crashed-reopen", &[("n", &n.to_string())]),
                                },
                                true,
                                &th,
                            )
                            .on_click(move |_, window, cx| {
                                dismiss(cx);
                                window.dispatch_action(Box::new(super::ReopenSession), cx);
                            }),
                        )
                    }),
            );
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(th.scrim)
                .flex()
                .items_center()
                .justify_center()
                .child(float::appear("crashed-in", card, 8.0))
                .into_any_element(),
        )
    }
}
