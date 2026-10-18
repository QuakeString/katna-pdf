// SPDX-License-Identifier: GPL-3.0-or-later

//! Cropping pages (docs/PLAN.md, Phase 2, page operations): with crop
//! on, a box dragged on a page keeps only that part of it, and of the
//! other pages picked with it; a click on a page uncrops it.

use gpui::{AnyElement, Context, div, prelude::*};
use katna_ui::scale::desktop_px;

use super::annots::Draft;
use super::{Pages, Viewer};

/// A drag shorter than this, in points, is a click: it uncrops.
const CLICK: f32 = 4.0;

impl Viewer {
    /// Turns cropping on or off; the annotation tool is put down and the
    /// pages shown, to drag on.
    pub(super) fn toggle_crop(&mut self, cx: &mut Context<Self>) {
        self.cropping = !self.cropping;
        if self.cropping {
            self.tool = None;
            self.organizer = false;
        }
        self.draft = None;
        cx.notify();
    }

    /// A press on the pages while cropping starts the box.
    pub(super) fn crop_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.cropping {
            return false;
        }
        if let Some((page, point)) = self.page_point(position) {
            self.selection = None;
            self.draft = Some(Draft {
                page,
                path: vec![point],
            });
            cx.notify();
        }
        true
    }

    /// The pointer let go while cropping: crops to the box, or uncrops
    /// on a click. The pages picked are cropped alike when the page is
    /// one of them.
    pub(super) fn crop_release(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.cropping {
            return false;
        }
        let Some(draft) = self.draft.take() else {
            return true;
        };
        cx.notify();
        if self.changing_pages {
            return true;
        }
        let (Some(from), Some(to)) = (draft.path.first(), draft.path.last()) else {
            return true;
        };
        let click = (from[0] - to[0]).abs() < CLICK && (from[1] - to[1]).abs() < CLICK;
        let crop = if click {
            None
        } else {
            let Some(inv) = self.inverse(draft.page) else {
                return true;
            };
            Some(inv.rect([
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ]))
        };
        let pages = if self.picked.contains(&draft.page) {
            self.picked.clone()
        } else {
            [draft.page].into()
        };
        let picked = self.picked.clone();
        self.change_pages(Pages::Crop { pages, crop }, picked, cx);
        true
    }

    /// The box being dragged on page `page`.
    pub(super) fn crop_overlay(&self, page: usize) -> Option<AnyElement> {
        let draft = self
            .draft
            .as_ref()
            .filter(|d| self.cropping && d.page == page)?;
        let (from, to) = (draft.path.first()?, draft.path.last()?);
        let shown = self.shown;
        let (x0, y0) = (from[0].min(to[0]), from[1].min(to[1]));
        let (x1, y1) = (from[0].max(to[0]), from[1].max(to[1]));
        let accent = self.theme.katna.accent;
        Some(
            div()
                .absolute()
                .left(desktop_px(x0 * shown))
                .top(desktop_px(y0 * shown))
                .w(desktop_px((x1 - x0) * shown))
                .h(desktop_px((y1 - y0) * shown))
                .border_2()
                .border_color(gpui::rgba(accent))
                .bg(gpui::rgba(accent & 0xffff_ff00 | 0x1a))
                .into_any_element(),
        )
    }
}
