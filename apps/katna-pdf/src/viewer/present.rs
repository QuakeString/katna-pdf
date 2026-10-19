// SPDX-License-Identifier: GPL-3.0-or-later

//! Presenting (docs/PLAN.md §4.1, P1): the window full screen, one page
//! at a time on black, in its own colours. Pages come on as their file
//! says (`/Trans`: wipes, pushes, covers, splits, boxes and fades) and
//! move on by themselves after the seconds it gives (`/Dur`). Keys,
//! clicks and typed page numbers move through them; B and W blank the
//! screen; Esc ends.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, Window, div, img,
    prelude::*,
};
use katna_pdf_cos::{PageShow, Transition, TransitionStyle};
use katna_ui::tokens::{radius, space, text};
use katna_ui::{px, unpx};

use super::{
    Content, Present, PresentBlack, PresentEnter, PresentFirst, PresentLast, PresentNext,
    PresentPrevious, PresentStop, PresentWhite, Viewer,
};
use crate::text::t;

/// Presenting.
pub(super) struct Presenting {
    page: usize,
    /// The page going, when it started going, and how the new one comes
    /// on.
    leaving: Option<(usize, Instant, Transition)>,
    /// How each page is shown, once read.
    shows: Vec<PageShow>,
    /// The screen blanked black (`Some(false)`) or white (`Some(true)`).
    blank: Option<bool>,
    /// A page number being typed.
    typed: String,
    /// The window was full screen already.
    was_full: bool,
    /// Bumped on each change of page, so a page's timer ends with it.
    step: u64,
}

impl Viewer {
    pub fn presenting(&self) -> bool {
        self.presenting.is_some()
    }

    /// Presents from the page shown.
    pub(super) fn start_presenting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.presenting.is_some() {
            self.stop_presenting(window, cx);
            return;
        }
        if self.page_count() == 0 {
            return;
        }
        let page = self.current_page();
        let was_full = window.is_fullscreen();
        if !was_full {
            window.toggle_fullscreen();
        }
        self.presenting = Some(Presenting {
            page,
            leaving: None,
            shows: Vec::new(),
            blank: None,
            typed: String::new(),
            was_full,
            step: 0,
        });
        self.forget_far(page..page + 1);
        window.focus(&self.focus, cx);
        self.read_shows(cx);
        cx.notify();
    }

    /// Reads how the pages are shown, then starts the first one's timer.
    fn read_shows(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let shows = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path).ok()?;
                    Some(super::cos(bytes, key.as_deref()).ok()?.page_shows())
                })
                .await
                .unwrap_or_default();
            this.update(cx, |this, cx| {
                if let Some(presenting) = this.presenting.as_mut() {
                    presenting.shows = shows;
                    this.time_page(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn stop_presenting(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(presenting) = self.presenting.take() else {
            return;
        };
        if !presenting.was_full && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.go_to_page(presenting.page, cx);
        cx.notify();
    }

    /// Shows page `page`, bringing it on as the file says when it is the
    /// next or the one before.
    fn present_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let count = self.page_count();
        let Some(presenting) = self.presenting.as_mut() else {
            return;
        };
        let page = page.min(count.saturating_sub(1));
        presenting.typed.clear();
        if presenting.blank.take().is_some() && page == presenting.page {
            cx.notify();
            return;
        }
        if page == presenting.page {
            return;
        }
        let old = presenting.page;
        // A page further off may no longer be drawn: it comes on at once.
        presenting.leaving = presenting
            .shows
            .get(page)
            .and_then(|s| s.transition)
            .filter(|t| {
                t.style != TransitionStyle::Replace && t.seconds > 0.0 && old.abs_diff(page) == 1
            })
            .map(|t| (old, Instant::now(), t));
        presenting.page = page;
        presenting.step += 1;
        self.forget_far(page..page + 1);
        self.time_page(cx);
        cx.notify();
    }

    /// Moves on from the page shown after the seconds its file gives.
    fn time_page(&mut self, cx: &mut Context<Self>) {
        let Some(presenting) = self.presenting.as_ref() else {
            return;
        };
        let Some(seconds) = presenting
            .shows
            .get(presenting.page)
            .and_then(|s| s.seconds)
        else {
            return;
        };
        let coming = presenting.leaving.map_or(0.0, |(_, _, t)| t.seconds);
        let step = presenting.step;
        let wait = Duration::from_secs_f32(seconds + coming);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.presenting.as_ref()
                    && p.step == step
                    && p.blank.is_none()
                    && p.page + 1 < this.page_count()
                {
                    let next = p.page + 1;
                    this.present_page(next, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn present_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(page) = self.presenting.as_ref().map(|p| p.page) else {
            return;
        };
        let to = if forward {
            page + 1
        } else {
            page.saturating_sub(1)
        };
        self.present_page(to, cx);
    }

    fn present_blank(&mut self, white: bool, cx: &mut Context<Self>) {
        if let Some(presenting) = self.presenting.as_mut() {
            presenting.blank = match presenting.blank {
                Some(w) if w == white => None,
                _ => Some(white),
            };
            cx.notify();
        }
    }

    /// Enter: the page typed, or the next.
    fn present_enter(&mut self, cx: &mut Context<Self>) {
        let Some(presenting) = self.presenting.as_mut() else {
            return;
        };
        match std::mem::take(&mut presenting.typed).parse::<usize>() {
            Ok(n) if n > 0 => self.present_page(n - 1, cx),
            _ => self.present_step(true, cx),
        }
    }

    fn present_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(presenting) = self.presenting.as_mut() else {
            return;
        };
        let key = &event.keystroke.key;
        if key.len() == 1 && key.chars().all(|c| c.is_ascii_digit()) && presenting.typed.len() < 6 {
            presenting.typed.push_str(key);
            cx.notify();
        }
    }

    /// The window while presenting.
    pub(super) fn render_presenting(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Content::Ready(opened) = &self.content else {
            return div().into_any_element();
        };
        let doc = opened.doc.clone();
        let Some(presenting) = self.presenting.as_ref() else {
            return div().into_any_element();
        };
        let (page, blank, typed) = (presenting.page, presenting.blank, presenting.typed.clone());
        let going = presenting.leaving;
        let viewport = window.viewport_size();
        let (vw, vh) = (unpx(viewport.width), unpx(viewport.height));
        let (pw, ph) = doc.page_size(page).unwrap_or((612.0, 792.0));
        let fit = (vw / pw).min(vh / ph);
        let (w, h) = (pw * fit, ph * fit);
        let device = fit * katna_ui::scale::scale() * window.scale_factor();
        self.want(page, device, cx);
        if page + 1 < self.page_count() {
            self.want(page + 1, device, cx);
        }
        let image = |this: &Self, p: usize| {
            this.pages
                .get(&p)
                .map(|d| img(ImageSource::Render(d.image.clone())).size_full())
        };
        // The page coming on, how far along, and the one going.
        let mut leaving = None;
        if let Some((old, started, transition)) = going {
            let done = started.elapsed().as_secs_f32() / transition.seconds.max(0.01);
            if done < 1.0 && self.pages.contains_key(&old) {
                leaving = Some((old, done, transition));
                window.request_animation_frame();
            }
        }
        if leaving.is_none()
            && let Some(p) = self.presenting.as_mut()
        {
            p.leaving = None;
        }
        let layer = |x: f32, y: f32| div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h));
        let new = layer(0.0, 0.0)
            .bg(gpui::white())
            .children(image(self, page));
        let stage = match leaving {
            None => div().size_full().child(new).into_any_element(),
            Some((old, k, transition)) => {
                let old = layer(0.0, 0.0).bg(gpui::white()).children(image(self, old));
                transitioned(old, k, transition, (w, h), |x, y| {
                    layer(x, y).bg(gpui::white()).children(image(self, page))
                })
            }
        };
        div()
            .id("katna-pdf-presenting")
            .track_focus(&self.focus)
            .key_context("Presenting")
            .on_action(cx.listener(|this, _: &PresentNext, _, cx| this.present_step(true, cx)))
            .on_action(cx.listener(|this, _: &PresentPrevious, _, cx| {
                this.present_step(false, cx);
            }))
            .on_action(cx.listener(|this, _: &PresentFirst, _, cx| this.present_page(0, cx)))
            .on_action(cx.listener(|this, _: &PresentLast, _, cx| {
                this.present_page(usize::MAX, cx);
            }))
            .on_action(cx.listener(|this, _: &PresentEnter, _, cx| this.present_enter(cx)))
            .on_action(cx.listener(|this, _: &PresentBlack, _, cx| this.present_blank(false, cx)))
            .on_action(cx.listener(|this, _: &PresentWhite, _, cx| this.present_blank(true, cx)))
            .on_action(cx.listener(|this, _: &Present, window, cx| {
                this.stop_presenting(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PresentStop, window, cx| {
                this.stop_presenting(window, cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.present_key(event, cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.present_step(true, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.present_step(false, cx)),
            )
            .size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black())
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .overflow_hidden()
                    .child(stage),
            )
            .children(blank.map(|white| {
                div()
                    .absolute()
                    .inset_0()
                    .bg(if white { gpui::white() } else { gpui::black() })
            }))
            .when(!typed.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(space::S6))
                        .bottom(px(space::S6))
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .rounded(px(radius::SM))
                        .bg(gpui::rgba(0x000000cc))
                        .text_color(gpui::white())
                        .text_size(px(text::TITLE))
                        .child(t("present-go-to", &[("page", &typed)])),
                )
            })
            .into_any_element()
    }
}

/// The old page and the new one `k` of the way through `transition`,
/// on a stage `size` big; `new_at` places the new page.
fn transitioned(
    old: gpui::Div,
    k: f32,
    transition: Transition,
    (w, h): (f32, f32),
    new_at: impl Fn(f32, f32) -> gpui::Div,
) -> AnyElement {
    // Where things move: a unit step in the stage's x and y.
    let angle = transition.direction.unwrap_or(0.0).to_radians();
    let (dx, dy) = (angle.cos().round(), -angle.sin().round());
    let stage = div().size_full().relative();
    // The part of the stage a page shows in, the page in it unmoved.
    let clip = |x: f32, y: f32, cw: f32, ch: f32, page: gpui::Div| {
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(cw.max(0.0)))
            .h(px(ch.max(0.0)))
            .overflow_hidden()
            .child(page)
    };
    let shifted = |x: f32, y: f32| new_at(-x, -y);
    match transition.style {
        TransitionStyle::Push => stage
            .child(old_at(old, dx * k * w, dy * k * h))
            .child(new_at((k - 1.0) * dx * w, (k - 1.0) * dy * h)),
        TransitionStyle::Cover | TransitionStyle::Fly => stage
            .child(old)
            .child(new_at((k - 1.0) * dx * w, (k - 1.0) * dy * h)),
        TransitionStyle::Uncover => {
            stage
                .child(new_at(0.0, 0.0))
                .child(old_at(old, dx * k * w, dy * k * h))
        }
        TransitionStyle::Wipe => {
            let (x, y, cw, ch) = match (dx as i32, dy as i32) {
                (-1, _) => ((1.0 - k) * w, 0.0, k * w, h),
                (_, 1) => (0.0, 0.0, w, k * h),
                (_, -1) => (0.0, (1.0 - k) * h, w, k * h),
                _ => (0.0, 0.0, k * w, h),
            };
            stage.child(old).child(clip(x, y, cw, ch, shifted(x, y)))
        }
        TransitionStyle::Split | TransitionStyle::Box => {
            let boxed = transition.style == TransitionStyle::Box;
            // The middle part, growing out or shrinking in.
            let m = if transition.outward { k } else { 1.0 - k };
            let (cw, ch) = match (boxed, transition.horizontal) {
                (true, _) => (m * w, m * h),
                (false, true) => (w, m * h),
                (false, false) => (m * w, h),
            };
            let (x, y) = ((w - cw) / 2.0, (h - ch) / 2.0);
            if transition.outward {
                stage.child(old).child(clip(x, y, cw, ch, shifted(x, y)))
            } else {
                // Coming in from the edges: the old page's middle shrinks.
                let old_part = clip(x, y, cw, ch, old_at(old, -x, -y));
                stage.child(new_at(0.0, 0.0)).child(old_part)
            }
        }
        // Fades, and the rest, which come on as a fade.
        _ => stage.child(old).child(new_at(0.0, 0.0).opacity(k)),
    }
    .into_any_element()
}

/// The old page moved by `x`, `y`.
fn old_at(old: gpui::Div, x: f32, y: f32) -> gpui::Div {
    old.left(px(x)).top(px(y))
}
