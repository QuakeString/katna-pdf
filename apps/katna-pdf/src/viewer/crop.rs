// SPDX-License-Identifier: GPL-3.0-or-later

//! Cropping pages (E14): with crop on, a box dragged on a page, its
//! corners and its middle dragged to change it, and a bar over the tools
//! that crops this page, all of them, the odd or the even ones to it,
//! cuts each one's white margins, or puts the whole page back.

use std::collections::{BTreeMap, BTreeSet};

use gpui::{AnyElement, Context, SharedString, div, prelude::*};
use katna_pdf_render::Document;
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::tokens::space;

use super::{Content, Pages, Viewer};
use crate::float;
use crate::text::t;

/// A box smaller than this, in points, is no box.
const SMALLEST: f32 = 4.0;
/// How near a corner, in pixels on the screen, a press takes it.
const GRIP: f32 = 10.0;
/// A pixel lighter than this in every colour is white paper.
const WHITE: u8 = 235;
/// Points of paper kept around the ink when margins are cut.
const PAD: f32 = 4.0;

/// The box being made, on page `page`, in points from its top left as
/// it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CropBox {
    pub page: usize,
    pub rect: [f32; 4],
}

/// What a drag on the box does.
#[derive(Clone, Copy, Debug)]
pub(super) enum Grip {
    /// A new box, from this point.
    New([f32; 2]),
    /// This corner (0 top left, 1 top right, 2 bottom right, 3 bottom
    /// left) moves.
    Corner(usize),
    /// The whole box moves, from where it was when pressed at this point.
    Move([f32; 4], [f32; 2]),
}

/// Which pages the bar crops.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Which {
    #[default]
    This,
    All,
    Odd,
    Even,
}

const WHICH: [(Which, &str); 4] = [
    (Which::This, "crop-this"),
    (Which::All, "crop-all"),
    (Which::Odd, "crop-odd"),
    (Which::Even, "crop-even"),
];

impl Viewer {
    /// Turns cropping on or off; other tools are put down.
    pub(super) fn toggle_crop(&mut self, cx: &mut Context<Self>) {
        if !self.cropping && !self.allows(super::Allow::Assemble, cx) {
            return;
        }
        self.cropping = !self.cropping;
        if self.cropping {
            self.finish_text_edit(true, cx);
            self.tool = None;
            self.adding = None;
            self.redacting = None;
            self.linking = None;
            self.organizer = false;
            self.signing = None;
            self.picked_object = None;
        }
        self.crop_box = None;
        self.crop_grip = None;
        self.draft = None;
        cx.notify();
    }

    /// The pages `which` names, with `page` the one the box is on.
    fn crop_targets(&self, which: Which, page: usize) -> BTreeSet<usize> {
        let count = self.page_count();
        match which {
            Which::This if self.picked.contains(&page) => self.picked.clone(),
            Which::This => [page].into(),
            Which::All => (0..count).collect(),
            // Odd pages as people count them: 1, 3, 5…
            Which::Odd => (0..count).step_by(2).collect(),
            Which::Even => (1..count).step_by(2).collect(),
        }
    }

    /// A press on the pages while cropping: takes a corner of the box, or
    /// the box, or starts a new one.
    pub(super) fn crop_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.cropping {
            return false;
        }
        let Some((page, point)) = self.page_point(position) else {
            return true;
        };
        self.selection = None;
        let reach = GRIP / self.shown.max(0.01);
        self.crop_grip = Some(match self.crop_box.filter(|b| b.page == page) {
            Some(CropBox { rect, .. }) => {
                let corners = corners(rect);
                match corners.iter().position(|c| {
                    (c[0] - point[0]).abs() <= reach && (c[1] - point[1]).abs() <= reach
                }) {
                    Some(corner) => Grip::Corner(corner),
                    None if inside(rect, point) => Grip::Move(rect, point),
                    None => Grip::New(point),
                }
            }
            None => Grip::New(point),
        });
        if let Some(Grip::New(point)) = self.crop_grip {
            self.crop_box = Some(CropBox {
                page,
                rect: [point[0], point[1], point[0], point[1]],
            });
        }
        cx.notify();
        true
    }

    /// The pointer moved with the box in hand.
    pub(super) fn crop_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(grip), Some(cropped)) = (self.crop_grip, self.crop_box) else {
            return false;
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&cropped.page).copied() else {
            return true;
        };
        let (w, h) = (
            (b.size.width / shown).max(0.0),
            (b.size.height / shown).max(0.0),
        );
        let p = [
            ((position.x - b.left()) / shown).clamp(0.0, w),
            ((position.y - b.top()) / shown).clamp(0.0, h),
        ];
        let rect = match grip {
            Grip::New(from) => [
                from[0].min(p[0]),
                from[1].min(p[1]),
                from[0].max(p[0]),
                from[1].max(p[1]),
            ],
            Grip::Corner(corner) => {
                let [x0, y0, x1, y1] = cropped.rect;
                // The corner across stays where it is.
                let fixed = match corner {
                    0 => [x1, y1],
                    1 => [x0, y1],
                    2 => [x0, y0],
                    _ => [x1, y0],
                };
                [
                    fixed[0].min(p[0]),
                    fixed[1].min(p[1]),
                    fixed[0].max(p[0]),
                    fixed[1].max(p[1]),
                ]
            }
            Grip::Move(start, from) => {
                let (bw, bh) = (start[2] - start[0], start[3] - start[1]);
                let x = (start[0] + p[0] - from[0]).clamp(0.0, (w - bw).max(0.0));
                let y = (start[1] + p[1] - from[1]).clamp(0.0, (h - bh).max(0.0));
                [x, y, x + bw, y + bh]
            }
        };
        // A corner dragged past the one across turns into it.
        if let Grip::Corner(_) = grip {
            let [x0, y0, x1, y1] = rect;
            let corner = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
                .iter()
                .position(|c| (c[0] - p[0]).abs() < 0.01 && (c[1] - p[1]).abs() < 0.01);
            if let Some(corner) = corner {
                self.crop_grip = Some(Grip::Corner(corner));
            }
        }
        self.crop_box = Some(CropBox {
            page: cropped.page,
            rect,
        });
        cx.notify();
        true
    }

    /// The pointer let go: the box stays to be changed or cropped to; a
    /// click leaves none.
    pub(super) fn crop_release(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.cropping {
            return false;
        }
        if self.crop_grip.take().is_some() {
            if self.crop_box.is_some_and(|b| {
                b.rect[2] - b.rect[0] < SMALLEST || b.rect[3] - b.rect[1] < SMALLEST
            }) {
                self.crop_box = None;
            }
            cx.notify();
        }
        true
    }

    /// Crops the pages picked in the bar to the box.
    fn crop_to_box(&mut self, cx: &mut Context<Self>) {
        let Some(CropBox { page, rect }) = self.crop_box else {
            return;
        };
        if self.changing_pages {
            return;
        }
        // Each page takes the box where its own page shows it.
        let mut crops = BTreeMap::new();
        for p in self.crop_targets(self.crop_which, page) {
            if let Some(inv) = self.inverse(p) {
                crops.insert(p, inv.rect(rect));
            }
        }
        self.crop_box = None;
        let picked = self.picked.clone();
        self.change_pages(Pages::Crops(crops), picked, cx);
    }

    /// Puts the whole of the pages picked in the bar back.
    fn uncrop(&mut self, cx: &mut Context<Self>) {
        if self.changing_pages {
            return;
        }
        let page = self
            .crop_box
            .map_or_else(|| self.current_page(), |b| b.page);
        let pages = self.crop_targets(self.crop_which, page);
        self.crop_box = None;
        let picked = self.picked.clone();
        self.change_pages(Pages::Crop { pages, crop: None }, picked, cx);
    }

    /// Crops each page picked in the bar to what is drawn on it, with a
    /// little paper around.
    fn cut_white(&mut self, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        if self.changing_pages || self.crop_finding {
            return;
        }
        let page = self
            .crop_box
            .map_or_else(|| self.current_page(), |b| b.page);
        let pages = self.crop_targets(self.crop_which, page);
        let doc = opened.doc.clone();
        self.crop_finding = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let inked = cx
                .background_executor()
                .spawn(async move {
                    pages
                        .into_iter()
                        .filter_map(|p| ink_box(doc.as_ref(), p).map(|b| (p, b)))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.crop_finding = false;
                let mut crops = BTreeMap::new();
                for (p, rect) in inked {
                    if let Some(inv) = this.inverse(p) {
                        crops.insert(p, inv.rect(rect));
                    }
                }
                if crops.is_empty() {
                    this.notice = Some(super::said("crop-blank", &[]));
                } else {
                    this.crop_box = None;
                    let picked = this.picked.clone();
                    this.change_pages(Pages::Crops(crops), picked, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The bar over the tools while cropping: which pages, Cut white
    /// margins, Whole page and Crop.
    pub(super) fn crop_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.cropping || !matches!(self.content, Content::Ready(_)) {
            return None;
        }
        let th = self.theme;
        let mut seg = float::seg(&th);
        for (i, (which, id)) in WHICH.into_iter().enumerate() {
            seg = seg.child(
                float::seg_item(
                    ("crop-which", i),
                    None,
                    Some(SharedString::from(t(id, &[]))),
                    self.crop_which == which,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.crop_which = which;
                    cx.notify();
                })),
            );
        }
        let quiet = |id: &'static str, words: String| {
            float::button(id, words, false, &th)
                .border_color(gpui::transparent_black())
                .bg(gpui::transparent_black())
        };
        let has_box = self.crop_box.is_some();
        let hint = if has_box {
            "crop-hint-box"
        } else {
            "crop-hint"
        };
        Some(
            div()
                .id("crop-bar")
                .occlude()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .p(px(space::S1))
                .pl(px(space::S3))
                .rounded(px(10.0))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .shadow(th.lifted())
                .max_w_full()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_none()
                        .text_size(px(katna_ui::tokens::text::CAPTION))
                        .text_color(th.dim)
                        .child(t(hint, &[])),
                )
                .child(seg)
                .child(
                    quiet(
                        "crop-white",
                        t(
                            if self.crop_finding {
                                "crop-white-finding"
                            } else {
                                "crop-white"
                            },
                            &[],
                        ),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.cut_white(cx))),
                )
                .child(
                    quiet("crop-whole", t("crop-whole", &[]))
                        .on_click(cx.listener(|this, _, _, cx| this.uncrop(cx))),
                )
                .child(
                    float::button("crop-apply", t("crop-apply", &[]), true, &th)
                        .when(!has_box, |b| b.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.crop_to_box(cx))),
                )
                .into_any_element(),
        )
    }

    /// The box on page `page`: the page outside it dimmed, its edge and
    /// its corners to drag.
    pub(super) fn crop_overlay(&self, page: usize) -> Option<AnyElement> {
        let CropBox { rect, .. } = self.crop_box.filter(|b| self.cropping && b.page == page)?;
        let shown = self.shown;
        let [x0, y0, x1, y1] = rect.map(|v| v * shown);
        let th = self.theme;
        let shade = gpui::black().opacity(0.45);
        let dim = |l: f32, t: f32, w: f32, h: f32| {
            div()
                .absolute()
                .left(desktop_px(l))
                .top(desktop_px(t))
                .w(desktop_px(w.max(0.0)))
                .h(desktop_px(h.max(0.0)))
                .bg(shade)
        };
        let corner = |[x, y]: [f32; 2]| {
            let side = 9.0;
            div()
                .absolute()
                .left(desktop_px(x) - px(side / 2.0))
                .top(desktop_px(y) - px(side / 2.0))
                .size(px(side))
                .bg(gpui::white())
                .border_1()
                .border_color(th.accent)
        };
        Some(
            div()
                .absolute()
                .inset_0()
                // Clicks and drags reach the pages under it.
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top_0()
                        .h(desktop_px(y0))
                        .bg(shade),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .top(desktop_px(y1))
                        .bg(shade),
                )
                .child(dim(0.0, y0, x0, y1 - y0))
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .left(desktop_px(x1))
                        .top(desktop_px(y0))
                        .h(desktop_px(y1 - y0))
                        .bg(shade),
                )
                .child(
                    div()
                        .absolute()
                        .left(desktop_px(x0))
                        .top(desktop_px(y0))
                        .w(desktop_px(x1 - x0))
                        .h(desktop_px(y1 - y0))
                        .border_1()
                        .border_color(th.accent),
                )
                .children(corners([x0, y0, x1, y1]).map(corner))
                .into_any_element(),
        )
    }
}

/// The corners of `rect`: top left, top right, bottom right, bottom left.
fn corners([x0, y0, x1, y1]: [f32; 4]) -> [[f32; 2]; 4] {
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn inside([x0, y0, x1, y1]: [f32; 4], [x, y]: [f32; 2]) -> bool {
    (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
}

/// Where page `page` has anything drawn, in points from its top left as
/// it shows, with a little paper around; `None` for a blank page.
fn ink_box(doc: &dyn Document, page: usize) -> Option<[f32; 4]> {
    let (w, h) = doc.page_size(page)?;
    let bitmap = doc.render(page, 1.0)?;
    let (bw, bh) = (bitmap.width as usize, bitmap.height as usize);
    let mut found: Option<[usize; 4]> = None;
    for (i, px) in bitmap.rgba.as_chunks::<4>().0.iter().enumerate() {
        if px[3] == 0 || (px[0] >= WHITE && px[1] >= WHITE && px[2] >= WHITE) {
            continue;
        }
        let (x, y) = (i % bw, i / bw);
        found = Some(match found {
            None => [x, y, x, y],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
        });
    }
    let [x0, y0, x1, y1] = found?;
    let (sx, sy) = (w / bw.max(1) as f32, h / bh.max(1) as f32);
    Some([
        (x0 as f32 * sx - PAD).max(0.0),
        (y0 as f32 * sy - PAD).max(0.0),
        ((x1 + 1) as f32 * sx + PAD).min(w),
        ((y1 + 1) as f32 * sy + PAD).min(h),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_go_round_from_the_top_left() {
        assert_eq!(
            corners([1.0, 2.0, 3.0, 4.0]),
            [[1.0, 2.0], [3.0, 2.0], [3.0, 4.0], [1.0, 4.0]]
        );
        assert!(inside([0.0, 0.0, 2.0, 2.0], [1.0, 1.0]));
        assert!(!inside([0.0, 0.0, 2.0, 2.0], [3.0, 1.0]));
    }
}
