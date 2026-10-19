// SPDX-License-Identifier: GPL-3.0-or-later

//! Measuring (docs/PLAN.md §4.1 and §2.5): with the distance, perimeter
//! and area tools from the annotation row, lines are drawn over the page,
//! snapping to the ends, middles and crossings of the lines the page
//! draws (`katna_pdf_render::measure`), and kept as measurement
//! annotations that show their value (`katna_pdf_cos::Scale`). The scale
//! is the file's own when its drawing says (a page's viewport), paper
//! millimetres otherwise, or set by hand from a line of known length.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, Scale};
use katna_pdf_render::measure::{self, SnapKind, Snaps};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::{Content, Viewer};
use crate::annotate::{COLOURS, Tool};
use crate::text::t;

/// How near, in pixels on screen, the pointer snaps to a line.
const SNAP: f32 = 8.0;
/// A press and release closer than this, in points, is a click.
const CLICK: f32 = 3.0;
/// Measurements in the first colour (yellow, hard to read on paper)
/// are drawn blue.
const BLUE: [f32; 3] = [0.1, 0.45, 0.9];

/// A measurement being drawn.
pub(super) struct Measuring {
    page: usize,
    /// The points placed, in points from the page's top left.
    points: Vec<[f32; 2]>,
    /// Still pressed since the first point (a distance dragged out).
    pressed: bool,
}

/// What the measuring tools know of the file.
#[derive(Default)]
pub(super) struct Measures {
    /// Each page's lines to snap to, once read; `None` while reading.
    snaps: HashMap<usize, Option<Arc<Snaps>>>,
    /// Each page's own scale, as its file says, once read.
    scales: Option<Vec<Option<Scale>>>,
    /// The scale set by hand, for every page.
    set: Option<Scale>,
    /// The pointer: its page, the point (snapped), and what it snapped to.
    pointer: Option<(usize, [f32; 2], Option<SnapKind>)>,
    /// The length in points of the last distance measured, to set the
    /// scale from.
    last: Option<f64>,
    /// The field the real length of the last distance is typed in.
    calibrating: Option<(Entity<TextInput>, Subscription)>,
}

impl Viewer {
    fn measure_tool(&self) -> Option<Tool> {
        self.tool.filter(|t| t.measures())
    }

    /// Gets ready to measure: the lines of the pages on screen, and
    /// each page's scale, read.
    pub(super) fn start_measuring(&mut self, cx: &mut Context<Self>) {
        for page in self.visible.clone() {
            self.want_snaps(page, cx);
        }
        self.read_scales(cx);
    }

    /// Reads each page's scale from the file, once.
    fn read_scales(&mut self, cx: &mut Context<Self>) {
        if self.measures.scales.is_some() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        self.measures.scales = Some(Vec::new());
        let key = self.key.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let scales = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path).ok()?;
                    let doc = super::cos(bytes, key.as_deref()).ok()?;
                    Some(
                        (0..doc.pages().len())
                            .map(|p| doc.page_scale(p))
                            .collect::<Vec<_>>(),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.measures.scales = Some(scales.unwrap_or_default());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The scale page `page` is measured in, and where it comes from.
    fn scale(&self, page: usize) -> (Scale, &'static str) {
        if let Some(set) = &self.measures.set {
            return (set.clone(), "measure-scale-set");
        }
        match self
            .measures
            .scales
            .as_ref()
            .and_then(|s| s.get(page)?.clone())
        {
            Some(scale) => (scale, "measure-scale-file"),
            None => (Scale::default(), "measure-scale-paper"),
        }
    }

    /// Reads page `page`'s lines to snap to, once.
    fn want_snaps(&mut self, page: usize, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        if self.measures.snaps.contains_key(&page) {
            return;
        }
        self.measures.snaps.insert(page, None);
        let doc = opened.doc.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let snaps = cx
                .background_executor()
                .spawn(async move { Snaps::new(doc.segments(page)) })
                .await;
            this.update(cx, |this, _| {
                if this.generation == generation {
                    this.measures.snaps.insert(page, Some(Arc::new(snaps)));
                }
            })
            .ok();
        })
        .detach();
    }

    /// The point on a page under `position`, snapped to the page's lines.
    fn measure_point(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<(usize, [f32; 2], Option<SnapKind>)> {
        let (page, point) = self.page_point(position)?;
        self.want_snaps(page, cx);
        let radius = SNAP / self.shown.max(0.01);
        let snapped = self
            .measures
            .snaps
            .get(&page)
            .and_then(|s| s.as_ref()?.snap(point, radius));
        Some(match snapped {
            Some((p, kind)) => (page, p, Some(kind)),
            None => (page, point, None),
        })
    }

    /// A press with a measuring tool: places a point, or finishes.
    pub(super) fn measure_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tool) = self.measure_tool() else {
            return false;
        };
        self.choose(None, cx);
        self.selection = None;
        let Some((page, point, kind)) = self.measure_point(event.position, cx) else {
            return true;
        };
        self.measures.pointer = Some((page, point, kind));
        let radius = SNAP / self.shown.max(0.01);
        match self.measuring.as_mut().filter(|m| m.page == page) {
            None => {
                self.measuring = Some(Measuring {
                    page,
                    points: vec![point],
                    pressed: true,
                });
            }
            // A distance's second click.
            Some(m) if tool == Tool::Distance => {
                let points = vec![m.points[0], point];
                self.finish_measure(tool, page, points, cx);
            }
            Some(m) => {
                let closes =
                    tool == Tool::Area && m.points.len() >= 3 && dist(m.points[0], point) <= radius;
                if event.click_count >= 2 || closes {
                    let points = std::mem::take(&mut m.points);
                    self.finish_measure(tool, page, points, cx);
                } else {
                    m.points.push(point);
                }
            }
        }
        cx.notify();
        true
    }

    /// The pointer moved with a measuring tool in hand.
    pub(super) fn measure_move(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.measure_tool().is_none() {
            return false;
        }
        let at = self.measure_point(position, cx);
        if at != self.measures.pointer {
            self.measures.pointer = at;
            cx.notify();
        }
        true
    }

    /// Let go with a measuring tool: a distance dragged out is done.
    pub(super) fn measure_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(tool) = self.measure_tool() else {
            return false;
        };
        let Some(m) = self.measuring.as_mut() else {
            return true;
        };
        let was_pressed = std::mem::replace(&mut m.pressed, false);
        if tool == Tool::Distance
            && was_pressed
            && let Some((page, point, _)) = self.measures.pointer
            && page == m.page
            && dist(m.points[0], point) > CLICK
        {
            let points = vec![m.points[0], point];
            self.finish_measure(tool, page, points, cx);
        }
        true
    }

    /// Keeps what was measured as an annotation on the page.
    fn finish_measure(
        &mut self,
        tool: Tool,
        page: usize,
        points: Vec<[f32; 2]>,
        cx: &mut Context<Self>,
    ) {
        self.measuring = None;
        let Some(inv) = self.inverse(page) else {
            return;
        };
        let (scale, _) = self.scale(page);
        let (kind, value) = match tool {
            Tool::Distance if points.len() == 2 => {
                let len = measure::length(&points);
                self.measures.last = Some(len);
                (
                    AnnotKind::Line {
                        from: inv.map(points[0]),
                        to: inv.map(points[1]),
                        arrow: false,
                    },
                    scale.length(len),
                )
            }
            Tool::Perimeter if points.len() >= 2 => (
                AnnotKind::PolyLine(points.iter().map(|p| inv.map(*p)).collect()),
                scale.length(measure::length(&points)),
            ),
            Tool::Area if points.len() >= 3 => (
                AnnotKind::Polygon(points.iter().map(|p| inv.map(*p)).collect()),
                scale.area(measure::area(&points)),
            ),
            _ => return,
        };
        let mut a = Annotation::new(page, kind);
        a.colour = match self.colour {
            0 => BLUE,
            i => COLOURS[i],
        };
        a.width = 1.0;
        a.contents = value;
        a.measure = Some(scale);
        a.author = super::annots::author();
        a.rect = a.bounds();
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// The measurement being drawn, the pointer's snap, and the value so
    /// far, over page `page`.
    pub(super) fn measure_overlay(&self, page: usize) -> Vec<AnyElement> {
        let Some(tool) = self.measure_tool() else {
            return Vec::new();
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent;
        let mut out = Vec::new();
        let pointer = self.measures.pointer.filter(|(p, ..)| *p == page);
        if let Some(m) = self.measuring.as_ref().filter(|m| m.page == page) {
            let mut points = m.points.clone();
            if let Some((_, at, _)) = pointer {
                points.push(at);
            }
            let closed = tool == Tool::Area && points.len() > 2;
            let line = points.clone();
            out.push(
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let mut builder = gpui::PathBuilder::stroke(desktop_px(1.5));
                        for (i, p) in line.iter().enumerate() {
                            let at = bounds.origin
                                + gpui::point(desktop_px(p[0] * shown), desktop_px(p[1] * shown));
                            if i == 0 {
                                builder.move_to(at);
                            } else {
                                builder.line_to(at);
                            }
                        }
                        if closed {
                            builder.close();
                        }
                        if line.len() > 1
                            && let Ok(path) = builder.build()
                        {
                            window.paint_path(path, gpui::rgba(accent));
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .into_any_element(),
            );
            // The value so far, by the pointer.
            if let Some(last) = points.last().copied() {
                let (scale, _) = self.scale(page);
                let value = match tool {
                    Tool::Area if points.len() > 2 => scale.area(measure::area(&points)),
                    _ => scale.length(measure::length(&points)),
                };
                out.push(
                    div()
                        .absolute()
                        .left(desktop_px(last[0] * shown + 12.0))
                        .top(desktop_px(last[1] * shown + 12.0))
                        .px(px(space::S2))
                        .py(px(space::S1))
                        .rounded(px(radius::XS))
                        .bg(self.theme.card)
                        .border_1()
                        .border_color(self.theme.line)
                        .text_size(px(text::SMALL))
                        .text_color(self.theme.text)
                        .whitespace_nowrap()
                        .child(value)
                        .into_any_element(),
                );
            }
        }
        // Where the pointer snapped: a small square on the point.
        if let Some((_, at, Some(_))) = pointer {
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(at[0] * shown - 5.0))
                    .top(desktop_px(at[1] * shown - 5.0))
                    .size(desktop_px(10.0))
                    .border_2()
                    .border_color(gpui::rgba(accent))
                    .into_any_element(),
            );
        }
        out
    }

    fn start_calibrating(&mut self, cx: &mut Context<Self>) {
        if self.measures.last.is_none() {
            return;
        }
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("measure-real-length", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.calibrate(cx),
            InputEvent::Cancel => {
                this.measures.calibrating = None;
                cx.notify();
            }
            _ => {}
        });
        self.measures.calibrating = Some((input, sub));
        self.focus_calibrating = true;
        cx.notify();
    }

    /// Sets the scale so the last distance measured is the length typed.
    fn calibrate(&mut self, cx: &mut Context<Self>) {
        let (Some((input, _)), Some(last)) = (&self.measures.calibrating, self.measures.last)
        else {
            return;
        };
        let typed = input.read(cx).text().to_string();
        let page = self.measures.pointer.map_or(0, |(p, ..)| p);
        let (now, _) = self.scale(page);
        match parse_length(&typed, &now.unit)
            .and_then(|(v, unit)| Scale::calibrated(last, v, &unit))
        {
            Some(scale) => {
                self.measures.set = Some(scale);
                self.measures.calibrating = None;
                self.notice = None;
            }
            None => self.notice = Some(t("measure-real-length-bad", &[("text", &typed)])),
        }
        cx.notify();
    }

    pub(super) fn focus_calibrating(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.focus_calibrating)
            && let Some((input, _)) = &self.measures.calibrating
        {
            let handle = input.focus_handle(cx);
            window.focus(&handle, cx);
        }
    }

    /// The bar under the tools while measuring: the scale, where the
    /// pointer is, and setting the scale.
    pub(super) fn measure_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tool = self.measure_tool()?;
        let th = self.theme;
        let page = self
            .measures
            .pointer
            .map(|(p, ..)| p)
            .unwrap_or_else(|| self.current_page());
        let (scale, source) = self.scale(page);
        let dim = |s: String| {
            div()
                .flex_none()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .whitespace_nowrap()
                .child(s)
        };
        let mut bar = div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(space::S4))
            .px(px(space::S4))
            .border_b_1()
            .border_color(th.line)
            .child(dim(t(
                match tool {
                    Tool::Distance => "measure-how-distance",
                    Tool::Perimeter => "measure-how-perimeter",
                    _ => "measure-how-area",
                },
                &[],
            )))
            .child(dim(t(
                source,
                &[("scale", &scale.length(1.0).replace(' ', "\u{a0}"))],
            )));
        if let Some((_, at, _)) = self.measures.pointer {
            let x = scale.length(f64::from(at[0]));
            let y = scale.length(f64::from(at[1]));
            bar = bar.child(dim(t("measure-at", &[("x", &x), ("y", &y)])));
        }
        bar = bar.child(div().flex_1());
        match &self.measures.calibrating {
            Some((input, _)) => {
                bar = bar.child(dim(t("measure-last-is", &[]))).child(
                    div()
                        .w(px(120.0))
                        .h(px(30.0))
                        .flex()
                        .items_center()
                        .px(px(space::S3))
                        .rounded(px(radius::SM))
                        .bg(th.field)
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(input.clone()),
                )
            }
            None if self.measures.last.is_some() => {
                bar = bar.child(
                    widgets::outlined_button(
                        "measure-set-scale",
                        t("measure-set-scale", &[]),
                        &th.katna,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.start_calibrating(cx))),
                );
            }
            None => {}
        }
        if self.measures.set.is_some() {
            bar = bar.child(
                widgets::outlined_button(
                    "measure-unset-scale",
                    t("measure-unset-scale", &[]),
                    &th.katna,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.measures.set = None;
                    cx.notify();
                })),
            );
        }
        Some(bar.into_any_element())
    }
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A length as typed: a number and a unit (`5 m`, `12.5ft`, `300`), the
/// unit `unit` when none is given.
fn parse_length(s: &str, unit: &str) -> Option<(f64, String)> {
    let s = s.trim().replace(',', ".");
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let value: f64 = s[..end].parse().ok()?;
    let typed = s[end..].trim();
    let unit = if typed.is_empty() { unit } else { typed };
    (unit.chars().count() <= 8).then(|| (value, unit.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::parse_length;

    #[test]
    fn lengths_are_read_as_typed() {
        assert_eq!(parse_length("5 m", "mm"), Some((5.0, "m".into())));
        assert_eq!(parse_length("12,5ft", "mm"), Some((12.5, "ft".into())));
        assert_eq!(parse_length("300", "ft"), Some((300.0, "ft".into())));
        assert_eq!(parse_length("m", "mm"), None);
    }
}
