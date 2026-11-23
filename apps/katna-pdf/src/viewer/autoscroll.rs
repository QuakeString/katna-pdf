// SPDX-License-Identifier: GPL-3.0-or-later

//! Scrolling by itself (Ctrl+Shift+H): the pages move up steadily, ↓ and
//! ↑ make it faster or slower, Escape or the same keys again stop it, and
//! it stops at the end.

use std::time::Duration;

use gpui::{Context, Task};
use katna_ui::scale::desktop_px;

use super::{Content, Viewer, said};

/// How fast it goes at first, of `SPEEDS`.
const START: usize = 2;

/// Points of page scrolled each second, slowest to fastest.
const SPEEDS: [f32; 9] = [15.0, 25.0, 40.0, 60.0, 90.0, 130.0, 180.0, 250.0, 350.0];

/// Each step, about sixty a second.
const FRAME: Duration = Duration::from_millis(16);

/// Scrolling by itself, while it goes.
pub(super) struct AutoScroll {
    speed: usize,
    _task: Task<()>,
}

impl Viewer {
    /// Starts scrolling by itself, or stops it.
    pub(super) fn toggle_auto_scroll(&mut self, cx: &mut Context<Self>) {
        if self.auto_scroll.take().is_some() {
            self.notice = Some(said("autoscroll-stopped", &[]));
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            let mut still = 0;
            loop {
                cx.background_executor().timer(FRAME).await;
                let going = this.update(cx, |this, cx| {
                    let Some(auto) = this.auto_scroll.as_ref() else {
                        return false;
                    };
                    let step = SPEEDS[auto.speed] * this.shown * FRAME.as_secs_f32();
                    let at = |list: &gpui::ListState| {
                        let top = list.logical_scroll_top();
                        (top.item_ix, top.offset_in_item)
                    };
                    let before = at(&this.list);
                    this.list.scroll_by(desktop_px(step));
                    // Stopped by the end of the pages for a few steps:
                    // done.
                    still = if at(&this.list) == before {
                        still + 1
                    } else {
                        0
                    };
                    cx.notify();
                    still < 10
                });
                match going {
                    Ok(true) => {}
                    Ok(false) => {
                        this.update(cx, |this, cx| {
                            if this.auto_scroll.take().is_some() {
                                this.notice = Some(said("autoscroll-end", &[]));
                            }
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    Err(_) => break,
                }
            }
        });
        self.auto_scroll = Some(AutoScroll {
            speed: START,
            _task: task,
        });
        self.notice = Some(said("autoscroll-on", &[]));
        cx.notify();
    }

    /// Faster (`by` 1) or slower (-1) while scrolling by itself; whether
    /// it was.
    pub(super) fn auto_scroll_speed(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        let Some(auto) = self.auto_scroll.as_mut() else {
            return false;
        };
        auto.speed = auto.speed.saturating_add_signed(by).min(SPEEDS.len() - 1);
        let level = (auto.speed + 1).to_string();
        let most = SPEEDS.len().to_string();
        self.notice = Some(said(
            "autoscroll-speed",
            &[("level", &level), ("most", &most)],
        ));
        cx.notify();
        true
    }
}
