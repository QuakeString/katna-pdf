// SPDX-License-Identifier: GPL-3.0-or-later

//! Rulers and guides while editing, as Acrobat and Master PDF Editor
//! have them: rulers along each page's top and left edges in the units
//! chosen in Settings; a guide is dragged out of a ruler, moved by
//! dragging it, and taken away by dragging it back onto its ruler (or
//! off the page) or by double-clicking it. Objects being moved or
//! resized snap to the guides. They are the same on every page of the
//! document and are not written into the file.

use gpui::{AnyElement, Context, div, prelude::*};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::tokens::text;

use super::Viewer;

/// A ruler's thickness on screen: the top one, and the left one, a
/// little wider for its numbers.
const RULER: f32 = 18.0;
const RULER_LEFT: f32 = 26.0;

/// How far either side of a guide a press takes it, on screen.
const HOLD: f32 = 4.0;

/// The colour guides are drawn in (as Acrobat's: cyan).
const GUIDE: u32 = 0x00_a8_e0_ff;

/// A guide: a line across the page (at `at` points from its top) or
/// down it (from its left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Guide {
    pub across: bool,
    pub at: f32,
}

/// A guide being dragged: which, and on which page's rulers.
pub(super) struct GuideDrag {
    index: usize,
    page: usize,
    /// Whether it is off the page (onto its ruler or past it), to be
    /// taken away when let go.
    out: bool,
}

/// The rulers shown or not, and the guides.
#[derive(Default)]
pub(super) struct Guides {
    pub on: bool,
    pub lines: Vec<Guide>,
    dragging: Option<GuideDrag>,
}

impl Guides {
    /// The guides down the page (`xs`) and across it (`ys`), in points,
    /// to snap to; none while the rulers are hidden.
    pub(super) fn snap_lines(&self) -> (Vec<f32>, Vec<f32>) {
        if !self.on {
            return (Vec::new(), Vec::new());
        }
        let pick = |across: bool| {
            self.lines
                .iter()
                .filter(|g| g.across == across)
                .map(|g| g.at)
                .collect()
        };
        (pick(false), pick(true))
    }
}

/// The steps a ruler is marked in, in points: numbered ones about 24
/// to 160 pixels apart at zoom `shown`, and the small marks between
/// them. `unit` is how many points a unit is.
fn steps(unit: f32, shown: f32) -> (f32, f32) {
    // Ten millimetres, an inch or 72 points, then more as it shrinks
    // and fewer, down to a millimetre, a quarter inch or 9 points, as it
    // grows.
    let mm = 72.0 / 25.4;
    let (mut major, least) = if (unit - mm).abs() < 0.01 {
        (10.0 * mm, mm)
    } else if unit >= 72.0 {
        (unit, unit / 4.0)
    } else {
        (72.0, 9.0)
    };
    while major * shown < 24.0 {
        major *= 2.0;
    }
    while major * shown > 160.0 && major / 2.0 >= least {
        major /= 2.0;
    }
    (major, major / 4.0)
}

/// A ruler's number: how many units `points` is, as short as it reads.
fn label(points: f32, unit: f32) -> String {
    let n = points / unit;
    if (n - n.round()).abs() < 0.01 {
        format!("{}", n.round() as i64)
    } else {
        format!("{n:.1}")
    }
}

impl Viewer {
    /// Shows or hides the rulers and the guides.
    pub(super) fn toggle_guides(&mut self, cx: &mut Context<Self>) {
        self.guides.on = !self.guides.on;
        // How to use them, while there are none yet.
        if self.guides.on && self.guides.lines.is_empty() {
            self.notice = Some(super::said("guides-hint", &[]));
        }
        cx.notify();
    }

    /// Where `position` is on page `page`, in points from its top left,
    /// wherever the pointer is (on the page or not).
    fn on_page(&self, page: usize, position: gpui::Point<gpui::Pixels>) -> Option<[f32; 2]> {
        let b = *self.page_bounds.borrow().get(&page)?;
        let shown = desktop_px(self.shown);
        Some([
            (position.x - b.left()) / shown,
            (position.y - b.top()) / shown,
        ])
    }

    /// How far page `page`'s rulers are moved in from its top and left
    /// edges so they stay in sight while the page is scrolled past them,
    /// as the window's own rulers would (as laid out last frame).
    fn ruler_shift(&self, page: usize) -> (gpui::Pixels, gpui::Pixels) {
        let zero = gpui::Pixels::ZERO;
        let (Some(b), Some(view)) = (
            self.ruler_bounds.borrow().get(&page).copied(),
            self.view_bounds.get(),
        ) else {
            return (zero, zero);
        };
        let room = |gap: gpui::Pixels, long: gpui::Pixels, ruler: f32| {
            let most = long - desktop_px(ruler);
            if gap <= zero || most <= zero {
                zero
            } else if gap > most {
                most
            } else {
                gap
            }
        };
        (
            room(view.left() - b.left(), b.size.width, RULER_LEFT),
            room(view.top() - b.top(), b.size.height, RULER),
        )
    }

    /// The pointer moved with a guide held: it follows, and is marked to
    /// go when off the page.
    pub(super) fn guide_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.guides.dragging.as_ref() else {
            return false;
        };
        let (index, page) = (drag.index, drag.page);
        let Some(at) = self.on_page(page, position) else {
            return true;
        };
        let (w, h) = self.page_size(page);
        // The ruler covers the page's edge; a guide let go over it goes.
        let shown = desktop_px(self.shown);
        let (sx, sy) = self.ruler_shift(page);
        if let Some(g) = self.guides.lines.get_mut(index) {
            let (v, end) = if g.across { (at[1], h) } else { (at[0], w) };
            let edge = if g.across {
                (sy + desktop_px(RULER)) / shown
            } else {
                (sx + desktop_px(RULER_LEFT)) / shown
            };
            let out = v < edge || v > end;
            g.at = v.clamp(0.0, end);
            if let Some(d) = self.guides.dragging.as_mut() {
                d.out = out;
            }
        }
        cx.notify();
        true
    }

    /// A guide let go: kept where it is, or taken away when off the page.
    pub(super) fn guide_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.guides.dragging.take() else {
            return false;
        };
        if drag.out && drag.index < self.guides.lines.len() {
            self.guides.lines.remove(drag.index);
        }
        cx.notify();
        true
    }

    /// The rulers along page `page`'s top and left edges and the guides
    /// over it, while editing with them on.
    pub(super) fn guides_overlay(&self, page: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.guides.on || !self.text_editing {
            return Vec::new();
        }
        let th = self.theme;
        let shown = self.shown;
        let (w, h) = self.page_size(page);
        let units = cx.global::<crate::prefs::Prefs>().units;
        let unit = units.points();
        let (major, minor) = steps(unit, shown);
        let (sx, sy) = self.ruler_shift(page);
        let mut out = Vec::new();
        // The guides, under the rulers, each with room either side to
        // take hold of it.
        let colour = gpui::rgba(GUIDE);
        for (i, g) in self.guides.lines.iter().enumerate() {
            let going = self
                .guides
                .dragging
                .as_ref()
                .is_some_and(|d| d.index == i && d.out);
            let line = div()
                .bg(if going {
                    gpui::rgba(GUIDE & 0xffff_ff60)
                } else {
                    colour
                })
                .when(g.across, |d| d.w_full().h(desktop_px(1.0)))
                .when(!g.across, |d| d.h_full().w(desktop_px(1.0)));
            let held = div()
                .id(("guide", page * 1000 + i))
                .absolute()
                .flex()
                .justify_center()
                .items_center()
                .when(g.across, |d| {
                    d.left_0()
                        .right_0()
                        .top(desktop_px(g.at * shown - HOLD))
                        .h(desktop_px(HOLD * 2.0 + 1.0))
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                })
                .when(!g.across, |d| {
                    d.top_0()
                        .bottom_0()
                        .left(desktop_px(g.at * shown - HOLD))
                        .w(desktop_px(HOLD * 2.0 + 1.0))
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if event.click_count >= 2 {
                            if i < this.guides.lines.len() {
                                this.guides.lines.remove(i);
                            }
                            this.guides.dragging = None;
                        } else {
                            this.guides.dragging = Some(GuideDrag {
                                index: i,
                                page,
                                out: false,
                            });
                        }
                        cx.notify();
                    }),
                )
                .child(line);
            out.push(held.into_any_element());
        }
        // The rulers: marks every `minor`, numbers every `major`.
        let face = th.card;
        let ink = th.text2;
        let ruler = |across: bool| {
            let long = if across { w } else { h };
            let mut r = div()
                .id(if across { "ruler-top" } else { "ruler-left" })
                .absolute()
                .top(if across { sy } else { gpui::Pixels::ZERO })
                .left(if across { gpui::Pixels::ZERO } else { sx })
                .overflow_hidden()
                .bg(face)
                .border_color(th.line)
                .when(across, |d| {
                    d.w(desktop_px(long * shown))
                        .h(desktop_px(RULER))
                        .border_b_1()
                        .cursor(gpui::CursorStyle::ResizeUpDown)
                })
                .when(!across, |d| {
                    d.h(desktop_px(long * shown))
                        .w(desktop_px(RULER_LEFT))
                        .border_r_1()
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                });
            let count = (long / minor).floor() as usize;
            for k in 1..=count {
                let at = k as f32 * minor;
                let big = (at / major - (at / major).round()).abs() < 1e-3;
                let size = if big { RULER * 0.5 } else { RULER * 0.25 };
                let mark = div().absolute().bg(ink);
                let mark = if across {
                    mark.left(desktop_px(at * shown))
                        .bottom_0()
                        .w(desktop_px(1.0))
                        .h(desktop_px(size))
                } else {
                    mark.top(desktop_px(at * shown))
                        .right_0()
                        .h(desktop_px(1.0))
                        .w(desktop_px(size))
                };
                r = r.child(mark);
                if big {
                    let number = div()
                        .absolute()
                        .whitespace_nowrap()
                        .text_size(px(text::MICRO))
                        .line_height(desktop_px(RULER * 0.5))
                        .text_color(ink)
                        .child(label(at, unit));
                    r = r.child(if across {
                        number.left(desktop_px(at * shown + 2.0)).top_0()
                    } else {
                        number
                            .top(desktop_px(at * shown + 1.0))
                            .left(desktop_px(2.0))
                    });
                }
            }
            // Pressing a ruler draws a guide out of it.
            r.on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(at) = this.on_page(page, event.position) else {
                        return;
                    };
                    this.guides.lines.push(Guide {
                        across,
                        at: if across { at[1] } else { at[0] },
                    });
                    this.guides.dragging = Some(GuideDrag {
                        index: this.guides.lines.len() - 1,
                        page,
                        out: true,
                    });
                    cx.notify();
                }),
            )
            .into_any_element()
        };
        out.push(ruler(true));
        out.push(ruler(false));
        // The corner where they meet says the unit.
        out.push(
            div()
                .absolute()
                .top(sy)
                .left(sx)
                .w(desktop_px(RULER_LEFT))
                .h(desktop_px(RULER))
                .flex()
                .items_center()
                .justify_center()
                .bg(face)
                .border_b_1()
                .border_r_1()
                .border_color(th.line)
                .text_size(px(text::MICRO))
                .text_color(th.dim)
                .child(units.suffix())
                .into_any_element(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulers_are_marked_in_steps_that_read() {
        let mm = 72.0 / 25.4;
        // Centimetres at 100 %; whole inches; points by 72.
        assert_eq!(steps(mm, 1.0).0, 10.0 * mm);
        assert_eq!(steps(72.0, 1.0).0, 72.0);
        assert_eq!(steps(1.0, 1.0).0, 72.0);
        // Zoomed far out the numbers spread; zoomed in they come closer.
        assert_eq!(steps(72.0, 0.25).0, 144.0);
        assert_eq!(steps(72.0, 4.0).0, 36.0);
        assert_eq!(label(36.0, 72.0), "0.5");
        assert_eq!(label(10.0 * mm, mm), "10");
    }

    #[test]
    fn guides_snap_only_while_shown() {
        let mut g = Guides {
            lines: vec![
                Guide {
                    across: true,
                    at: 100.0,
                },
                Guide {
                    across: false,
                    at: 50.0,
                },
            ],
            ..Guides::default()
        };
        assert_eq!(g.snap_lines(), (vec![], vec![]));
        g.on = true;
        assert_eq!(g.snap_lines(), (vec![50.0], vec![100.0]));
    }
}
