// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing the bookmarks (docs/PLAN.md §4.3, Phase 4): the contents
//! pane's buttons add one for the place shown, rename, move, nest and
//! delete them, and set where one goes; Ctrl+B adds one from anywhere.
//! Each change writes the whole outline again
//! (`katna_pdf_cos::Document::set_outline`).

use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, Subscription, Window, div, prelude::*,
};
use katna_pdf_cos::Destination;
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::space;
use katna_ui::widgets::{self, Tip};

use super::{Content, GAP, PaneTab, Viewer};
use crate::contents::{Change, Path};
use crate::text::t;

/// The bookmark whose title is being typed.
pub(super) struct Renaming {
    path: Path,
    input: Entity<TextInput>,
    _input: Subscription,
}

impl Viewer {
    /// The place shown: the page at the top of the view, and the height
    /// on it at the view's top edge.
    pub(super) fn here(&self) -> Destination {
        let page = self.current_page();
        let Content::Ready(opened) = &self.content else {
            return Destination { page, top: None };
        };
        let height = opened.doc.page_size(page).map_or(792.0, |(_, h)| h);
        let gap = if page == 0 { space::S5 } else { GAP / 2.0 };
        let scale = katna_ui::scale::scale();
        let offset = katna_ui::unpx(self.list.logical_scroll_top().offset_in_item);
        // As `go_to` places it, backwards.
        let below = ((offset - gap) * scale / self.shown).clamp(0.0, height);
        let top = match opened.doc.page_transform(page) {
            Some([a, b, c, d, e, f]) => {
                let det = a * d - b * c;
                if det.abs() < 1e-9 {
                    height - below
                } else {
                    // The point at the shown page's left edge.
                    let (x, y) = (-e, f64::from(below) - f);
                    ((-b * x + a * y) / det) as f32
                }
            }
            None => height - below,
        };
        Destination {
            page,
            top: Some(top.round()),
        }
    }

    /// A bookmark for the place shown, after the one picked, its title
    /// ready to type: Ctrl+B.
    pub(super) fn add_bookmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let here = self.here();
        self.pane = true;
        self.tab = PaneTab::Contents;
        let title = t("bookmark-new", &[("page", &(here.page + 1).to_string())]);
        if self.change_outline(Change::Add(title, here), cx) {
            self.start_renaming(window, cx);
        }
    }

    /// Makes `change` to the outline shown and writes it into the file.
    /// False when it could not be made (nothing picked, or at an end).
    pub(super) fn change_outline(&mut self, change: Change, cx: &mut Context<Self>) -> bool {
        let Some(contents) = self.contents.as_mut() else {
            return false;
        };
        let Some((items, chosen)) = contents.changed(&change) else {
            return false;
        };
        contents.items = items.clone();
        if let Some(path) = &chosen {
            contents.reveal(path);
        }
        contents.chosen = chosen;
        self.renaming = None;
        self.change_document(
            Vec::new(),
            "bookmark-failed",
            move |doc| doc.set_outline(&items).map_err(|e| e.to_string()),
            cx,
        );
        cx.notify();
        true
    }

    /// Puts the picked bookmark's title in a field to type over.
    pub(super) fn start_renaming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(contents) = self.contents.as_ref() else {
            return;
        };
        let Some(path) = contents.chosen.clone() else {
            return;
        };
        let Some(row) = contents.rows().into_iter().find(|r| r.path == path) else {
            return;
        };
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("bookmark-title", &[]), cx);
            input.set_accent(accent);
            input.set_text(row.title.clone(), cx);
            input.select_all_text(cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_renaming(true, cx),
            InputEvent::Cancel => this.finish_renaming(false, cx),
            _ => {}
        });
        let handle = input.focus_handle(cx);
        window.focus(&handle, cx);
        cx.defer_in(window, move |_, window, cx| window.focus(&handle, cx));
        self.renaming = Some(Renaming {
            path,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing a title, keeping it when `keep` and it is not empty.
    fn finish_renaming(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let title = renaming.input.read(cx).text().trim().to_owned();
        if keep && !title.is_empty() {
            if let Some(contents) = self.contents.as_mut() {
                contents.chosen = Some(renaming.path);
            }
            self.change_outline(Change::Rename(title), cx);
        }
        cx.notify();
    }

    /// The field for the title being typed, when it is `path`'s.
    pub(super) fn renaming_field(&self, path: &Path) -> Option<AnyElement> {
        let renaming = self.renaming.as_ref().filter(|r| &r.path == path)?;
        Some(
            div()
                .h(px(26.0))
                .child(renaming.input.clone())
                .into_any_element(),
        )
    }

    /// The contents pane's buttons.
    pub(super) fn outline_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let picked = self.contents.as_ref().is_some_and(|c| c.chosen.is_some());
        let button = |id: &'static str, icon: &'static str, tip: &str, enabled: bool| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .size(px(32.0))
                .tip(t(tip, &[]), &th.katna)
                .when(!enabled, |b| b.opacity(0.4))
        };
        let change = |change: Change| {
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.change_outline(change.clone(), cx);
            })
        };
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                button("bookmark-add", "bookmark-plus", "bookmark-add", true)
                    .on_click(cx.listener(|this, _, window, cx| this.add_bookmark(window, cx))),
            )
            .child(
                button("bookmark-rename", "rename", "bookmark-rename", picked)
                    .on_click(cx.listener(|this, _, window, cx| this.start_renaming(window, cx))),
            )
            .child(
                button("bookmark-place", "place", "bookmark-place", picked).on_click(cx.listener(
                    |this, _, _, cx| {
                        let here = this.here();
                        this.change_outline(Change::Place(here), cx);
                    },
                )),
            )
            .child(
                button("bookmark-up", "arrow-up", "bookmark-up", picked)
                    .on_click(change(Change::Up)),
            )
            .child(
                button("bookmark-down", "arrow-down", "bookmark-down", picked)
                    .on_click(change(Change::Down)),
            )
            .child(
                button("bookmark-outdent", "outdent", "bookmark-outdent", picked)
                    .on_click(change(Change::Outdent)),
            )
            .child(
                button("bookmark-indent", "indent", "bookmark-indent", picked)
                    .on_click(change(Change::Indent)),
            )
            .child(
                button("bookmark-delete", "trash", "bookmark-delete", picked)
                    .on_click(change(Change::Delete)),
            )
            .into_any_element()
    }
}
