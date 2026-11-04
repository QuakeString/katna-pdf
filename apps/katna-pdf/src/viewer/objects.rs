// SPDX-License-Identifier: GPL-3.0-or-later

//! Picking, moving, resizing, turning, lining up, copying and deleting
//! the pictures and shapes on a page (docs/PLAN.md §4.3, Phase 4), with
//! Edit text on and no tool for adding in hand: a click on one picks it,
//! Ctrl and a click picks more, and a drag over empty page picks all it
//! runs over; dragging them moves them, dragging a corner resizes them,
//! dragging the handle above turns them, the arrow keys nudge them, and
//! Delete takes them out. The row of tools shows where they are and how
//! big, in points from the page's top left, to type new numbers into
//! (with units and sums: `2cm+5`), and lines them up with each other or
//! the page, steps them forward and back, flips them, and fits them to
//! the page. Ctrl+C, Ctrl+X and Ctrl+V copy, cut and paste them, on any
//! page of any open file. A shape picked alone shows whether it is
//! filled and lined, how wide its line is and whether it is dashed, to
//! change, and its corners to drag one at a time; a picture can be
//! saved as a file, and drawn whole again when a clipping path cuts it
//! down. The changes go into the page's content
//! (`katna_pdf_cos::objects`).

use std::path::Path;
use std::sync::Mutex;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{ObjectChange, ObjectCopy, ObjectKind, PageObject, PictureFile, ShapeStyle};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};
use katna_ui::widgets::{self, Tip};

use super::{Viewer, separator};
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;
/// How near a corner a press grabs it, in points on the page shown.
const GRIP: f32 = 6.0;
/// How far above the frame's top the turning handle sits, in points on
/// the page shown at 100%.
const TURN_ABOVE: f32 = 22.0;
/// Smallest an object is made, in points.
const SMALLEST: f32 = 2.0;
/// How near, in pixels on screen, an edge or middle must come to a line
/// to line up with it.
const SNAP: f32 = 6.0;
/// Objects covering more of the page than this share of it (a page's
/// background) are not picked with a click.
const BACKGROUND: f32 = 0.8;
/// The grid's lines are this far apart, in points (a quarter inch).
pub(super) const GRID: f32 = 18.0;
/// How far a paste on the same page is moved along, in points, so the
/// copy is not hidden under the original.
const ASIDE: f32 = 12.0;
/// What the system clipboard says while objects copied here are what
/// Ctrl+V should paste.
const COPIED_MARK: &str = "katna-pdf: objects copied";

/// Objects copied, for any open file to paste.
static COPIED: Mutex<Vec<ObjectCopy>> = Mutex::new(Vec::new());

/// The objects picked: their page and places in the page's list (the
/// last picked last), and the inspector's fields (left, top, width,
/// height, turn, line width).
pub(super) struct Picked {
    page: usize,
    indices: Vec<usize>,
    fields: [Entity<TextInput>; 6],
    _fields: Vec<Subscription>,
    /// How see-through they were last made here (1 when not yet).
    opacity: f32,
    /// Whether a shape's own points are shown, to drag one at a time.
    points: bool,
}

/// What a drag on the picked objects holds: their body, a corner (0 top
/// left, 1 top right, 2 bottom left, 3 bottom right), the turning
/// handle, or one of a shape's own points.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Grip {
    Body,
    Corner(usize),
    Turn,
    Point(usize),
}

/// The picked objects being dragged, from a place to a place on the page
/// shown, in points; `shift` while turning keeps to eighths of a turn.
pub(super) struct Moving {
    grip: Grip,
    from: [f32; 2],
    to: [f32; 2],
    shift: bool,
}

/// A drag over empty page picking what it runs over.
pub(super) struct Band {
    page: usize,
    from: [f32; 2],
    to: [f32; 2],
}

/// Ways to line the picked objects up.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Align {
    Left,
    Centre,
    Right,
    Top,
    Middle,
    Bottom,
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

    /// Whether any object picked is on page `page`.
    pub(super) fn picked_object_on(&self, page: usize) -> bool {
        self.picked_object.as_ref().is_some_and(|p| p.page == page)
    }

    /// The picked objects' page and places, those still on the page.
    fn picked_indices(&self) -> Option<(usize, Vec<usize>)> {
        let picked = self.picked_object.as_ref()?;
        let count = self.runs.objects.get(&picked.page).map_or(0, |o| o.len());
        let indices: Vec<usize> = picked
            .indices
            .iter()
            .copied()
            .filter(|&i| i < count)
            .collect();
        (!indices.is_empty()).then_some((picked.page, indices))
    }

    /// The box round all the picked objects on the page shown.
    fn picked_box(&self) -> Option<[f32; 4]> {
        let (page, indices) = self.picked_indices()?;
        let mut all: Option<[f32; 4]> = None;
        for index in indices {
            let (_, b) = self.object_shown(page, index)?;
            all = Some(match all {
                None => b,
                Some(a) => [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ],
            });
        }
        all
    }

    /// The box the picked objects are dragged to, on the page shown.
    fn moved_box(&self) -> Option<[f32; 4]> {
        self.moved_snapped().map(|(b, _)| b)
    }

    /// Where the picked objects are being dragged to, lined up with the
    /// page's edges and middle, the grid when shown, and the other
    /// objects' edges and middles when near them, and the lines they
    /// meet (across, then down).
    fn moved_snapped(&self) -> Option<([f32; 4], [Option<f32>; 2])> {
        let raw = self.moved_raw()?;
        let (page, indices) = self.picked_indices()?;
        let moving = self.moving.as_ref()?;
        if matches!(moving.grip, Grip::Turn | Grip::Point(_)) {
            return Some((raw, [None, None]));
        }
        // The page as shown, in points (the bounds drawn last are cleared
        // while a frame is laid out).
        let size = match &self.content {
            super::Content::Ready(opened) => opened.doc.page_size(page)?,
            _ => return None,
        };
        let size = [size.0, size.1];
        let mut xs = vec![0.0, size[0] / 2.0, size[0]];
        let mut ys = vec![0.0, size[1] / 2.0, size[1]];
        if self.gridded {
            xs.extend((1..).map(|i| i as f32 * GRID).take_while(|&x| x < size[0]));
            ys.extend((1..).map(|i| i as f32 * GRID).take_while(|&y| y < size[1]));
        }
        let count = self.runs.objects.get(&page).map_or(0, |o| o.len());
        for i in (0..count).filter(|i| !indices.contains(i)) {
            if let Some((_, [l, t, r, b])) = self.object_shown(page, i) {
                // Not a page's background, whose edges are the page's.
                if (r - l) * (b - t) > size[0] * size[1] * BACKGROUND {
                    continue;
                }
                xs.extend([l, (l + r) / 2.0, r]);
                ys.extend([t, (t + b) / 2.0, b]);
            }
        }
        let grip = match moving.grip {
            Grip::Corner(c) => Some(c),
            _ => None,
        };
        Some(snap(raw, grip, &xs, &ys, SNAP / self.shown))
    }

    fn moved_raw(&self) -> Option<[f32; 4]> {
        let b = self.picked_box()?;
        let moving = self.moving.as_ref()?;
        let (dx, dy) = (moving.to[0] - moving.from[0], moving.to[1] - moving.from[1]);
        let [mut l, mut t, mut r, mut bo] = b;
        match moving.grip {
            Grip::Body => {
                l += dx;
                r += dx;
                t += dy;
                bo += dy;
            }
            Grip::Corner(0) => {
                l = (l + dx).min(r - SMALLEST);
                t = (t + dy).min(bo - SMALLEST);
            }
            Grip::Corner(1) => {
                r = (r + dx).max(l + SMALLEST);
                t = (t + dy).min(bo - SMALLEST);
            }
            Grip::Corner(2) => {
                l = (l + dx).min(r - SMALLEST);
                bo = (bo + dy).max(t + SMALLEST);
            }
            Grip::Corner(_) => {
                r = (r + dx).max(l + SMALLEST);
                bo = (bo + dy).max(t + SMALLEST);
            }
            Grip::Turn | Grip::Point(_) => {}
        }
        Some([l, t, r, bo])
    }

    /// How far the turning handle has been dragged round the picked
    /// objects' middle, in degrees anticlockwise (to eighths of a turn
    /// with Shift).
    fn turned(&self) -> Option<f32> {
        let moving = self.moving.as_ref()?;
        if moving.grip != Grip::Turn {
            return None;
        }
        let b = self.picked_box()?;
        let (mx, my) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        // Down the screen is up the page: angles are measured with y up.
        let was = (moving.from[0] - mx).atan2(-(moving.from[1] - my));
        let now = (moving.to[0] - mx).atan2(-(moving.to[1] - my));
        let mut degrees = (was - now).to_degrees();
        if moving.shift {
            degrees = (degrees / 45.0).round() * 45.0;
        }
        Some(degrees)
    }

    /// A press on the pages with Edit text on and no tool for adding in
    /// hand: grabs the picked objects' handle, corner or body, picks the
    /// object under the pointer (adding it with Ctrl), or starts picking
    /// over empty page. Returns whether it did.
    pub(super) fn object_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.text_editing
            || self.adding.is_some()
            || self.redacting.is_some()
            || self.designing.is_some()
            || self.linking.is_some()
        {
            return false;
        }
        let Some((page, point)) = self.page_point(event.position) else {
            return false;
        };
        let more = event.modifiers.control || event.modifiers.platform;
        let inside = |b: [f32; 4], slack: f32| {
            point[0] >= b[0] - slack
                && point[0] <= b[2] + slack
                && point[1] >= b[1] - slack
                && point[1] <= b[3] + slack
        };
        let start = |this: &mut Self, grip: Grip, cx: &mut Context<Self>| {
            this.moving = Some(Moving {
                grip,
                from: point,
                to: point,
                shift: event.modifiers.shift,
            });
            cx.notify();
            true
        };
        // The picked ones' points when shown, then handle, corners and
        // body.
        if self.picked_object_on(page)
            && let Some(b) = self.picked_box()
        {
            let grip = Self::scale_grip(self.shown);
            if let Some(points) = self.shown_points() {
                let hit = points.iter().position(|p| {
                    (p[0] - point[0]).abs() <= grip && (p[1] - point[1]).abs() <= grip
                });
                if let Some(i) = hit {
                    return start(self, Grip::Point(i), cx);
                }
            }
            let turn = [(b[0] + b[2]) / 2.0, b[1] - TURN_ABOVE / self.shown.max(0.1)];
            if (turn[0] - point[0]).abs() <= grip && (turn[1] - point[1]).abs() <= grip {
                return start(self, Grip::Turn, cx);
            }
            let corners = [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]];
            let corner = corners
                .iter()
                .position(|c| (c[0] - point[0]).abs() <= grip && (c[1] - point[1]).abs() <= grip);
            if let Some(corner) = corner
                && !more
            {
                return start(self, Grip::Corner(corner), cx);
            }
            if inside(b, 0.0) && !more {
                return start(self, Grip::Body, cx);
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
                if more {
                    self.toggle_picked(page, index, cx);
                    cx.notify();
                    return true;
                }
                self.pick_object(page, vec![index], cx);
                start(self, Grip::Body, cx)
            }
            None => {
                if !more && self.picked_object.take().is_some() {
                    cx.notify();
                }
                self.finish_text_edit(true, cx);
                self.band = Some(Band {
                    page,
                    from: point,
                    to: point,
                });
                true
            }
        }
    }

    /// The picked shape's own points on the page shown, while they are
    /// shown to drag.
    fn shown_points(&self) -> Option<Vec<[f32; 2]>> {
        self.picked_object.as_ref().filter(|p| p.points)?;
        let (page, indices) = self.picked_indices()?;
        let [index] = indices[..] else {
            return None;
        };
        let (object, _) = self.object_shown(page, index)?;
        if object.kind != ObjectKind::Path || object.points.is_empty() {
            return None;
        }
        let transform = self.page_transform(page)?;
        Some(
            object
                .points
                .iter()
                .map(|&[x, y]| {
                    let [l, t, _, _] = katna_pdf_render::map_rect(transform, [x, y, x, y]);
                    [l, t]
                })
                .collect(),
        )
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

    /// Object `index` of page `page` added to those picked, or taken off
    /// them when it is one; on another page, picked alone.
    fn toggle_picked(&mut self, page: usize, index: usize, cx: &mut Context<Self>) {
        match self.picked_object.as_mut().filter(|p| p.page == page) {
            Some(picked) => {
                if let Some(at) = picked.indices.iter().position(|&i| i == index) {
                    picked.indices.remove(at);
                    if picked.indices.is_empty() {
                        self.picked_object = None;
                    }
                } else {
                    picked.indices.push(index);
                }
                self.refresh_inspector(cx);
            }
            None => self.pick_object(page, vec![index], cx),
        }
    }

    /// Picks objects `indices` of page `page`, their numbers in the
    /// inspector.
    fn pick_object(&mut self, page: usize, indices: Vec<usize>, cx: &mut Context<Self>) {
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let fields = [0, 1, 2, 3, 4, 5].map(|_| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_accent(accent);
                input
            })
        });
        let subs = fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                cx.subscribe(f, move |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        match i {
                            4 => this.apply_turn(cx),
                            5 => this.apply_width(cx),
                            _ => this.apply_inspector(cx),
                        }
                    }
                })
            })
            .collect();
        self.picked_object = Some(Picked {
            page,
            indices,
            fields,
            _fields: subs,
            opacity: 1.0,
            points: false,
        });
        self.refresh_inspector(cx);
    }

    /// Ctrl+A with Edit text on: picks every object on the page shown.
    /// Returns whether there were any.
    pub(super) fn pick_all_objects(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.text_editing || self.adding.is_some() {
            return false;
        }
        let page = self
            .picked_object
            .as_ref()
            .map_or_else(|| self.current_page(), |p| p.page);
        let Some(objects) = self.runs.objects.get(&page) else {
            return false;
        };
        let (w, h) = self.page_size(page);
        let Some(transform) = self.page_transform(page) else {
            return false;
        };
        let indices: Vec<usize> = objects
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                let b = katna_pdf_render::map_rect(transform, o.rect);
                (b[2] - b[0]) * (b[3] - b[1]) < BACKGROUND * w * h
            })
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return false;
        }
        self.pick_object(page, indices, cx);
        cx.notify();
        true
    }

    /// Puts the picked objects' place and size in the inspector.
    pub(super) fn refresh_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.picked_box() else {
            return;
        };
        let angle = self.picked_angle();
        let width = self.picked_style().map(|s| s.width);
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let values = [b[0], b[1], b[2] - b[0], b[3] - b[1], angle];
        for (field, value) in picked.fields.iter().zip(values) {
            let shown = format!("{:.1}", value);
            field.update(cx, |f, cx| f.set_text(shown, cx));
        }
        let shown = width.map_or_else(String::new, |w| format!("{w:.1}"));
        picked.fields[5].update(cx, |f, cx| f.set_text(shown, cx));
    }

    /// How the one shape picked is painted.
    fn picked_style(&self) -> Option<ShapeStyle> {
        let (page, indices) = self.picked_indices()?;
        let [index] = indices[..] else {
            return None;
        };
        let (object, _) = self.object_shown(page, index)?;
        object.style
    }

    /// The line width typed: the picked shapes' lines go that wide.
    fn apply_width(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let Some(width) = length(picked.fields[5].read(cx).text()).filter(|w| *w >= 0.0) else {
            return;
        };
        self.style_picked(
            ObjectChange::Style {
                fill: None,
                stroke: None,
                width: Some(width),
                dash: None,
                cap: None,
                join: None,
            },
            cx,
        );
    }

    /// The picked shapes painted as `change` says (a `Style`).
    fn style_picked(&mut self, change: ObjectChange, cx: &mut Context<Self>) {
        let Some((page, indices)) = self.picked_indices() else {
            return;
        };
        let shapes: Vec<usize> = indices
            .into_iter()
            .filter(|&i| {
                self.object_shown(page, i)
                    .is_some_and(|(o, _)| o.kind == ObjectKind::Path)
            })
            .collect();
        if shapes.is_empty() {
            return;
        }
        if let Some(picked) = self.picked_object.as_mut() {
            picked.indices = shapes;
        }
        self.change_picked(change, cx);
    }

    /// The picked shape filled, or not, as it was not; one left neither
    /// filled nor lined would vanish, so its line comes on instead.
    fn toggle_fill(&mut self, cx: &mut Context<Self>) {
        let Some(style) = self.picked_style() else {
            return;
        };
        let fill = style.fill.is_none();
        self.style_picked(
            ObjectChange::Style {
                fill: Some(fill),
                stroke: (!fill && style.stroke.is_none()).then_some(true),
                width: None,
                dash: None,
                cap: None,
                join: None,
            },
            cx,
        );
    }

    /// The picked shape's line drawn, or not, as it was not; one left
    /// neither lined nor filled would vanish, so it is filled instead.
    fn toggle_stroke(&mut self, cx: &mut Context<Self>) {
        let Some(style) = self.picked_style() else {
            return;
        };
        let stroke = style.stroke.is_none();
        self.style_picked(
            ObjectChange::Style {
                fill: (!stroke && style.fill.is_none()).then_some(true),
                stroke: Some(stroke),
                width: None,
                dash: None,
                cap: None,
                join: None,
            },
            cx,
        );
    }

    /// The picked shape's line dashed, or solid again.
    fn toggle_dash(&mut self, cx: &mut Context<Self>) {
        let Some(style) = self.picked_style() else {
            return;
        };
        let dash = if style.dash.is_empty() {
            let w = style.width.max(1.0);
            vec![w * 3.0, w * 2.0]
        } else {
            Vec::new()
        };
        self.style_picked(
            ObjectChange::Style {
                fill: None,
                stroke: None,
                width: None,
                dash: Some(dash),
                cap: None,
                join: None,
            },
            cx,
        );
    }

    /// The picked shape's own points shown to drag, or hidden again.
    fn toggle_points(&mut self, cx: &mut Context<Self>) {
        if let Some(picked) = self.picked_object.as_mut() {
            picked.points = !picked.points;
            cx.notify();
        }
    }

    /// How far the picked object is turned, in degrees anticlockwise: a
    /// picture's or form's from how it is placed; 0 for a shape or for
    /// many.
    fn picked_angle(&self) -> f32 {
        let Some((page, indices)) = self.picked_indices() else {
            return 0.0;
        };
        if let [index] = indices[..]
            && let Some((object, _)) = self.object_shown(page, index)
        {
            return object.angle();
        }
        0.0
    }

    /// The inspector's numbers typed: the picked objects go there.
    fn apply_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let values: Option<Vec<f32>> = picked.fields[..4]
            .iter()
            .map(|f| length(f.read(cx).text()))
            .collect();
        let Some(&[l, t, w, h]) = values.as_deref() else {
            return;
        };
        if !(w >= SMALLEST && h >= SMALLEST) {
            return;
        }
        self.place_picked([l, t, l + w, t + h], cx);
    }

    /// The turn typed: the picked objects turn to it.
    fn apply_turn(&mut self, cx: &mut Context<Self>) {
        let Some(picked) = self.picked_object.as_ref() else {
            return;
        };
        let Some(to) = length(picked.fields[4].read(cx).text()) else {
            return;
        };
        let by = to - self.picked_angle();
        if by.abs() > 1e-3 {
            self.rotate_picked(by, cx);
        }
    }

    pub(super) fn object_drag(
        &mut self,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let page = match (&self.moving, &self.picked_object, &self.band) {
            (Some(_), Some(picked), _) => picked.page,
            (_, _, Some(band)) => band.page,
            _ => return false,
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            (event.position.x - b.left()) / shown,
            (event.position.y - b.top()) / shown,
        ];
        if let Some(moving) = self.moving.as_mut() {
            moving.to = point;
            moving.shift = event.modifiers.shift;
        } else if let Some(band) = self.band.as_mut() {
            band.to = point;
        }
        cx.notify();
        true
    }

    /// The pointer let go: the picked objects go where they were dragged,
    /// turn as far as the handle went, or the band picks what it ran
    /// over.
    pub(super) fn object_release(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(band) = self.band.take() {
            cx.notify();
            let dragged = (band.to[0] - band.from[0]).hypot(band.to[1] - band.from[1]);
            if dragged < CLICK {
                return true;
            }
            let b = [
                band.from[0].min(band.to[0]),
                band.from[1].min(band.to[1]),
                band.from[0].max(band.to[0]),
                band.from[1].max(band.to[1]),
            ];
            let (w, h) = self.page_size(band.page);
            let count = self.runs.objects.get(&band.page).map_or(0, |o| o.len());
            let indices: Vec<usize> = (0..count)
                .filter(|&i| {
                    self.object_shown(band.page, i).is_some_and(|(_, o)| {
                        o[0] >= b[0]
                            && o[1] >= b[1]
                            && o[2] <= b[2]
                            && o[3] <= b[3]
                            && (o[2] - o[0]) * (o[3] - o[1]) < BACKGROUND * w * h
                    })
                })
                .collect();
            if !indices.is_empty() {
                self.pick_object(band.page, indices, cx);
            }
            return true;
        }
        let turned = self.turned();
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
        match (moving.grip, turned) {
            (Grip::Point(index), _) => {
                if let Some(inv) = self.picked_indices().and_then(|(p, _)| self.inverse(p)) {
                    let to = inv.map(moving.to);
                    self.change_picked(ObjectChange::MovePoint { index, to }, cx);
                }
            }
            (_, Some(degrees)) => {
                if degrees.abs() > 0.05 {
                    self.rotate_picked(degrees, cx);
                }
            }
            (_, None) => self.place_picked(new, cx),
        }
        true
    }

    /// Moves and sizes the picked objects together to fill `shown` (a box
    /// on the page shown).
    fn place_picked(&mut self, shown: [f32; 4], cx: &mut Context<Self>) {
        let (Some((page, indices)), Some(b)) = (self.picked_indices(), self.picked_box()) else {
            return;
        };
        let Some(inv) = self.inverse(page) else {
            return;
        };
        let [l, bo, r, t] = inv.rect(b);
        let [nl, nb, nr, nt] = inv.rect(shown);
        // Flat things (a line across or down) keep their flat side.
        let sx = if r - l > 1e-3 {
            (nr - nl) / (r - l)
        } else {
            1.0
        };
        let sy = if t - bo > 1e-3 {
            (nt - nb) / (t - bo)
        } else {
            1.0
        };
        let matrix = [sx, 0.0, 0.0, sy, nl - l * sx, nb - bo * sy];
        self.transform_picked(page, indices.into_iter().map(|i| (i, matrix)).collect(), cx);
    }

    /// Moves the picked objects as `moves` says, each by its matrix in
    /// the page's own space.
    fn transform_picked(
        &mut self,
        page: usize,
        moves: Vec<(usize, [f32; 6])>,
        cx: &mut Context<Self>,
    ) {
        self.change_page(
            page,
            move |doc| {
                doc.transform_objects(page, &moves)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// The picked objects' box in the page's own space (left, bottom,
    /// right, top).
    fn picked_rect(&self) -> Option<(usize, Vec<usize>, [f32; 4])> {
        let (page, indices) = self.picked_indices()?;
        let b = self.picked_box()?;
        let inv = self.inverse(page)?;
        Some((page, indices, inv.rect(b)))
    }

    /// The arrow keys: the picked objects move `dx` across and `dy` down
    /// the page shown, in points. Returns whether any were picked.
    pub(super) fn nudge_picked(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) -> bool {
        let Some(b) = self.picked_box() else {
            return false;
        };
        self.place_picked([b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy], cx);
        true
    }

    /// Turns the picked objects `degrees` anticlockwise about their
    /// middle.
    fn rotate_picked(&mut self, degrees: f32, cx: &mut Context<Self>) {
        let Some((page, indices, [l, b, r, t])) = self.picked_rect() else {
            return;
        };
        let (mx, my) = ((l + r) / 2.0, (b + t) / 2.0);
        let (sin, cos) = degrees.to_radians().sin_cos();
        let matrix = [
            cos,
            sin,
            -sin,
            cos,
            mx - mx * cos + my * sin,
            my - mx * sin - my * cos,
        ];
        self.transform_picked(page, indices.into_iter().map(|i| (i, matrix)).collect(), cx);
    }

    /// Turns the picked objects over, left to right or top to bottom.
    fn flip_picked(&mut self, across: bool, cx: &mut Context<Self>) {
        let Some((page, indices, [l, b, r, t])) = self.picked_rect() else {
            return;
        };
        let matrix = if across {
            [-1.0, 0.0, 0.0, 1.0, l + r, 0.0]
        } else {
            [1.0, 0.0, 0.0, -1.0, 0.0, b + t]
        };
        self.transform_picked(page, indices.into_iter().map(|i| (i, matrix)).collect(), cx);
    }

    /// Makes the picked objects as big as the page holds, in the middle
    /// of it.
    fn fit_picked(&mut self, cx: &mut Context<Self>) {
        let Some((page, _, [l, b, r, t])) = self.picked_rect() else {
            return;
        };
        let Some(inv) = self.inverse(page) else {
            return;
        };
        let (w, h) = self.page_size(page);
        let [pl, pb, pr, pt] = inv.rect([0.0, 0.0, w, h]);
        let (pw, ph) = (pr - pl, pt - pb);
        let (ow, oh) = ((r - l).max(1e-3), (t - b).max(1e-3));
        let scale = (pw / ow).min(ph / oh);
        let (nw, nh) = (ow * scale, oh * scale);
        let nl = pl + (pw - nw) / 2.0;
        let nb = pb + (ph - nh) / 2.0;
        let shown = self
            .page_transform(page)
            .map(|tr| katna_pdf_render::map_rect(tr, [nl, nb, nl + nw, nb + nh]));
        if let Some(shown) = shown {
            self.place_picked(shown, cx);
        }
    }

    /// Lines the picked objects up with each other, or one with the
    /// page.
    fn align_picked(&mut self, how: Align, cx: &mut Context<Self>) {
        let Some((page, indices)) = self.picked_indices() else {
            return;
        };
        let target = if indices.len() > 1 {
            self.picked_box()
        } else {
            let (w, h) = self.page_size(page);
            Some([0.0, 0.0, w, h])
        };
        let (Some(target), Some(inv)) = (target, self.inverse(page)) else {
            return;
        };
        let mut moves = Vec::new();
        for index in indices {
            let Some((_, b)) = self.object_shown(page, index) else {
                continue;
            };
            let (dx, dy) = match how {
                Align::Left => (target[0] - b[0], 0.0),
                Align::Centre => ((target[0] + target[2] - b[0] - b[2]) / 2.0, 0.0),
                Align::Right => (target[2] - b[2], 0.0),
                Align::Top => (0.0, target[1] - b[1]),
                Align::Middle => (0.0, (target[1] + target[3] - b[1] - b[3]) / 2.0),
                Align::Bottom => (0.0, target[3] - b[3]),
            };
            let [l, bo, r, t] = inv.rect(b);
            let [nl, nb, nr, nt] = inv.rect([b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy]);
            let _ = (r, t, nr, nt);
            moves.push((index, [1.0, 0.0, 0.0, 1.0, nl - l, nb - bo]));
        }
        self.transform_picked(page, moves, cx);
    }

    /// Changes the picked objects as `change` says; ones moved to the
    /// front or back stay picked where they now are in the page's list.
    fn change_picked(&mut self, change: ObjectChange, cx: &mut Context<Self>) {
        let Some((page, mut indices)) = self.picked_indices() else {
            return;
        };
        let count = self.runs.objects.get(&page).map_or(0, |o| o.len());
        let Some(picked) = self.picked_object.as_mut() else {
            return;
        };
        indices.sort_unstable();
        let n = indices.len();
        match change {
            ObjectChange::Front => picked.indices = (count - n..count).collect(),
            ObjectChange::Back => picked.indices = (0..n).collect(),
            ObjectChange::Forward => {
                picked.indices = indices.iter().map(|&i| (i + 1).min(count - 1)).collect();
            }
            ObjectChange::Backward => {
                picked.indices = indices.iter().map(|&i| i.saturating_sub(1)).collect();
            }
            ObjectChange::Opacity(o) => picked.opacity = o,
            _ => {}
        }
        self.change_page(
            page,
            move |doc| {
                let mut owned: Option<katna_pdf_cos::Document> = None;
                let mut bytes = Vec::new();
                // To the front, lowest first, each landing on top of the
                // last; to the back, highest first, each going under.
                let order: Vec<usize> = match change {
                    ObjectChange::Front => {
                        indices.iter().enumerate().map(|(k, &i)| i - k).collect()
                    }
                    ObjectChange::Back => indices
                        .iter()
                        .rev()
                        .enumerate()
                        .map(|(k, &i)| i + k)
                        .collect(),
                    ObjectChange::Forward => indices.iter().rev().copied().collect(),
                    _ => indices.clone(),
                };
                let last = order.len().saturating_sub(1);
                for (k, index) in order.into_iter().enumerate() {
                    let now = owned.as_ref().unwrap_or(doc);
                    bytes = now
                        .change_object(page, index, &change)
                        .map_err(|e| e.to_string())?;
                    if k < last {
                        owned = Some(
                            katna_pdf_cos::Document::load(bytes.clone())
                                .map_err(|e| e.to_string())?,
                        );
                    }
                }
                Ok(bytes)
            },
            cx,
        );
    }

    /// The picked shapes coloured `ink`: one shape's inside when it is
    /// filled, else its line; many take it inside and out. With
    /// `line_only` (Shift), the line only.
    pub(super) fn colour_picked(&mut self, ink: [f32; 3], line_only: bool, cx: &mut Context<Self>) {
        let one = self.picked_style();
        let change = match (line_only, one) {
            (true, _) => ObjectChange::Colours {
                fill: None,
                stroke: Some(ink),
            },
            (false, Some(style)) if style.fill.is_some() => ObjectChange::Colours {
                fill: Some(ink),
                stroke: None,
            },
            (false, Some(_)) => ObjectChange::Colours {
                fill: None,
                stroke: Some(ink),
            },
            (false, None) => ObjectChange::Colours {
                fill: Some(ink),
                stroke: Some(ink),
            },
        };
        self.style_picked(change, cx);
    }

    /// The picked picture drawn whole, out from under the clipping path
    /// that cuts it down.
    fn unclip_picked(&mut self, cx: &mut Context<Self>) {
        self.change_picked(ObjectChange::Unclip, cx);
    }

    /// Asks where to save the picked picture as a file: a JPEG as it is
    /// in the file, else a PNG.
    fn save_picked_picture(&mut self, cx: &mut Context<Self>) {
        let Some((page, indices)) = self.picked_indices() else {
            return;
        };
        let [index] = indices[..] else {
            return;
        };
        let key = self.key.clone();
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let picture = doc
                        .picture_file(page, index)
                        .ok_or_else(|| t("object-save-picture-none", &[]))?;
                    picture_bytes(picture)
                })
                .await;
            let (bytes, ext) = match read {
                Ok(read) => read,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.notice = Some(t("object-save-failed", &[("error", &error)]));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let name = format!("{stem}-p{}-{}.{ext}", page + 1, index + 1);
            let Ok(chosen) = this.update(cx, |_, cx| cx.prompt_for_new_path(&folder, Some(&name)))
            else {
                return;
            };
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t(
                        "object-saved-picture",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => t("object-save-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a picture file to show in the picked picture's place.
    fn replace_picked(&mut self, cx: &mut Context<Self>) {
        let Some((page, indices)) = self.picked_indices() else {
            return;
        };
        let [index] = indices[..] else {
            return;
        };
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

    /// Delete with objects picked: they are taken out. Returns whether
    /// any were.
    pub(super) fn delete_picked_object(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((page, indices)) = self.picked_indices() else {
            return false;
        };
        self.picked_object = None;
        self.moving = None;
        self.change_page(
            page,
            move |doc| {
                doc.delete_objects(page, &indices)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        true
    }

    /// Ctrl+C with objects picked: copies them, for Ctrl+V on any page of
    /// any open file. Returns whether any were picked.
    pub(super) fn copy_picked_objects(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((page, indices)) = self.picked_indices() else {
            return false;
        };
        let key = self.key.clone();
        let Some(path) = self.path.clone() else {
            return false;
        };
        let current = self.unsaved();
        cx.spawn(async move |this, cx| {
            let copies = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).ok()?;
                    indices
                        .iter()
                        .map(|&i| doc.copy_object(page, i).ok())
                        .collect::<Option<Vec<_>>>()
                })
                .await;
            let Some(copies) = copies else {
                return;
            };
            this.update(cx, |_, cx| {
                if let Ok(mut copied) = COPIED.lock() {
                    *copied = copies;
                }
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(COPIED_MARK.into()));
            })
            .ok();
        })
        .detach();
        true
    }

    /// Ctrl+X: copies the picked objects and takes them out.
    pub(super) fn cut_picked_objects(&mut self, cx: &mut Context<Self>) -> bool {
        self.copy_picked_objects(cx) && self.delete_picked_object(cx)
    }

    /// Ctrl+V with objects copied last: pastes them on the page shown,
    /// where they were (`in_place`), or moved a little along when that
    /// page is the one they were copied from. Returns whether any were
    /// copied.
    pub(super) fn paste_copied_objects(&mut self, in_place: bool, cx: &mut Context<Self>) -> bool {
        if !self.text_editing {
            return false;
        }
        let marked = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text == COPIED_MARK);
        let copies = match COPIED.lock() {
            Ok(copied) if marked && !copied.is_empty() => copied.clone(),
            _ => return false,
        };
        let page = self
            .picked_object
            .as_ref()
            .map_or_else(|| self.current_page(), |p| p.page);
        let count = self.runs.objects.get(&page).map_or(0, |o| o.len());
        // Over its original, a copy would be lost: along a little, unless
        // it is wanted just there. Where it lands on this page is then
        // what the next paste moves along from.
        let by = if in_place {
            [0.0, 0.0]
        } else {
            let same = copies.iter().any(|c| {
                self.runs
                    .objects
                    .get(&page)
                    .is_some_and(|objects| objects.iter().any(|o| o.rect == c.rect))
            });
            if same { [ASIDE, -ASIDE] } else { [0.0, 0.0] }
        };
        if by != [0.0, 0.0]
            && let Ok(mut copied) = COPIED.lock()
        {
            for copy in copied.iter_mut() {
                copy.rect = [
                    copy.rect[0] + by[0],
                    copy.rect[1] + by[1],
                    copy.rect[2] + by[0],
                    copy.rect[3] + by[1],
                ];
            }
        }
        self.pick_object(page, (count..count + copies.len()).collect(), cx);
        self.change_page(
            page,
            move |doc| {
                let refs: Vec<&ObjectCopy> = copies.iter().collect();
                doc.paste_objects(page, &refs, by)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        true
    }

    /// The picked objects' frame, corners and turning handle on page
    /// `page`, where they are being dragged to while they are; the band
    /// being dragged over the page; and the grid when shown.
    pub(super) fn objects_overlay(&self, page: usize, cx: &Context<Self>) -> Vec<AnyElement> {
        let shown = self.shown;
        // The frame and handles take the press themselves, so a
        // paragraph's box under them does not.
        let handle = cx.entity().downgrade();
        let grab = move || {
            let handle = handle.clone();
            move |event: &gpui::MouseDownEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                cx.stop_propagation();
                handle
                    .update(cx, |this, cx| this.press(event, window, cx))
                    .ok();
            }
        };
        let accent = gpui::rgba(self.theme.katna.accent);
        let paper = self.paper();
        let mut out = Vec::new();
        if self.gridded && self.text_editing {
            let (w, h) = self.page_size(page);
            let line = gpui::rgba(0x80_80_80_30);
            for x in (1..).map(|i| i as f32 * GRID).take_while(|&x| x < w) {
                out.push(
                    div()
                        .absolute()
                        .left(desktop_px(x * shown))
                        .top_0()
                        .bottom_0()
                        .w(desktop_px(1.0))
                        .bg(line)
                        .into_any_element(),
                );
            }
            for y in (1..).map(|i| i as f32 * GRID).take_while(|&y| y < h) {
                out.push(
                    div()
                        .absolute()
                        .top(desktop_px(y * shown))
                        .left_0()
                        .right_0()
                        .h(desktop_px(1.0))
                        .bg(line)
                        .into_any_element(),
                );
            }
        }
        if let Some(band) = self.band.as_ref().filter(|b| b.page == page) {
            let l = band.from[0].min(band.to[0]);
            let t = band.from[1].min(band.to[1]);
            let r = band.from[0].max(band.to[0]);
            let b = band.from[1].max(band.to[1]);
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(l * shown))
                    .top(desktop_px(t * shown))
                    .w(desktop_px((r - l) * shown))
                    .h(desktop_px((b - t) * shown))
                    .border_1()
                    .border_color(accent)
                    .bg(gpui::rgba((self.theme.katna.accent & 0xffff_ff00) | 0x20))
                    .into_any_element(),
            );
        }
        if !self.picked_object_on(page) {
            return out;
        }
        let Some((_, indices)) = self.picked_indices() else {
            return out;
        };
        let moved = self.moved_snapped();
        let Some(b) = moved.map(|(b, _)| b).or_else(|| self.picked_box()) else {
            return out;
        };
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
        // Each of many, thinly, inside the frame round them all.
        if indices.len() > 1 && self.moving.is_none() {
            for &index in &indices {
                if let Some((_, o)) = self.object_shown(page, index) {
                    out.push(
                        div()
                            .absolute()
                            .left(desktop_px(o[0] * shown))
                            .top(desktop_px(o[1] * shown))
                            .w(desktop_px((o[2] - o[0]) * shown))
                            .h(desktop_px((o[3] - o[1]) * shown))
                            .border_1()
                            .border_color(gpui::rgba(
                                (self.theme.katna.accent & 0xffff_ff00) | 0x80,
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        out.extend([div()
            .id(("object-frame", page))
            .absolute()
            .left(desktop_px(b[0] * shown - 1.0))
            .top(desktop_px(b[1] * shown - 1.0))
            .w(desktop_px((b[2] - b[0]) * shown + 2.0))
            .h(desktop_px((b[3] - b[1]) * shown + 2.0))
            .border_1()
            .border_color(accent)
            .cursor_move()
            .on_mouse_down(gpui::MouseButton::Left, grab())
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
                    .id(("object-corner", i))
                    .absolute()
                    .left(desktop_px(x * shown - size / 2.0))
                    .top(desktop_px(y * shown - size / 2.0))
                    .size(desktop_px(size))
                    .bg(paper)
                    .border_1()
                    .border_color(accent)
                    .cursor(cursor)
                    .on_mouse_down(gpui::MouseButton::Left, grab())
                    .into_any_element(),
            );
        }
        // The shape's own points, when shown; the one being dragged
        // goes with the pointer.
        if let Some(points) = self.shown_points() {
            let dragged = self.moving.as_ref().and_then(|m| match m.grip {
                Grip::Point(i) => Some((i, m.to)),
                _ => None,
            });
            for (i, p) in points.into_iter().enumerate() {
                let p = match dragged {
                    Some((d, to)) if d == i => to,
                    _ => p,
                };
                out.push(
                    div()
                        .id(("object-point", i))
                        .absolute()
                        .left(desktop_px(p[0] * shown - size / 2.0))
                        .top(desktop_px(p[1] * shown - size / 2.0))
                        .size(desktop_px(size))
                        .rounded_full()
                        .bg(accent)
                        .border_1()
                        .border_color(paper)
                        .cursor_crosshair()
                        .on_mouse_down(gpui::MouseButton::Left, grab())
                        .into_any_element(),
                );
            }
        }
        // The turning handle above the middle of the top, on a stalk;
        // while it is dragged, how far round it has gone.
        let mx = (b[0] + b[2]) / 2.0 * shown;
        let top = b[1] * shown;
        out.push(
            div()
                .absolute()
                .left(desktop_px(mx - 0.5))
                .top(desktop_px(top - TURN_ABOVE + size / 2.0))
                .w(desktop_px(1.0))
                .h(desktop_px(TURN_ABOVE - size / 2.0))
                .bg(accent)
                .into_any_element(),
        );
        out.push(
            div()
                .id(("object-turn", page))
                .absolute()
                .left(desktop_px(mx - size / 2.0))
                .top(desktop_px(top - TURN_ABOVE - size / 2.0))
                .size(desktop_px(size))
                .rounded_full()
                .bg(paper)
                .border_1()
                .border_color(accent)
                .cursor_grab()
                .on_mouse_down(gpui::MouseButton::Left, grab())
                .into_any_element(),
        );
        if let Some(degrees) = self.turned() {
            let th = self.theme;
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(mx + size))
                    .top(desktop_px(top - TURN_ABOVE - size))
                    .px(px(space::S1))
                    .rounded(px(katna_ui::tokens::radius::XS))
                    .bg(th.field)
                    .text_size(px(text::SMALL))
                    .text_color(gpui::rgba(th.katna.text))
                    .child(format!("{degrees:.0}°"))
                    .into_any_element(),
            );
        }
        out
    }

    /// The inspector, in the row of tools while objects are picked: what
    /// they are, where, how big and how far turned, and buttons to line
    /// them up, step them forward and back, flip, fit and delete them.
    pub(super) fn inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picked = self.picked_object.as_ref()?;
        let (page, indices) = self.picked_indices()?;
        let one = match indices[..] {
            [index] => self.object_shown(page, index).map(|(o, _)| o),
            _ => None,
        };
        let many = indices.len() > 1;
        let th = self.theme;
        let mut row = div().flex().items_center().gap(px(space::S1));
        let style = one.as_ref().and_then(|o| o.style.clone());
        let labels = ["X", "Y", "W", "H", "∠", "─"];
        for (i, (field, label)) in picked.fields.iter().zip(labels).enumerate() {
            // The line width only for one shape.
            if i == 5 && style.is_none() {
                break;
            }
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
        if let Some(style) = style {
            let on = |on: bool| {
                if on {
                    th.katna.accent
                } else {
                    th.katna.text_dim
                }
            };
            let toggle = |id: &'static str, icon: &'static str, lit: bool, tip: &'static str| {
                widgets::icon_button_colored(id, icon, 18.0, on(lit), &th.katna)
                    .when(lit, |b| b.bg(th.field))
                    .tip(t(tip, &[]), &th.katna)
            };
            row = row
                .child(
                    toggle("object-fill", "fill", style.fill.is_some(), "object-fill")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_fill(cx))),
                )
                .child(
                    toggle(
                        "object-stroke",
                        "stroke",
                        style.stroke.is_some(),
                        "object-stroke",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_stroke(cx))),
                )
                .child(
                    toggle("object-dash", "dash", !style.dash.is_empty(), "object-dash")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_dash(cx))),
                );
            if one.as_ref().is_some_and(|o| !o.points.is_empty()) {
                row = row.child(
                    toggle("object-points", "points", picked.points, "object-points")
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_points(cx))),
                );
            }
            row = row.child(separator(th));
        }
        let suffix = if many { "-together" } else { "-page" };
        for (id, icon, how) in [
            ("object-align-left", "align-left", Align::Left),
            ("object-align-centre", "align-centre", Align::Centre),
            ("object-align-right", "align-right", Align::Right),
            ("object-align-top", "align-top", Align::Top),
            ("object-align-middle", "align-middle", Align::Middle),
            ("object-align-bottom", "align-bottom", Align::Bottom),
        ] {
            row = row.child(
                tool(id, icon, t(&format!("{id}{suffix}"), &[]))
                    .on_click(cx.listener(move |this, _, _, cx| this.align_picked(how, cx))),
            );
        }
        row = row
            .child(separator(th))
            .child(
                tool("object-rotate", "rotate-cw", t("object-rotate", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.rotate_picked(-90.0, cx))),
            )
            .child(
                tool("object-flip-h", "flip-h", t("object-flip-h", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.flip_picked(true, cx))),
            )
            .child(
                tool("object-flip-v", "flip-v", t("object-flip-v", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.flip_picked(false, cx))),
            )
            .child(
                tool("object-fit", "fit-page", t("object-fit", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.fit_picked(cx))),
            )
            .child(separator(th))
            .child(
                tool("object-forward", "step-forward", t("object-forward", &[])).on_click(
                    cx.listener(|this, _, _, cx| this.change_picked(ObjectChange::Forward, cx)),
                ),
            )
            .child(
                tool("object-backward", "step-back", t("object-backward", &[])).on_click(
                    cx.listener(|this, _, _, cx| this.change_picked(ObjectChange::Backward, cx)),
                ),
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
        if one.as_ref().is_some_and(|o| o.kind == ObjectKind::Picture) {
            row = row
                .child(
                    tool("object-replace", "image", t("object-replace", &[]))
                        .on_click(cx.listener(|this, _, _, cx| this.replace_picked(cx))),
                )
                .child(
                    tool(
                        "object-save-picture",
                        "download",
                        t("object-save-picture", &[]),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.save_picked_picture(cx))),
                );
        }
        if one.as_ref().is_some_and(|o| o.clipped) {
            row = row.child(
                tool("object-unclip", "crop", t("object-unclip", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.unclip_picked(cx))),
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

/// A picture as a file's bytes and the file's extension: a JPEG as it
/// is, else a PNG.
fn picture_bytes(picture: PictureFile) -> Result<(Vec<u8>, &'static str), String> {
    use image::{DynamicImage, GrayImage, RgbImage};
    let (width, height, components, bits, data) = match picture {
        PictureFile::Jpeg(bytes) => return Ok((bytes, "jpg")),
        PictureFile::Samples {
            width,
            height,
            components,
            bits,
            data,
        } => (width, height, components, bits, data),
    };
    let n = components as usize;
    let per_row = (width as usize * n * bits as usize).div_ceil(8);
    // Each sample as a byte, 0 to 255.
    let mut samples = Vec::with_capacity(width as usize * height as usize * n);
    for y in 0..height as usize {
        let row = data.get(y * per_row..(y + 1) * per_row).unwrap_or(&[]);
        for i in 0..width as usize * n {
            let value = match bits {
                8 => row.get(i).copied().unwrap_or(0),
                16 => row.get(i * 2).copied().unwrap_or(0),
                1 => {
                    let bit = row.get(i / 8).map_or(0, |b| (b >> (7 - i % 8)) & 1);
                    if bit == 1 { 255 } else { 0 }
                }
                2 => row
                    .get(i / 4)
                    .map_or(0, |b| ((b >> (6 - 2 * (i % 4))) & 3) * 85),
                4 => row.get(i / 2).map_or(0, |b| {
                    let v = if i % 2 == 0 { b >> 4 } else { b & 15 };
                    v * 17
                }),
                _ => return Err(format!("{bits} bits a sample")),
            };
            samples.push(value);
        }
    }
    let image = match n {
        1 => GrayImage::from_raw(width, height, samples).map(DynamicImage::ImageLuma8),
        3 => RgbImage::from_raw(width, height, samples).map(DynamicImage::ImageRgb8),
        4 => {
            let rgb: Vec<u8> = samples
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|cmyk| {
                    let k = 255 - cmyk[3] as u32;
                    [0, 1, 2].map(|i| ((255 - cmyk[i] as u32) * k / 255) as u8)
                })
                .collect();
            RgbImage::from_raw(width, height, rgb).map(DynamicImage::ImageRgb8)
        }
        _ => return Err(format!("{n} components a pixel")),
    };
    let image = image.ok_or_else(|| "too few samples".to_owned())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok((out.into_inner(), "png"))
}

/// A length typed: points, or with a unit (`mm`, `cm`, `in`, `pt`), and
/// sums, differences, products and quotients of those (`2cm+5`,
/// `210mm/2`).
pub(super) fn length(typed: &str) -> Option<f32> {
    let s: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
    let s = s.replace(',', ".");
    let (value, rest) = sum(&s)?;
    rest.is_empty().then_some(value)
}

fn sum(s: &str) -> Option<(f32, &str)> {
    let (mut value, mut rest) = product(s)?;
    loop {
        let sign = match rest.chars().next() {
            Some('+') => 1.0,
            Some('-') => -1.0,
            _ => return Some((value, rest)),
        };
        let (next, after) = product(&rest[1..])?;
        value += sign * next;
        rest = after;
    }
}

fn product(s: &str) -> Option<(f32, &str)> {
    let (mut value, mut rest) = factor(s)?;
    loop {
        let times = match rest.chars().next() {
            Some('*') | Some('×') => true,
            Some('/') => false,
            _ => return Some((value, rest)),
        };
        let (next, after) = factor(&rest[rest.chars().next()?.len_utf8()..])?;
        value = if times {
            value * next
        } else if next.abs() > 1e-9 {
            value / next
        } else {
            return None;
        };
        rest = after;
    }
}

fn factor(s: &str) -> Option<(f32, &str)> {
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits = s
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit() || *c == '.')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let number: f32 = s[..digits].parse().ok()?;
    let rest = &s[digits..];
    let (scale, rest) = if let Some(r) = rest.strip_prefix("mm") {
        (72.0 / 25.4, r)
    } else if let Some(r) = rest.strip_prefix("cm") {
        (72.0 / 2.54, r)
    } else if let Some(r) = rest.strip_prefix("in") {
        (72.0, r)
    } else if let Some(r) = rest.strip_prefix('"') {
        (72.0, r)
    } else if let Some(r) = rest.strip_prefix("pt") {
        (1.0, r)
    } else {
        (1.0, rest)
    };
    Some((sign * number * scale, rest))
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

    #[test]
    fn pictures_come_out_as_jpeg_or_png() {
        let (bytes, ext) = picture_bytes(PictureFile::Jpeg(vec![0xff, 0xd8, 0xff])).unwrap();
        assert_eq!((bytes, ext), (vec![0xff, 0xd8, 0xff], "jpg"));
        // Two 1-bit rows of three pixels: on, off, on; off, on, off.
        let (bytes, ext) = picture_bytes(PictureFile::Samples {
            width: 3,
            height: 2,
            components: 1,
            bits: 1,
            data: vec![0b1010_0000, 0b0100_0000],
        })
        .unwrap();
        assert_eq!(ext, "png");
        let image = image::load_from_memory(&bytes).unwrap().into_luma8();
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(0, 0).0, [255]);
        assert_eq!(image.get_pixel(1, 0).0, [0]);
        assert_eq!(image.get_pixel(1, 1).0, [255]);
        // CMYK pure cyan comes out as RGB cyan; black as black.
        let (bytes, _) = picture_bytes(PictureFile::Samples {
            width: 2,
            height: 1,
            components: 4,
            bits: 8,
            data: vec![255, 0, 0, 0, 0, 0, 0, 255],
        })
        .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().into_rgb8();
        assert_eq!(image.get_pixel(0, 0).0, [0, 255, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 0]);
    }

    #[test]
    fn lengths_are_typed_with_units_and_sums() {
        assert_eq!(length("12"), Some(12.0));
        assert_eq!(length("12,5"), Some(12.5));
        assert_eq!(length("1in"), Some(72.0));
        assert_eq!(length("2.54cm"), Some(72.0));
        assert_eq!(length("25.4 mm"), Some(72.0));
        assert_eq!(length("10pt+1in"), Some(82.0));
        assert_eq!(length("1in-10"), Some(62.0));
        assert_eq!(length("2*36"), Some(72.0));
        assert_eq!(length("144/2"), Some(72.0));
        assert_eq!(length("1in+2*10"), Some(92.0));
        assert_eq!(length("-5"), Some(-5.0));
        assert_eq!(length("abc"), None);
        assert_eq!(length("10/0"), None);
        assert_eq!(length(""), None);
    }
}
