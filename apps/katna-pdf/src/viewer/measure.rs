// SPDX-License-Identifier: GPL-3.0-or-later

//! Measuring (docs/PLAN.md §4.1 and §2.5): with the distance, perimeter
//! and area tools from the annotation row, lines are drawn over the page,
//! snapping to the ends, middles and crossings of the lines the page
//! draws (`katna_pdf_render::measure`), and kept as measurement
//! annotations that show their value (`katna_pdf_cos::Scale`). The scale
//! is the file's own when its drawing says (a page's viewport), paper
//! millimetres otherwise, or set by hand from a line of known length.
//! The measure card (C06) floats at the top right while measuring: the
//! scale as paper to real length, where it comes from, what to snap to,
//! and the three tools.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, Scale};
use katna_pdf_render::measure::{self, SnapKind, SnapTo, Snaps};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};

use super::{Content, Viewer};
use crate::annotate::{COLOURS, Tool};
use crate::float;
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
    /// What the pointer snaps to.
    snap_to: SnapTo,
    /// The measure card's scale: millimetres on paper, and the real
    /// length they are.
    fields: Option<ScaleFields>,
}

pub(super) struct ScaleFields {
    paper: Entity<TextInput>,
    real: Entity<TextInput>,
    /// Set while the fields are filled in from the scale, so filling
    /// isn't taken as typing.
    filling: bool,
    _subs: [Subscription; 2],
}

/// Millimetres in one of a length unit people measure drawings in.
fn millimetres_in(unit: &str) -> Option<f64> {
    Some(match unit {
        "mm" => 1.0,
        "cm" => 10.0,
        "m" => 1000.0,
        "km" => 1_000_000.0,
        "in" => 25.4,
        "ft" => 304.8,
        "yd" => 914.4,
        _ => return None,
    })
}

/// The scale as a drawing's ratio, `1:100`, when its unit is a length.
fn ratio(scale: &Scale) -> Option<String> {
    let real = scale.factor * millimetres_in(&scale.unit)?;
    let n = real / (25.4 / 72.0);
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    Some(if n >= 1.0 {
        format!("1:{}", (n * 100.0).round() / 100.0)
    } else {
        format!("{}:1", (100.0 / n).round() / 100.0)
    })
}

impl Viewer {
    /// The tool in hand when it takes points a click at a time: the
    /// measuring tools, and polygons and clouds.
    fn measure_tool(&self) -> Option<Tool> {
        self.tool.filter(|t| t.clicks_points())
    }

    /// Gets ready to measure: the lines of the pages on screen, and
    /// each page's scale, read.
    pub(super) fn start_measuring(&mut self, cx: &mut Context<Self>) {
        for page in self.visible.clone() {
            self.want_snaps(page, cx);
        }
        self.read_scales(cx);
        if self.measures.fields.is_none() {
            let accent = gpui::rgba(self.theme.katna.accent).into();
            let field = |cx: &mut Context<Self>| {
                cx.new(|cx| {
                    let mut input = TextInput::new("", cx);
                    input.set_accent(accent);
                    input
                })
            };
            let paper = field(cx);
            let real = field(cx);
            let typed = |this: &mut Self, _, event: &InputEvent, cx: &mut Context<Self>| {
                if matches!(event, InputEvent::Changed | InputEvent::Submit) {
                    this.scale_typed(cx);
                }
            };
            let subs = [cx.subscribe(&paper, typed), cx.subscribe(&real, typed)];
            self.measures.fields = Some(ScaleFields {
                paper,
                real,
                filling: false,
                _subs: subs,
            });
        }
        self.fill_scale(cx);
    }

    /// Fills the card's scale in from the scale pages are measured in.
    fn fill_scale(&mut self, cx: &mut Context<Self>) {
        let (scale, _) = self.scale(self.current_page());
        let Some(fields) = self.measures.fields.as_mut() else {
            return;
        };
        fields.filling = true;
        let (paper, real) = (fields.paper.clone(), fields.real.clone());
        paper.update(cx, |input, cx| input.set_text("1", cx));
        real.update(cx, |input, cx| {
            input.set_text(scale.length(72.0 / 25.4), cx)
        });
        if let Some(fields) = self.measures.fields.as_mut() {
            fields.filling = false;
        }
    }

    /// The scale typed in the card, once both fields make sense.
    fn scale_typed(&mut self, cx: &mut Context<Self>) {
        let Some(fields) = &self.measures.fields else {
            return;
        };
        if fields.filling {
            return;
        }
        let paper = fields.paper.read(cx).text().trim().replace(',', ".");
        let real = fields.real.read(cx).text().to_string();
        let (now, _) = self.scale(self.current_page());
        let Ok(paper) = paper.parse::<f64>() else {
            return;
        };
        if let Some(scale) = parse_length(&real, &now.unit)
            .and_then(|(v, unit)| Scale::calibrated(paper * 72.0 / 25.4, v, &unit))
            && scale != now
        {
            self.measures.set = Some(scale);
            cx.notify();
        }
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
                    if this.measures.set.is_none() {
                        this.fill_scale(cx);
                    }
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
        let to = self.measures.snap_to;
        let snapped = self
            .measures
            .snaps
            .get(&page)
            .and_then(|s| s.as_ref()?.snap_to(point, radius, to));
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
                let closes = matches!(tool, Tool::Area | Tool::Polygon | Tool::Cloud)
                    && m.points.len() >= 3
                    && dist(m.points[0], point) <= radius;
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
        // A polygon, cloud or polyline is a shape, not a measurement.
        if matches!(tool, Tool::Polygon | Tool::Cloud | Tool::Polyline) {
            let open = tool == Tool::Polyline;
            if points.len() < if open { 2 } else { 3 } {
                return;
            }
            let on_page = points.iter().map(|p| inv.map(*p)).collect();
            let mut a = Annotation::new(
                page,
                if open {
                    AnnotKind::PolyLine(on_page)
                } else {
                    AnnotKind::Polygon(on_page)
                },
            );
            a.colour = COLOURS[self.colour];
            a.width = super::super::annotate::WIDTH;
            a.cloudy = tool == Tool::Cloud;
            a.author = super::annots::author();
            a.rect = a.bounds();
            self.change_annotations(
                AnnotChanges {
                    put: vec![a],
                    remove: Vec::new(),
                },
                cx,
            );
            return;
        }
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
            let closed =
                matches!(tool, Tool::Area | Tool::Polygon | Tool::Cloud) && points.len() > 2;
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
            if let Some(last) = points.last().copied()
                && tool.measures()
            {
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
                self.fill_scale(cx);
            }
            None => self.notice = Some(super::said("measure-real-length-bad", &[("text", &typed)])),
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

    /// The bar under the tools while drawing a polygon or cloud: how.
    pub(super) fn measure_bar(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        let tool = self.measure_tool().filter(|t| !t.measures())?;
        let th = self.theme;
        Some(
            div()
                .h(px(40.0))
                .flex()
                .items_center()
                .px(px(space::S4))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .whitespace_nowrap()
                .child(t(
                    match tool {
                        Tool::Distance => "measure-how-distance",
                        Tool::Perimeter => "measure-how-perimeter",
                        Tool::Polygon | Tool::Cloud => "measure-how-shape",
                        Tool::Polyline => "measure-how-polyline",
                        _ => "measure-how-area",
                    },
                    &[],
                ))
                .into_any_element(),
        )
    }

    /// The measure card (C06) at the top right while measuring.
    pub(super) fn measure_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tool = self.measure_tool().filter(|t| t.measures())?;
        let fields = self.measures.fields.as_ref()?;
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
                .text_size(px(text::CAPTION))
                .text_color(th.dim)
                .whitespace_nowrap()
                .child(s)
        };
        let field = |input: &Entity<TextInput>, width: f32| {
            div()
                .w(px(width))
                .h(px(30.0))
                .flex_none()
                .flex()
                .items_center()
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .bg(th.field)
                .border_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .child(input.clone())
        };
        let ratio = ratio(&scale).unwrap_or_default();
        let from = div()
            .text_size(px(text::CAPTION))
            .text_color(if source == "measure-scale-file" {
                gpui::rgba(th.valid).into()
            } else {
                th.dim
            })
            .child(t(
                match source {
                    "measure-scale-file" => "measure-from-file",
                    "measure-scale-set" => "measure-from-you",
                    _ => "measure-from-paper",
                },
                &[("ratio", &ratio)],
            ));
        let mut by_hand = div().flex().flex_wrap().items_center().gap(px(space::S2));
        match &self.measures.calibrating {
            Some((input, _)) => {
                by_hand = by_hand
                    .child(dim(t("measure-last-is", &[])))
                    .child(field(input, 110.0));
            }
            None if self.measures.last.is_some() => {
                by_hand = by_hand.child(
                    float::chip("measure-set-scale", t("measure-set-scale", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.start_calibrating(cx))),
                );
            }
            None => {}
        }
        if self.measures.set.is_some() {
            by_hand = by_hand.child(
                float::chip(
                    "measure-unset-scale",
                    t("measure-unset-scale", &[]),
                    false,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.measures.set = None;
                    this.fill_scale(cx);
                    cx.notify();
                })),
            );
        }
        let to = self.measures.snap_to;
        let snap_chip = |id: &'static str, on: bool, change: fn(&mut SnapTo)| {
            float::chip(id, t(id, &[]), on, &th).on_click(cx.listener(move |this, _, _, cx| {
                change(&mut this.measures.snap_to);
                cx.notify();
            }))
        };
        let snaps = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S2))
            .child(snap_chip("measure-snap-ends", to.ends, |s| {
                s.ends = !s.ends
            }))
            .child(snap_chip("measure-snap-middles", to.middles, |s| {
                s.middles = !s.middles
            }))
            .child(snap_chip("measure-snap-crossings", to.crossings, |s| {
                s.crossings = !s.crossings
            }))
            .child(snap_chip("measure-snap-off", to == SnapTo::OFF, |s| {
                *s = if *s == SnapTo::OFF {
                    SnapTo::default()
                } else {
                    SnapTo::OFF
                }
            }));
        let tools = [Tool::Distance, Tool::Perimeter, Tool::Area].map(|each| {
            float::seg_item(
                ("measure-tool", each as usize),
                None,
                Some(t(each.name(), &[]).into()),
                each == tool,
                &th,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.tool != Some(each) {
                    this.pick_tool(each, cx);
                }
            }))
        });
        let mut foot = vec![t(
            match tool {
                Tool::Distance => "measure-how-distance",
                Tool::Perimeter => "measure-how-perimeter",
                _ => "measure-how-area",
            },
            &[],
        )];
        if let Some((_, at, _)) = self.measures.pointer {
            let x = scale.length(f64::from(at[0]));
            let y = scale.length(f64::from(at[1]));
            foot.push(t("measure-at", &[("x", &x), ("y", &y)]));
        }
        Some(
            div()
                .id("measure-card")
                .occlude()
                .absolute()
                .top(px(space::S2))
                .right(px(float::GAP))
                .w(px(300.0))
                .p(px(space::S3))
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .rounded(px(10.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("measure-scale", &[])),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(field(&fields.paper, 52.0))
                        .child(dim(t("measure-paper-is", &[])))
                        .child(field(&fields.real, 96.0)),
                )
                .child(from)
                .child(by_hand)
                .child(
                    div()
                        .mt(px(space::S1))
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("measure-snap", &[])),
                )
                .child(snaps)
                .child(
                    div()
                        .mt(px(space::S1))
                        .child(float::seg(&th).children(tools)),
                )
                .children(foot.into_iter().map(|line| {
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(line)
                }))
                .into_any_element(),
        )
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
    use super::{Scale, parse_length, ratio};

    #[test]
    fn lengths_are_read_as_typed() {
        assert_eq!(parse_length("5 m", "mm"), Some((5.0, "m".into())));
        assert_eq!(parse_length("12,5ft", "mm"), Some((12.5, "ft".into())));
        assert_eq!(parse_length("300", "ft"), Some((300.0, "ft".into())));
        assert_eq!(parse_length("m", "mm"), None);
    }

    #[test]
    fn scales_read_as_a_drawings_ratio() {
        assert_eq!(ratio(&Scale::default()).as_deref(), Some("1:1"));
        let hundred = Scale::calibrated(72.0 / 25.4, 0.1, "m").unwrap();
        assert_eq!(ratio(&hundred).as_deref(), Some("1:100"));
        let twice = Scale::calibrated(72.0 / 25.4, 0.5, "mm").unwrap();
        assert_eq!(ratio(&twice).as_deref(), Some("2:1"));
        let odd = Scale::calibrated(1.0, 3.0, "px").unwrap();
        assert_eq!(ratio(&odd), None);
    }
}
