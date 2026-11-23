// SPDX-License-Identifier: GPL-3.0-or-later

//! Pop-up notes: a comment opened in a small window of its own over the
//! page, beside what it is about, as Acrobat and Okular open one on a
//! double-click. It stays open while the pages are read, moves by its
//! title bar, scrolls and zooms with its page, shows the replies, and
//! its words are typed into in place. Several can be open at once.

use gpui::{AnyElement, Context, Window, div, prelude::*};
use katna_pdf_cos::{Annotation, Ref};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::tokens::{radius, space, text};

use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// The width of a pop-up note, in points of screen.
const WIDTH: f32 = 260.0;

/// A comment open in a pop-up note.
pub(super) struct Popup {
    pub id: Ref,
    pub page: usize,
    /// Its top left corner, in points of the page as shown at 100 %
    /// (from the page's top left), so it keeps its place when zoomed.
    pub at: [f32; 2],
}

/// A pop-up note being moved by its title bar: which, where the
/// pointer was pressed, and where the note was then.
pub(super) struct PopupDrag {
    pub index: usize,
    pub from: gpui::Point<gpui::Pixels>,
    pub at: [f32; 2],
}

impl Viewer {
    /// Page `page`'s size as shown at 100 %, in points.
    fn page_size_shown(&self, page: usize) -> (f32, f32) {
        match &self.content {
            Content::Ready(opened) => opened.doc.page_size(page).unwrap_or((612.0, 792.0)),
            _ => (612.0, 792.0),
        }
    }

    /// Opens the chosen comment in a pop-up note beside it, or brings it
    /// to the front when it is open already.
    pub(super) fn open_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_annotation().cloned() else {
            return;
        };
        let Some(id) = a.id else {
            return;
        };
        let Some(page) = self.annots.as_ref().and_then(|all| {
            all.iter()
                .position(|on| on.iter().any(|x| x.id == Some(id)))
        }) else {
            return;
        };
        if let Some(i) = self.popups.iter().position(|p| p.id == id) {
            let p = self.popups.remove(i);
            self.popups.push(p);
        } else {
            let Some(transform) = self.page_transform(page) else {
                return;
            };
            let b = katna_pdf_render::map_rect(transform, a.rect);
            let (w, _) = self.page_size_shown(page);
            // To the right of what it is about, or to its left when the
            // page ends first.
            let room = WIDTH / self.shown;
            let x = if b[2] + 8.0 + room <= w {
                b[2] + 8.0
            } else {
                (b[0] - 8.0 - room).max(0.0)
            };
            self.popups.push(Popup {
                id,
                page,
                at: [x, b[1].max(0.0)],
            });
        }
        // A note with nothing said yet opens ready to type in.
        if a.contents.trim().is_empty() && !a.flags.locked {
            self.edit_comment_in(true, window, cx);
        }
        cx.notify();
    }

    /// Closes the pop-up note of comment `id`, keeping what was typed.
    pub(super) fn close_popup(&mut self, id: Ref, cx: &mut Context<Self>) {
        if self
            .comment
            .as_ref()
            .is_some_and(|c| c.id == id && c.in_popup)
        {
            self.finish_comment(true, cx);
        }
        self.popups.retain(|p| p.id != id);
        cx.notify();
    }

    /// Pop-up notes whose comment is gone (deleted, undone, another
    /// file) are closed.
    pub(super) fn prune_popups(&mut self) {
        let Some(annots) = self.annots.as_ref() else {
            self.popups.clear();
            return;
        };
        self.popups.retain(|p| {
            annots
                .get(p.page)
                .is_some_and(|on| on.iter().any(|a| a.id == Some(p.id)))
        });
    }

    /// The pointer moved with a note's title bar held: the note follows.
    pub(super) fn popup_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.popup_dragging.as_ref() else {
            return false;
        };
        let dx = f32::from(position.x - drag.from.x) / self.shown;
        let dy = f32::from(position.y - drag.from.y) / self.shown;
        let at = [drag.at[0] + dx, drag.at[1] + dy];
        let index = drag.index;
        let Some(page) = self.popups.get(index).map(|p| p.page) else {
            return true;
        };
        let (w, h) = self.page_size_shown(page);
        let shown = self.shown;
        if let Some(p) = self.popups.get_mut(index) {
            p.at = [
                at[0].clamp(-WIDTH / shown + 40.0, w - 40.0),
                at[1].clamp(0.0, (h - 20.0).max(0.0)),
            ];
        }
        cx.notify();
        true
    }

    pub(super) fn popup_release(&mut self, cx: &mut Context<Self>) -> bool {
        if self.popup_dragging.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// The pop-up notes open on page `page`, in the order opened (the
    /// last over the others).
    pub(super) fn popup_notes(
        &self,
        page: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(annots) = self.annots.as_ref().and_then(|all| all.get(page)) else {
            return Vec::new();
        };
        self.popups
            .iter()
            .enumerate()
            .filter(|(_, p)| p.page == page)
            .filter_map(|(i, p)| {
                let a = annots.iter().find(|a| a.id == Some(p.id))?;
                Some(self.popup_note(i, p, a, annots, window, cx))
            })
            .collect()
    }

    fn popup_note(
        &self,
        index: usize,
        popup: &Popup,
        a: &Annotation,
        annots: &[Annotation],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let id = popup.id;
        let tint: gpui::Hsla = gpui::Rgba {
            r: a.colour[0],
            g: a.colour[1],
            b: a.colour[2],
            a: 1.0,
        }
        .into();
        let author = if a.author.trim().is_empty() {
            t("popup-no-author", &[])
        } else {
            a.author.clone()
        };
        let when = a.modified.as_deref().and_then(super::comments::age);
        let title = div()
            .id(("popup-title", index))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .pl(px(space::S3))
            .pr(px(space::S1))
            .py(px(space::S1))
            .border_t_4()
            .border_color(tint)
            .cursor_move()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(i) = this.popups.iter().position(|p| p.id == id) else {
                        return;
                    };
                    // The note pressed comes to the front.
                    let p = this.popups.remove(i);
                    let at = p.at;
                    this.popups.push(p);
                    this.popup_dragging = Some(PopupDrag {
                        index: this.popups.len() - 1,
                        from: event.position,
                        at,
                    });
                    cx.notify();
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(author),
                    )
                    .children(when.map(|when| {
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(when)
                    })),
            )
            .child(
                float::sized_ib(
                    ("popup-close", index),
                    "x",
                    24.0,
                    12.0,
                    t("popup-close", &[]),
                    false,
                    &th,
                )
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| this.close_popup(id, cx))),
            );
        let typing = self
            .comment
            .as_ref()
            .filter(|c| c.id == id && c.in_popup)
            .map(|c| c.input.clone());
        let locked = a.flags.locked;
        let body = match typing {
            Some(input) => div()
                .px(px(space::S2))
                .pb(px(space::S2))
                .child(float::input_box(&input, window, cx, &th))
                .into_any_element(),
            None => {
                let words = if a.contents.trim().is_empty() {
                    div()
                        .text_color(th.dim)
                        .child(t(if locked { "popup-empty" } else { "popup-type" }, &[]))
                } else {
                    div().text_color(th.text).child(a.contents.clone())
                };
                div()
                    .id(("popup-words", index))
                    .px(px(space::S3))
                    .pb(px(space::S2))
                    .text_size(px(text::SMALL))
                    .when(!locked, |d| d.cursor_text())
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if locked {
                            return;
                        }
                        this.choose(Some(id), cx);
                        this.edit_comment_in(true, window, cx);
                    }))
                    .child(words)
                    .into_any_element()
            }
        };
        // The replies, oldest first, under the comment.
        let replies: Vec<AnyElement> = annots
            .iter()
            .filter(|r| r.reply_to == Some(id) && r.state.is_none() && !r.grouped)
            .map(|r| {
                div()
                    .mx(px(space::S2))
                    .mb(px(space::S2))
                    .px(px(space::S2))
                    .py(px(space::S1))
                    .rounded(px(radius::SM))
                    .bg(th.hover)
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.dim)
                            .child(r.author.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(r.contents.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        let shown = self.shown;
        div()
            .id(("popup", index))
            .absolute()
            .left(desktop_px(popup.at[0] * shown))
            .top(desktop_px(popup.at[1] * shown))
            .w(px(WIDTH))
            .max_h(px(320.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(radius::SM))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow(th.lifted())
            .occlude()
            // Presses on the note are the note's, not the page's.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(title)
            .child(
                div()
                    .id(("popup-scroll", index))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body)
                    .children(replies),
            )
            .into_any_element()
    }
}
