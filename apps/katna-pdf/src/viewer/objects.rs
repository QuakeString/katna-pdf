// SPDX-License-Identifier: GPL-3.0-or-later

//! Moving, resizing and deleting the pictures and shapes on a page
//! (docs/PLAN.md §4.3, Phase 4), with Edit text on and no tool for adding
//! in hand: a click on one picks it; dragging it moves it, dragging a
//! corner resizes it, and Delete takes it out. The row of tools shows
//! where it is and how big, in points from the page's top left, to type
//! new numbers into. The changes go into the page's content
//! (`katna_pdf_cos::objects`).

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{ObjectChange, ObjectKind, PageObject};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};
use katna_ui::widgets::{self, Tip};

use super::Viewer;
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;
/// How near a corner a press grabs it, in points on the page shown.
const GRIP: f32 = 6.0;
/// Smallest an object is made, in points.
const SMALLEST: f32 = 2.0;
/// How near, in pixels on screen, an edge or middle must come to a line
/// to line up with it.
const SNAP: f32 = 6.0;
/// Objects covering more of the page than this share of it (a page's
/// background) are not picked with a click.
const BACKGROUND: f32 = 0.8;

/// The object picked: its page and place in the page's list, and the
/// inspector's fields (left, top, width, height).
pub(super) struct Picked {
    page: usize,
    index: usize,
    fields: [Entity<TextInput>; 4],
    _fields: Vec<Subscription>,
    /// How see-through it was last made here (1 when not yet).
    opacity: f32,
}

/// The picked object being dragged: by its body, or by a corner (0 top
/// left, 1 top right, 2 bottom left, 3 bottom right), from a place to a
/// place on the page shown, in points.
pub(super) struct Moving {
    grip: Option<usize>,
    from: [f32; 2],
    to: [f32; 2],
}

impl Viewer {
    /// Object `index` of page `page`, and its box on the page shown
    /// (left, top, right, bottom, in points).
    fn object_shown(&self, page: usize, index: usize) -> Option<(PageObject, [f32; 4])> {
        let object = self.runs.objects.get(&page)?.get(index)?.clone();
        let transform = self.page_transform(page)?;
        let b = katna_pdf_render::map_rect(transform, object.rect);
        Some((object, b))
    }

    fn page_transform(&self, page: usize) -> Option<[f64; 6]> {
        match &self.content {
            super::Content::Ready(opened) => opened.doc.page_transform(page),
            _ => None,
        }
    }

    /// Whether the object picked is on page `page`.
    pub(super) fn picked_object_on(&self, page: usize) -> bool {
        self.picked_object.as_ref().is_some_and(|p| p.page == page)
    }

    /// The box the picked object is dragged to, on the page shown.
    fn moved_box(&self) -> Option<[f32; 4]> {
        self.moved_snapped().map(|(b, _)| b)
    }

    /// Where the picked object is being dragged to, lined up with the
    /// page's edges and middle and the other objects' edges and middles
    /// when near them, and the lines it meets (across, then down).
    fn moved_snapped(&self) -> Option<([f32; 4], [Option<f32>; 2])> {
        let raw = self.moved_raw()?;
        let picked = self.picked_object.as_ref()?;
        let moving = self.moving.as_ref()?;
        let page = picked.page;
        // The page as shown, in points (the bounds drawn last are cleared
        // while a frame is laid out).
        let size = match &self.content {
            super::Content::Ready(opened) => opened.doc.page_size(page)?,
            _ => return None,
        };
        let size = [size.0, size.1];
        let mut xs = vec![0.0, size[0] / 2.0, size[0]];
        let mut ys = vec![0.0, size[1] / 2.0, size[1]];
        let count = self.runs.objects.get(&page).map_or(0, |o| o.len());
        for i in (0..count).filter(|&i| i != picked.index) {
            if let Some((_, [l, t, r, b])) = self.object_shown(page, i) {
                // Not a page's background, whose edges are the page's.
                if (r - l) * (b - t) > size[0] * size[1] * BACKGROUND {
                    continue;
                }
                xs.extend([l, (l + r) / 2.0, r]);
                ys.extend([t, (t + b) / 2.0, b]);
            }
        }
        Some(snap(raw, moving.grip, &xs, &ys, SNAP / self.shown))
    }

    fn moved_raw(&self) -> Option<[f32; 4]> {
        let picked = self.picked_object.as_ref()?;
        let (_, b) = self.object_shown(picked.page, picked.index)?;
        let moving = self.moving.as_ref()?;
        let (dx, dy) = (moving.to[0] - moving.from[0], moving.to[1] - moving.from[1]);
        let [mut l, mut t, mut r, mut bo] = b;
        match moving.grip {
            None => {
                l += dx;
                r += dx;
                t += dy;
                bo += dy;
            }
            Some(0) => {
                l = (l + dx).min(r - SMALLEST);
                t = (t + dy).min(bo - SMALLEST);
            }
            Some(1) => {
                r = (r + dx).max(l + SMALLEST);
                t = (t + dy).min(bo - SMALLEST);
            }
            Some(2) => {
                l = (l + dx).min(r - SMALLEST);
                bo = (bo + dy).max(t + SMALLEST);
            }
            Some(_) => {
                r = (r + dx).max(l + SMALLEST);
                bo = (bo + dy).max(t + SMALLEST);
            }
        }
        Some([l, t, r, bo])
    }

    /// A press on the pages with Edit text on and no tool for adding in
    /// hand: grabs the picked object's corner or body, or picks the
    /// object under the pointer. Returns whether it did.
    pub(super) fn object_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.text_editing
            || self.adding.is_some()
            || self.redacting.is_some()
            || self.designing.is_some()
        {
            return false;
        }
        let Some((page, point)) = self.page_point(position) else {
            return false;
        };
        let inside = |b: [f32; 4], slack: f32| {
            point[0] >= b[0] - slack
                && point[0] <= b[2] + slack
                && point[1] >= b[1] - slack
                && point[1] <= b[3] + slack
        };
        // The picked one's corners, then its body.
        if let Some(picked) = self.picked_object.as_ref().filter(|p| p.page == page)
            && let Some((_, b)) = self.object_shown(page, picked.index)
        {
            let grip = Self::scale_grip(self.shown);
            let corners = [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]];
            let corner = corners
                .iter()
                .position(|c| (c[0] - point[0]).abs() <= grip && (c[1] - point[1]).abs() <= grip);
            if corner.is_some() || inside(b, 0.0) {
                self.moving = Some(Moving {
                    grip: corner,
                    from: point,
                    to: point,
                });
                cx.notify();
                return true;
            }
        }
        // The smallest object under the pointer, the last drawn of those
        // as big; not the page's background.
        let Some(objects) = self.runs.objects.get(&page).cloned() else {
            return false;
        };
        let Some(transform) = self.page_transform(page) else {
            return false;
        };
        let page_area = {
            let (w, h) = self.page_size(page);
            w * h
        };
        let hit = objects
            .iter()
            .enumerate()
            .filter_map(|(i, o)| {
                let b = katna_pdf_render::map_rect(transform, o.rect);
                let area = (b[2] - b[0]) * (b[3] - b[1]);
                // Thin lines are picked from a little way off.
                let slack = if (b[2] - b[0]).min(b[3] - b[1]) < GRIP {
                    GRIP / 2.0
                } else {
                    0.0
                };
                (inside(b, slack) && area < BACKGROUND * page_area).then_some((i, area))
            })
            .min_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.0.cmp(&a.0))
            })
            .map(|(i, _)| i);
        match hit {
            Some(index) => {
                self.pick_object(page, index, cx);
                self.moving = Some(Moving {
                    grip: None,
                    from: point,
                    to: point,
                });
                cx.notify();
                true
            }
            None => {
                let had = self.picked_object.take().is_some();
                if had {
                    cx.notify();
                }
                false
            }
        }
    }

    fn scale_grip(shown: f32) -> f32 {
        // As big on screen at any zoom.
        GRIP / shown.max(0.1)
    }

    fn page_size(&self, page: usize) -> (f32, f32) {
        match &self.content {
            super::Content::Ready(opened) => {
                let (w, h) = opened.doc.page_size(page).unwrap_or((612.0, 792.0));
                (w, h)
            }
            _ => (612.0, 792.0),
        }
    }

    /// Picks object `index` of page `page`, its numbers in the inspector.
    fn pick_object(&mut self, page: usize, index: usize, cx: &mut Context<Self>) {
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let fields = [0, 1, 2, 3].map(|_| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_accent(accent);
                input
            })
        });
        let subs = fields
            .iter()
            .map(|f| {
                cx.subscribe(f, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.apply_inspector(cx);
                    }
                })
            })
            .collect();
        self.picked_object = Some(Picked {
            page,
            index,
            fields,
            _fields: subs,
            opacity: 1.0,
        });
        self.refresh_inspector(cx);
    }

    /// Puts the picked object's place and size in the inspector.
    pub(super) fn refresh_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let Some((_, b)) = self.object_shown(picked.page, picked.index) else {
            return;
        };
        let values = [b[0], b[1], b[2] - b[0], b[3] - b[1]];
        for (field, value) in picked.fields.iter().zip(values) {
            let shown = format!("{:.1}", value);
            field.update(cx, |f, cx| f.set_text(shown, cx));
        }
    }

    /// The inspector's numbers typed: the picked object goes there.
    fn apply_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let values: Option<Vec<f32>> = picked
            .fields
            .iter()
            .map(|f| {
                f.read(cx)
                    .text()
                    .trim()
                    .replace(',', ".")
                    .parse::<f32>()
                    .ok()
            })
            .collect();
        let Some(&[l, t, w, h]) = values.as_deref() else {
            return;
        };
        if !(w >= SMALLEST && h >= SMALLEST) {
            return;
        }
        let (page, index) = (picked.page, picked.index);
        self.place_object(page, index, [l, t, l + w, t + h], cx);
    }

    pub(super) fn object_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self
            .moving
            .as_ref()
            .and(self.picked_object.as_ref())
            .map(|p| p.page)
        else {
            return false;
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            (position.x - b.left()) / shown,
            (position.y - b.top()) / shown,
        ];
        if let Some(moving) = self.moving.as_mut() {
            moving.to = point;
        }
        cx.notify();
        true
    }

    /// The pointer let go: the picked object goes where it was dragged.
    pub(super) fn object_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(new) = self.moved_box() else {
            return self.moving.take().is_some();
        };
        let Some(moving) = self.moving.take() else {
            return false;
        };
        cx.notify();
        let dragged = (moving.to[0] - moving.from[0]).hypot(moving.to[1] - moving.from[1]);
        if dragged < CLICK {
            return true;
        }
        if let Some(picked) = self.picked_object.as_ref() {
            let (page, index) = (picked.page, picked.index);
            self.place_object(page, index, new, cx);
        }
        true
    }

    /// Moves and sizes object `index` of page `page` to fill `shown` (a
    /// box on the page shown).
    fn place_object(&mut self, page: usize, index: usize, shown: [f32; 4], cx: &mut Context<Self>) {
        let (Some((object, _)), Some(inv)) = (self.object_shown(page, index), self.inverse(page))
        else {
            return;
        };
        let [l, b, r, t] = object.rect;
        let [nl, nb, nr, nt] = inv.rect(shown);
        // Flat things (a line across or down) keep their flat side.
        let sx = if r - l > 1e-3 {
            (nr - nl) / (r - l)
        } else {
            1.0
        };
        let sy = if t - b > 1e-3 {
            (nt - nb) / (t - b)
        } else {
            1.0
        };
        let matrix = [sx, 0.0, 0.0, sy, nl - l * sx, nb - b * sy];
        self.change_page(
            page,
            move |doc| {
                doc.transform_object(page, index, matrix)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Changes the picked object as `change` says; one moved to the
    /// front or back stays picked where it now is in the page's list.
    fn change_picked(&mut self, change: ObjectChange, cx: &mut Context<Self>) {
        let Some(page) = self.picked_object.as_ref().map(|p| p.page) else {
            return;
        };
        let count = self.runs.objects.get(&page).map_or(0, |o| o.len());
        let Some(picked) = self.picked_object.as_mut() else {
            return;
        };
        let index = picked.index;
        match change {
            ObjectChange::Front => picked.index = count.saturating_sub(1),
            ObjectChange::Back => picked.index = 0,
            ObjectChange::Opacity(o) => picked.opacity = o,
            _ => {}
        }
        self.change_page(
            page,
            move |doc| {
                doc.change_object(page, index, &change)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// The picked object, when a shape, filled and drawn in `ink`.
    pub(super) fn colour_picked(&mut self, ink: [f32; 3], cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        if self
            .object_shown(picked.page, picked.index)
            .is_some_and(|(o, _)| o.kind == ObjectKind::Path)
        {
            let change = ObjectChange::Colours {
                fill: Some(ink),
                stroke: Some(ink),
            };
            self.change_picked(change, cx);
        }
    }

    /// Turns the picked object a quarter turn right about its middle.
    fn rotate_picked(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let (page, index) = (picked.page, picked.index);
        let Some((object, _)) = self.object_shown(page, index) else {
            return;
        };
        let [l, b, r, t] = object.rect;
        let (mx, my) = ((l + r) / 2.0, (b + t) / 2.0);
        let matrix = [0.0, -1.0, 1.0, 0.0, mx - my, mx + my];
        self.change_page(
            page,
            move |doc| {
                doc.transform_object(page, index, matrix)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Asks for a picture file to show in the picked picture's place.
    fn replace_picked(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let (page, index) = (picked.page, picked.index);
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("object-replace-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                this.change_page(
                    page,
                    move |doc| {
                        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                        let image = katna_pdf_sign::page_image(&bytes)?;
                        doc.change_object(page, index, &ObjectChange::Picture(image))
                            .map_err(|e| e.to_string())
                    },
                    cx,
                )
            })
            .ok();
        })
        .detach();
    }

    /// Delete with an object picked: it is taken out. Returns whether one
    /// was.
    pub(super) fn delete_picked_object(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(picked) = self.picked_object.take() else {
            return false;
        };
        let (page, index) = (picked.page, picked.index);
        self.moving = None;
        self.change_page(
            page,
            move |doc| doc.delete_object(page, index).map_err(|e| e.to_string()),
            cx,
        );
        true
    }

    /// The picked object's frame and corners on page `page`, where it is
    /// being dragged to while it is.
    pub(super) fn objects_overlay(&self, page: usize) -> Vec<AnyElement> {
        let Some(picked) = self.picked_object.as_ref().filter(|p| p.page == page) else {
            return Vec::new();
        };
        let moved = self.moved_snapped();
        let Some(b) = moved
            .map(|(b, _)| b)
            .or_else(|| self.object_shown(page, picked.index).map(|(_, b)| b))
        else {
            return Vec::new();
        };
        let shown = self.shown;
        let accent = gpui::rgba(self.theme.katna.accent);
        let paper = self.paper();
        let mut out = Vec::new();
        // The lines it lines up with, across the whole page.
        if let Some((_, [gx, gy])) = moved {
            let guide = gpui::rgba(0xe0_30_80_ff);
            if let Some(x) = gx {
                out.push(
                    div()
                        .absolute()
                        .left(desktop_px(x * shown))
                        .top_0()
                        .bottom_0()
                        .w(desktop_px(1.0))
                        .bg(guide)
                        .into_any_element(),
                );
            }
            if let Some(y) = gy {
                out.push(
                    div()
                        .absolute()
                        .top(desktop_px(y * shown))
                        .left_0()
                        .right_0()
                        .h(desktop_px(1.0))
                        .bg(guide)
                        .into_any_element(),
                );
            }
        }
        out.extend([div()
            .absolute()
            .left(desktop_px(b[0] * shown - 1.0))
            .top(desktop_px(b[1] * shown - 1.0))
            .w(desktop_px((b[2] - b[0]) * shown + 2.0))
            .h(desktop_px((b[3] - b[1]) * shown + 2.0))
            .border_1()
            .border_color(accent)
            .cursor_move()
            .into_any_element()]);
        let size = 8.0;
        for (i, [x, y]) in [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]]
            .into_iter()
            .enumerate()
        {
            let cursor = if i == 0 || i == 3 {
                gpui::CursorStyle::ResizeUpLeftDownRight
            } else {
                gpui::CursorStyle::ResizeUpRightDownLeft
            };
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(x * shown - size / 2.0))
                    .top(desktop_px(y * shown - size / 2.0))
                    .size(desktop_px(size))
                    .bg(paper)
                    .border_1()
                    .border_color(accent)
                    .cursor(cursor)
                    .into_any_element(),
            );
        }
        out
    }

    /// The inspector, in the row of tools while an object is picked: what
    /// it is, where, how big, and a button to delete it.
    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picked = self.picked_object.as_ref()?;
        let (object, _) = self.object_shown(picked.page, picked.index)?;
        let th = self.theme;
        let mut row = div().flex().items_center().gap(px(space::S1));
        for (field, label) in picked.fields.iter().zip(["X", "Y", "W", "H"]) {
            row = row.child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(gpui::rgba(th.katna.text_dim))
                    .child(label),
            );
            row = row.child(
                div()
                    .w(px(44.0))
                    .h(px(28.0))
                    .px(px(space::S1))
                    .flex()
                    .items_center()
                    .rounded(px(katna_ui::tokens::radius::XS))
                    .bg(th.field)
                    .text_size(px(text::SMALL))
                    .child(field.clone()),
            );
        }
        let tool = |id: &'static str, icon: &'static str, tip: String| {
            widgets::icon_button(id, icon, 18.0, &th.katna).tip(tip, &th.katna)
        };
        row = row
            .child(
                tool("object-rotate", "rotate-cw", t("object-rotate", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.rotate_picked(cx))),
            )
            .child(
                tool("object-front", "to-front", t("object-front", &[])).on_click(
                    cx.listener(|this, _, _, cx| this.change_picked(ObjectChange::Front, cx)),
                ),
            )
            .child(
                tool("object-back", "to-back", t("object-back", &[])).on_click(
                    cx.listener(|this, _, _, cx| this.change_picked(ObjectChange::Back, cx)),
                ),
            );
        let percent = (picked.opacity * 100.0).round() as u32;
        // 100, 75, 50, 25, then fully again.
        let next = if picked.opacity <= 0.3 {
            1.0
        } else {
            picked.opacity - 0.25
        };
        row = row.child(
            tool(
                "object-opacity",
                "contrast",
                t("object-opacity", &[("percent", &percent.to_string())]),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.change_picked(ObjectChange::Opacity(next), cx)
            })),
        );
        if object.kind == ObjectKind::Picture {
            row = row.child(
                tool("object-replace", "image", t("object-replace", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.replace_picked(cx))),
            );
        }
        row = row.child(
            widgets::icon_button("object-delete", "trash", 18.0, &th.katna)
                .tip(t("object-delete", &[]), &th.katna)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.delete_picked_object(cx);
                })),
        );
        Some(row.into_any_element())
    }
}

/// The nearest of `lines` to any of `at`, within `reach`: how far to go
/// to it, and the line.
fn nearest(at: &[f32], lines: &[f32], reach: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &a in at {
        for &line in lines {
            let d = line - a;
            if d.abs() <= reach && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, line));
            }
        }
    }
    best
}

/// Box `b` (left, top, right, bottom) moved, or resized by corner
/// `grip`, so that what moves meets the nearest of the lines down (`xs`)
/// and across (`ys`) within `reach`; with the lines met.
fn snap(
    b: [f32; 4],
    grip: Option<usize>,
    xs: &[f32],
    ys: &[f32],
    reach: f32,
) -> ([f32; 4], [Option<f32>; 2]) {
    let [mut l, mut t, mut r, mut bo] = b;
    match grip {
        None => {
            let x = nearest(&[l, (l + r) / 2.0, r], xs, reach);
            let y = nearest(&[t, (t + bo) / 2.0, bo], ys, reach);
            let (dx, dy) = (x.map_or(0.0, |x| x.0), y.map_or(0.0, |y| y.0));
            (
                [l + dx, t + dy, r + dx, bo + dy],
                [x.map(|x| x.1), y.map(|y| y.1)],
            )
        }
        Some(grip) => {
            // The corner held: 0 top left, 1 top right, 2 bottom left,
            // 3 bottom right.
            let left = grip == 0 || grip == 2;
            let top = grip < 2;
            let x = nearest(&[if left { l } else { r }], xs, reach).filter(|&(_, line)| {
                if left {
                    line < r - SMALLEST
                } else {
                    line > l + SMALLEST
                }
            });
            let y = nearest(&[if top { t } else { bo }], ys, reach).filter(|&(_, line)| {
                if top {
                    line < bo - SMALLEST
                } else {
                    line > t + SMALLEST
                }
            });
            if let Some((_, line)) = x {
                if left { l = line } else { r = line }
            }
            if let Some((_, line)) = y {
                if top { t = line } else { bo = line }
            }
            ([l, t, r, bo], [x.map(|x| x.1), y.map(|y| y.1)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn things_line_up_when_near() {
        let xs = [0.0, 300.0, 600.0];
        let ys = [0.0, 400.0, 800.0];
        // Moved: its middle meets the page's middle; nothing near across.
        let (b, lines) = snap([196.0, 100.0, 398.0, 150.0], None, &xs, &ys, 5.0);
        assert_eq!(b, [199.0, 100.0, 401.0, 150.0]);
        assert_eq!(lines, [Some(300.0), None]);
        // Its left edge meets the page's.
        let (b, lines) = snap([3.0, 397.0, 53.0, 447.0], None, &xs, &ys, 5.0);
        assert_eq!(b, [0.0, 400.0, 50.0, 450.0]);
        assert_eq!(lines, [Some(0.0), Some(400.0)]);
        // Resized by the bottom right corner: that corner only.
        let (b, lines) = snap([10.0, 10.0, 597.0, 120.0], Some(3), &xs, &ys, 5.0);
        assert_eq!(b, [10.0, 10.0, 600.0, 120.0]);
        assert_eq!(lines, [Some(600.0), None]);
        // Too far: as it was.
        let (b, lines) = snap([20.0, 20.0, 40.0, 40.0], None, &xs, &ys, 5.0);
        assert_eq!(b, [20.0, 20.0, 40.0, 40.0]);
        assert_eq!(lines, [None, None]);
    }
}
