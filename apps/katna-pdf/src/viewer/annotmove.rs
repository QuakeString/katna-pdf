// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving and resizing comments on the page, as in Acrobat and Okular:
//! press on a comment and drag it somewhere else, or drag a corner of
//! the one chosen to make it larger or smaller. Its points go with it
//! (`Annotation::moved`, `Annotation::resized`); text markup stays on
//! its words and locked comments stay put. Esc while dragging puts it
//! back.

use gpui::{AnyElement, Context, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, Annotation, Ref};
use katna_ui::px;
use katna_ui::scale::desktop_px;

use super::{Content, Viewer};

/// How far, in points on the page shown, the pointer goes before a
/// press becomes a drag.
const DRAG: f32 = 4.0;

/// How near a corner, in points on the page shown, takes it.
const GRIP: f32 = 6.0;

/// A comment being dragged.
pub(super) struct AnnotMove {
    id: Ref,
    page: usize,
    /// The corner dragged (left top, right top, right bottom, left
    /// bottom, as shown), or none: the whole comment moves.
    corner: Option<usize>,
    /// Where the press was and where the pointer is, on the page shown.
    from: [f32; 2],
    to: [f32; 2],
    /// Whether it went far enough to be a drag.
    dragged: bool,
    original: Annotation,
}

impl AnnotMove {
    /// The comment as it would be let go now.
    fn result(&self, viewer: &Viewer) -> Option<Annotation> {
        let inv = viewer.inverse(self.page)?;
        let (from, to) = (inv.map(self.from), inv.map(self.to));
        Some(match self.corner {
            None => self.original.moved(to[0] - from[0], to[1] - from[1]),
            Some(corner) => {
                let shown = viewer.shown_rect(self.page, self.original.rect)?;
                // The corner across stays; this one goes to the pointer.
                let across = match corner {
                    0 => [shown[2], shown[3]],
                    1 => [shown[0], shown[3]],
                    2 => [shown[0], shown[1]],
                    _ => [shown[2], shown[1]],
                };
                let [ax, ay] = inv.map(across);
                let [px_, py] = inv.map(self.to);
                // Never smaller than a few points across.
                const MIN: f32 = 4.0;
                let (x0, x1) = (ax.min(px_), ax.max(px_).max(ax.min(px_) + MIN));
                let (y0, y1) = (ay.min(py), ay.max(py).max(ay.min(py) + MIN));
                self.original.resized([x0, y0, x1, y1])
            }
        })
    }
}

impl Viewer {
    /// `rect` (page space) on page `page` as shown: left, top, right,
    /// bottom in points from its top left.
    pub(super) fn shown_rect(&self, page: usize, rect: [f32; 4]) -> Option<[f32; 4]> {
        let Content::Ready(opened) = &self.content else {
            return None;
        };
        Some(katna_pdf_render::map_rect(
            opened.doc.page_transform(page)?,
            rect,
        ))
    }

    /// The corner of the chosen comment at `point` on page `page`, if it
    /// may be resized and one is there.
    fn corner_at(&self, page: usize, point: [f32; 2]) -> Option<usize> {
        let a = self.chosen_annotation().filter(|a| a.page == page)?;
        if !a.resizable() {
            return None;
        }
        let b = self.shown_rect(page, a.rect)?;
        let grip = GRIP / self.shown.max(0.1);
        [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
            .iter()
            .position(|c| (c[0] - point[0]).abs() <= grip && (c[1] - point[1]).abs() <= grip)
    }

    /// A press with no tool in hand: on a corner of the chosen comment
    /// starts resizing it; on a comment that may move, chooses it and
    /// starts moving it. Returns whether it did.
    pub(super) fn annot_move_press(
        &mut self,
        page: usize,
        point: [f32; 2],
        hit: Option<Ref>,
    ) -> bool {
        if let Some(corner) = self.corner_at(page, point)
            && let Some(a) = self.chosen_annotation().cloned()
            && let Some(id) = a.id
        {
            self.annot_moving = Some(AnnotMove {
                id,
                page,
                corner: Some(corner),
                from: point,
                to: point,
                dragged: false,
                original: a,
            });
            return true;
        }
        let Some(id) = hit else {
            return false;
        };
        let a = self
            .annots
            .as_ref()
            .and_then(|all| all.get(page)?.iter().find(|a| a.id == Some(id)).cloned());
        if let Some(a) = a.filter(Annotation::movable) {
            self.annot_moving = Some(AnnotMove {
                id,
                page,
                corner: None,
                from: point,
                to: point,
                dragged: false,
                original: a,
            });
        }
        false
    }

    /// The pointer moved while a comment is held.
    pub(super) fn annot_move_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self.annot_moving.as_ref().map(|m| m.page) else {
            return false;
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            ((position.x - b.left()) / shown).clamp(0.0, (b.size.width / shown).max(0.0)),
            ((position.y - b.top()) / shown).clamp(0.0, (b.size.height / shown).max(0.0)),
        ];
        if let Some(m) = self.annot_moving.as_mut() {
            m.to = point;
            if !m.dragged {
                m.dragged = (point[0] - m.from[0]).hypot(point[1] - m.from[1]) * self.shown >= DRAG;
            }
        }
        cx.notify();
        true
    }

    /// Lets go of the comment held: it stays where it was put.
    pub(super) fn annot_move_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(m) = self.annot_moving.take() else {
            return false;
        };
        if !m.dragged {
            return false;
        }
        if let Some(a) = m.result(self)
            && a != m.original
        {
            self.choose(Some(m.id), cx);
            self.change_annotations(
                AnnotChanges {
                    put: vec![a],
                    remove: Vec::new(),
                },
                cx,
            );
        }
        cx.notify();
        true
    }

    /// Esc while dragging: the comment goes back.
    pub(super) fn cancel_annot_move(&mut self, cx: &mut Context<Self>) -> bool {
        if self.annot_moving.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// The chosen comment's corners to drag, and while one is held, its
    /// outline where it would go.
    pub(super) fn annot_move_overlays(&self, page: usize) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let shown = self.shown;
        let accent = gpui::rgba(self.theme.katna.accent);
        let held = self
            .annot_moving
            .as_ref()
            .filter(|m| m.page == page && m.dragged);
        if let Some(m) = held
            && let Some(a) = m.result(self)
            && let Some(b) = self.shown_rect(page, a.outer_rect())
        {
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(b[0] * shown))
                    .top(desktop_px(b[1] * shown))
                    .w(desktop_px((b[2] - b[0]) * shown))
                    .h(desktop_px((b[3] - b[1]) * shown))
                    .border_1()
                    .border_dashed()
                    .border_color(accent)
                    .bg(gpui::Hsla::from(accent).opacity(0.08))
                    .into_any_element(),
            );
            return out;
        }
        let Some(a) = self
            .chosen_annotation()
            .filter(|a| a.page == page && a.resizable())
        else {
            return out;
        };
        let Some(b) = self.shown_rect(page, a.rect) else {
            return out;
        };
        let size = 8.0;
        for (i, [x, y]) in [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
            .into_iter()
            .enumerate()
        {
            let handle = div()
                .absolute()
                .left(desktop_px(x * shown - size / 2.0))
                .top(desktop_px(y * shown - size / 2.0))
                .size(desktop_px(size))
                .rounded(px(2.0))
                .border_1()
                .border_color(accent)
                .bg(gpui::white());
            out.push(
                if i % 2 == 0 {
                    handle.cursor_nwse_resize()
                } else {
                    handle.cursor_nesw_resize()
                }
                .into_any_element(),
            );
        }
        out
    }
}
