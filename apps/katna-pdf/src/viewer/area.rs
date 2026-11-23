// SPDX-License-Identifier: GPL-3.0-or-later

//! Tools that act on an area dragged out on a page: Zoom to an area
//! (the area fills the view; a click zooms in a step there) and Snapshot
//! (the area copied as a picture, drawn sharper than on screen), and
//! the Loupe, a lens following the pointer that shows the page under it
//! three times larger, drawn sharp at that size. They stay in hand until
//! Escape or picked again.

use std::sync::Arc;

use gpui::{
    AnyElement, ClipboardItem, Context, Image, ImageFormat, ImageSource, RenderImage, div, img,
    prelude::*,
};
use katna_pdf_render::Region;
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::tokens::radius;

use super::{Content, POINT, STEPS, Viewer, Zoom};
use crate::text::t;

/// What an area tool does with the area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AreaKind {
    Zoom,
    Snapshot,
    Loupe,
}

/// A page and a point on it, in points.
type Spot = (usize, [f32; 2]);

/// The loupe's lens: where it is, in points on `page`, and the page
/// under it drawn larger.
#[derive(Default)]
pub(super) struct Lens {
    at: Option<Spot>,
    /// The drawing shown, and the spot it was drawn for.
    image: Option<(Spot, Arc<RenderImage>)>,
    /// Whether a drawing is under way.
    drawing: bool,
}

/// An area tool in hand, and the area being dragged out: its page and
/// corners in points from the page's top left as shown.
pub(super) struct AreaTool {
    pub(super) kind: AreaKind,
    drag: Option<(usize, [f32; 2], [f32; 2])>,
    lens: Lens,
}

/// The loupe's lens is this many logical pixels a side...
const LENS: f32 = 240.0;
/// ...and shows the page this many times larger than the view.
const MAGNIFY: f32 = 3.0;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// Snapshots are at most this many pixels a side.
const SNAPSHOT_SIDE: f32 = 4096.0;

impl Viewer {
    /// Picks up area tool `kind`, or puts it down if it is in hand.
    pub(super) fn toggle_area_tool(&mut self, kind: AreaKind, cx: &mut Context<Self>) {
        if self.area.as_ref().is_some_and(|a| a.kind == kind) {
            self.area = None;
        } else {
            self.tool = None;
            self.hand = false;
            self.selection = None;
            self.area = Some(AreaTool {
                kind,
                drag: None,
                lens: Lens::default(),
            });
        }
        cx.notify();
    }

    pub(super) fn area_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tool) = &self.area else {
            return false;
        };
        if tool.kind == AreaKind::Loupe {
            return true;
        }
        let point = self.page_point(position);
        if let (Some(tool), Some((page, p))) = (self.area.as_mut(), point) {
            tool.drag = Some((page, p, p));
            cx.notify();
        }
        true
    }

    pub(super) fn area_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self.area.as_ref().and_then(|a| a.drag).map(|d| d.0) else {
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
        if let Some(drag) = self.area.as_mut().and_then(|a| a.drag.as_mut()) {
            drag.2 = point;
        }
        cx.notify();
        true
    }

    /// The pointer moved with the loupe in hand: the lens follows it.
    pub(super) fn loupe_move(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.area.as_ref().is_none_or(|a| a.kind != AreaKind::Loupe) {
            return false;
        }
        let at = self.page_point(position);
        if let Some(tool) = self.area.as_mut() {
            tool.lens.at = at;
        }
        self.draw_lens(cx);
        cx.notify();
        true
    }

    /// Draws the page under the lens, if it is not drawn there already
    /// and no drawing is under way (that one, done, starts the next).
    fn draw_lens(&mut self, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let doc = opened.doc.clone();
        let colors = self.page_colors();
        let shown = self.shown;
        let Some(tool) = self.area.as_mut() else {
            return;
        };
        let lens = &mut tool.lens;
        let Some((page, at)) = lens.at else {
            return;
        };
        if lens.drawing
            || lens
                .image
                .as_ref()
                .is_some_and(|(spot, _)| *spot == (page, at))
        {
            return;
        }
        lens.drawing = true;
        let scale = shown * MAGNIFY * 2.0;
        let side = (LENS * 2.0) as u32;
        let region = Region {
            x: (at[0] * scale - side as f32 / 2.0).max(0.0) as u32,
            y: (at[1] * scale - side as f32 / 2.0).max(0.0) as u32,
            width: side,
            height: side,
        };
        let drawn = cx.background_executor().spawn(async move {
            let mut bitmap = doc.render_region(page, scale, region)?;
            colors.apply(&mut bitmap, &[]);
            Some(bitmap)
        });
        cx.spawn(async move |this, cx| {
            let bitmap = drawn.await;
            this.update(cx, |this, cx| {
                let Some(tool) = this.area.as_mut() else {
                    return;
                };
                tool.lens.drawing = false;
                if let Some(image) = bitmap.and_then(super::render_image)
                    && let Some((_, old)) = tool.lens.image.replace(((page, at), image))
                {
                    this.released.push(old);
                }
                this.draw_lens(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The loupe's lens over `page`, centred on the pointer.
    pub(super) fn lens_overlay(&self, page: usize) -> Option<AnyElement> {
        let tool = self.area.as_ref()?;
        let (p, at) = tool.lens.at?;
        if p != page {
            return None;
        }
        let ((_, drawn_at), image) = tool.lens.image.as_ref()?;
        let shown = self.shown;
        // The drawing is twice the lens in pixels, so half of it shows
        // at the lens's size; it starts at the spot it was drawn for
        // less half the drawing (or the page's edge).
        let scale = shown * MAGNIFY;
        let left = (drawn_at[0] * scale - LENS / 2.0).max(0.0);
        let top = (drawn_at[1] * scale - LENS / 2.0).max(0.0);
        let x = at[0] * scale - left - LENS / 2.0;
        let y = at[1] * scale - top - LENS / 2.0;
        // Drawn at two pixels to the lens's one.
        let size = image.size(0);
        let (w, h) = (size.width.0 as f32 / 2.0, size.height.0 as f32 / 2.0);
        Some(
            div()
                .absolute()
                .left(desktop_px(at[0] * shown - LENS / 2.0))
                .top(desktop_px(at[1] * shown - LENS / 2.0))
                .size(desktop_px(LENS))
                .rounded(px(radius::SM))
                .overflow_hidden()
                .border_2()
                .border_color(self.theme.accent)
                .bg(self.paper())
                .shadow(self.theme.page_shadow())
                .child(
                    img(ImageSource::Render(image.clone()))
                        .absolute()
                        .left(desktop_px(-x))
                        .top(desktop_px(-y))
                        .w(desktop_px(w))
                        .h(desktop_px(h)),
                )
                .into_any_element(),
        )
    }

    /// The pointer let go: the area is zoomed to or copied.
    pub(super) fn area_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(tool) = self.area.as_mut() else {
            return false;
        };
        let kind = tool.kind;
        let Some((page, from, to)) = tool.drag.take() else {
            return true;
        };
        cx.notify();
        let rect = [
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]),
            from[1].max(to[1]),
        ];
        let click = rect[2] - rect[0] < CLICK && rect[3] - rect[1] < CLICK;
        match kind {
            AreaKind::Zoom if click => {
                // A click zooms in a step, keeping that spot in the middle.
                let now = self.shown / POINT;
                if let Some(next) = STEPS.iter().copied().find(|s| *s > now * 1.01) {
                    self.zoom_to(page, [from[0], from[1], from[0], from[1]], next);
                }
            }
            AreaKind::Zoom => {
                let Some(view) = self.view_bounds.get() else {
                    return true;
                };
                let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
                let fits =
                    (f32::from(view.size.width) / w).min(f32::from(view.size.height) / h) * 0.95;
                let zoom = (fits / POINT).clamp(STEPS[0], STEPS[STEPS.len() - 1]);
                self.zoom_to(page, rect, zoom);
            }
            AreaKind::Snapshot if click => {}
            AreaKind::Snapshot => self.snapshot(page, rect, cx),
            AreaKind::Loupe => {}
        }
        true
    }

    /// Zooms to `zoom` and brings `rect` on `page` into the middle of
    /// the view once the pages are laid out at the new size.
    fn zoom_to(&mut self, page: usize, rect: [f32; 4], zoom: f32) {
        self.zoom = Zoom::Fixed(zoom);
        self.list.remeasure();
        self.list.scroll_to(gpui::ListOffset {
            item_ix: self.spread.row_of(page),
            offset_in_item: px(0.0),
        });
        self.area_focus = Some((page, rect, 0));
    }

    /// After a zoom to an area: once its page is laid out at the new
    /// size, scrolls so the area is in the middle of the view.
    pub(super) fn bring_area_into_view(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some((page, rect, tries)) = self.area_focus else {
            return;
        };
        let size = match &self.content {
            Content::Ready(opened) => opened.doc.page_size(page),
            _ => None,
        };
        let (Some(view), Some((page_w, _))) = (self.view_bounds.get(), size) else {
            self.area_focus = None;
            return;
        };
        let bounds = self.page_bounds.borrow().get(&page).copied();
        let shown = desktop_px(self.shown);
        // Laid out at the new size yet?
        let fresh = bounds.filter(|b| (b.size.width / shown - page_w).abs() < 1.0);
        let Some(b) = fresh else {
            // Give up after a few frames, rather than scroll later.
            self.area_focus = (tries < 10).then_some((page, rect, tries + 1));
            window.request_animation_frame();
            return;
        };
        self.area_focus = None;
        let middle = |a: f32, b: f32| (a + b) / 2.0 * self.shown;
        let x = f32::from(b.left()) + middle(rect[0], rect[2]);
        let y = f32::from(b.top()) + middle(rect[1], rect[3]);
        let view_x = f32::from(view.left()) + f32::from(view.size.width) / 2.0;
        let view_y = f32::from(view.top()) + f32::from(view.size.height) / 2.0;
        self.pan_by(x - view_x, cx);
        self.list.scroll_by(px(y - view_y));
        window.request_animation_frame();
    }

    /// Copies `rect` of `page` as a picture, drawn at twice the size on
    /// screen (at least 144 dots to the inch).
    fn snapshot(&mut self, page: usize, rect: [f32; 4], cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let doc = opened.doc.clone();
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let scale = (self.shown * 2.0)
            .max(2.0)
            .min(SNAPSHOT_SIDE / w.max(h).max(1.0));
        let region = Region {
            x: (rect[0] * scale).round() as u32,
            y: (rect[1] * scale).round() as u32,
            width: (w * scale).round().max(1.0) as u32,
            height: (h * scale).round().max(1.0) as u32,
        };
        let drawn = cx
            .background_executor()
            .spawn(async move { doc.render_region(page, scale, region).map(|b| b.png()) });
        cx.spawn(async move |this, cx| {
            let png = drawn.await;
            this.update(cx, |this, cx| {
                this.notice = Some(match png {
                    Some(png) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                            ImageFormat::Png,
                            png,
                        )));
                        t("snapshot-copied", &[])
                    }
                    None => t("snapshot-failed", &[]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The area being dragged out on `page`.
    pub(super) fn area_overlay(&self, page: usize) -> Option<AnyElement> {
        let (p, from, to) = self.area.as_ref()?.drag?;
        if p != page {
            return None;
        }
        let shown = self.shown;
        let accent = self.theme.katna.accent & 0xffff_ff00;
        Some(
            div()
                .absolute()
                .left(desktop_px(from[0].min(to[0]) * shown))
                .top(desktop_px(from[1].min(to[1]) * shown))
                .w(desktop_px((to[0] - from[0]).abs() * shown))
                .h(desktop_px((to[1] - from[1]).abs() * shown))
                .border_1()
                .border_color(gpui::rgba(accent | 0xff))
                .bg(gpui::rgba(accent | 0x22))
                .into_any_element(),
        )
    }
}
