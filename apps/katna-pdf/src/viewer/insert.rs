// SPDX-License-Identifier: GPL-3.0-or-later

//! Adding to the pages (docs/PLAN.md §4.3, Phase 4): with Edit text on, a
//! row of tools puts new text, pictures, rectangles, ellipses and lines
//! into a page's own content (`katna_pdf_cos::insert`), drawn as the
//! page's own text and pictures are, not as annotations. Text is typed
//! in place where the page is clicked; a picture comes from a file, or
//! from the clipboard with Ctrl+V; shapes are dragged out.

use gpui::{AnyElement, Context, Window, div, prelude::*};
use katna_pdf_cos::{Insert, ShapeKind};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::tokens::space;
use katna_ui::widgets::{self, Tip};

use super::annots::Draft;
use super::textedit::Target;
use super::{Content, Viewer, separator};
use crate::text::t;

/// A tool for adding to a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Adding {
    Text,
    Picture,
    Rectangle,
    Ellipse,
    Line,
}

impl Adding {
    const ALL: [Adding; 5] = [
        Adding::Text,
        Adding::Picture,
        Adding::Rectangle,
        Adding::Ellipse,
        Adding::Line,
    ];

    fn icon(self) -> &'static str {
        match self {
            Adding::Text => "format-text",
            Adding::Picture => "image",
            Adding::Rectangle => "shape-square",
            Adding::Ellipse => "shape-circle",
            Adding::Line => "shape-line",
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Adding::Text => "add-text",
            Adding::Picture => "add-picture",
            Adding::Rectangle => "add-rectangle",
            Adding::Ellipse => "add-ellipse",
            Adding::Line => "add-line",
        }
    }
}

/// The colours to add in, red, green, blue: black, red, blue, green,
/// orange, white.
pub(super) const INKS: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],
    [0.85, 0.1, 0.1],
    [0.1, 0.35, 0.9],
    [0.1, 0.6, 0.25],
    [1.0, 0.6, 0.0],
    [1.0, 1.0, 1.0],
];
/// Size of text added, in points.
pub(super) const TEXT_SIZE: f32 = 12.0;
/// Its font, one of the standard fonts every reader has.
const FONT: &str = "Helvetica";
/// Line width of shapes, in points.
const LINE_WIDTH: f32 = 2.0;
/// Widest a picture placed with a click is, in points.
const PICTURE_WIDTH: f32 = 200.0;
/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

pub(super) fn colour([r, g, b]: [f32; 3]) -> gpui::Rgba {
    gpui::rgb(u32::from_be_bytes([
        0,
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
    ]))
}

impl Viewer {
    /// The colour picked to add in, as the screen shows it.
    pub(super) fn ink_colour(&self) -> gpui::Rgba {
        colour(INKS[self.ink])
    }

    /// The row of tools for adding, under the toolbar while Edit text is
    /// on: the pointer (to edit paragraphs), the tools, and colours.
    pub(super) fn edit_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.text_editing || !matches!(self.content, Content::Ready(_)) {
            return None;
        }
        let th = self.theme;
        let mut row = div()
            .id("edit-tools")
            .h(px(44.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S3))
            .border_b_1()
            .border_color(th.line)
            .overflow_x_scroll();
        let button = |id: usize, icon: &'static str, tip: &'static str, on: bool| {
            let tint = if on {
                th.katna.accent
            } else {
                th.katna.text_dim
            };
            widgets::icon_button_colored(("edit-tool", id), icon, 18.0, tint, &th.katna)
                .when(on, |b| b.bg(th.field))
                .tip(t(tip, &[]), &th.katna)
        };
        row = row.child(
            button(0, "text-edit", "edit-paragraphs", self.adding.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.pick_adding(None, cx))),
        );
        row = row.child(separator(th));
        for (i, adding) in Adding::ALL.into_iter().enumerate() {
            row = row.child(
                button(
                    i + 1,
                    adding.icon(),
                    adding.tip(),
                    self.adding == Some(adding),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_adding(Some(adding), cx))),
            );
        }
        row = row.child(
            button(
                Adding::ALL.len() + 1,
                "water-drop",
                "page-marks",
                self.marking.is_some(),
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle_marking(cx))),
        );
        row = row.child(separator(th));
        for (i, ink) in INKS.into_iter().enumerate() {
            let on = self.ink == i;
            row = row.child(
                div()
                    .id(("ink", i))
                    .size(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.border_2().border_color(gpui::rgba(th.katna.accent))
                    })
                    .hover(move |s| s.bg(th.hover))
                    .tip(t("tool-colour", &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ink = i;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(18.0))
                            .rounded_full()
                            .border_1()
                            .border_color(th.line)
                            .bg(colour(ink)),
                    ),
            );
        }
        if let Some(inspector) = self.inspector(cx) {
            row = row.child(separator(th)).child(inspector);
        }
        Some(row.into_any_element())
    }

    pub(super) fn pick_adding(&mut self, adding: Option<Adding>, cx: &mut Context<Self>) {
        self.finish_text_edit(true, cx);
        self.adding = adding;
        self.placing = None;
        self.picked_object = None;
        self.moving = None;
        cx.notify();
    }

    /// A press on the pages with a tool for adding in hand: text starts
    /// being typed there, the rest start being dragged out. Returns
    /// whether it did.
    pub(super) fn insert_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.text_editing {
            return false;
        }
        let Some(adding) = self.adding else {
            return false;
        };
        self.finish_text_edit(true, cx);
        let Some((page, point)) = self.page_point(position) else {
            return true;
        };
        if adding == Adding::Text {
            self.start_typing(page, Target::New(point), String::new(), window, cx);
        } else {
            self.placing = Some(Draft {
                page,
                path: vec![point],
            });
        }
        cx.notify();
        true
    }

    pub(super) fn insert_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self.placing.as_ref().map(|d| d.page) else {
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
        if let Some(draft) = self.placing.as_mut() {
            draft.path.truncate(1);
            draft.path.push(point);
        }
        cx.notify();
        true
    }

    /// The pointer let go: the shape dragged out is drawn into the page,
    /// or a picture is asked for to put where it was. Returns whether it
    /// did either.
    pub(super) fn insert_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(draft) = self.placing.take() else {
            return false;
        };
        cx.notify();
        let (Some(adding), Some(inv)) = (self.adding, self.inverse(draft.page)) else {
            return true;
        };
        let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last()) else {
            return true;
        };
        let click = (to[0] - from[0]).hypot(to[1] - from[1]) < CLICK;
        let boxed = [
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]),
            from[1].max(to[1]),
        ];
        let ink = INKS[self.ink];
        let shape = |kind, rect| Insert::Shape {
            kind,
            rect,
            stroke: Some(ink),
            fill: None,
            width: LINE_WIDTH,
        };
        let item = match adding {
            Adding::Text => return true,
            Adding::Picture => {
                self.choose_picture_to_add(draft.page, (!click).then_some(boxed), from, cx);
                return true;
            }
            _ if click => return true,
            Adding::Rectangle => shape(ShapeKind::Rectangle, inv.rect(boxed)),
            Adding::Ellipse => shape(ShapeKind::Ellipse, inv.rect(boxed)),
            Adding::Line => {
                let (a, b) = (inv.map(from), inv.map(to));
                shape(ShapeKind::Line, [a[0], a[1], b[0], b[1]])
            }
        };
        self.add_to_page(draft.page, item, cx);
        true
    }

    /// Text typed at `at` (its box's top left on the page shown), drawn
    /// into the page.
    pub(super) fn write_new_text(
        &mut self,
        page: usize,
        at: [f32; 2],
        text: String,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim_end().to_owned();
        let Some(inv) = self.inverse(page).filter(|_| !text.trim().is_empty()) else {
            return;
        };
        // The first line's baseline, as the box shows it.
        let at = inv.map([at[0], at[1] + TEXT_SIZE * 0.95]);
        let item = Insert::Text {
            at,
            text,
            font: FONT.into(),
            size: TEXT_SIZE,
            colour: INKS[self.ink],
        };
        self.add_to_page(page, item, cx);
    }

    fn add_to_page(&mut self, page: usize, item: Insert, cx: &mut Context<Self>) {
        self.change_page(
            page,
            move |doc| doc.insert(page, &[item]).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Asks for a picture file to put in `boxed` on page `page` (a box on
    /// the page shown), or with its top left at `at` when no box was
    /// dragged out.
    fn choose_picture_to_add(
        &mut self,
        page: usize,
        boxed: Option<[f32; 4]>,
        at: [f32; 2],
        cx: &mut Context<Self>,
    ) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("add-picture-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                    katna_pdf_sign::page_image(&bytes)
                })
                .await;
            this.update(cx, |this, cx| match read {
                Ok(image) => this.add_picture(page, boxed, at, image, cx),
                Err(error) => {
                    this.notice = Some(t("text-edit-failed", &[("error", &error)]));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Puts `image` on page `page`: in `boxed` (on the page shown),
    /// keeping its shape, or with its top left at `at`, at most
    /// [`PICTURE_WIDTH`] wide.
    fn add_picture(
        &mut self,
        page: usize,
        boxed: Option<[f32; 4]>,
        at: [f32; 2],
        image: katna_pdf_cos::Image,
        cx: &mut Context<Self>,
    ) {
        let Some(inv) = self.inverse(page) else {
            return;
        };
        let (w, h) = match &image {
            katna_pdf_cos::Image::Pixels(p) => (p.width, p.height),
            katna_pdf_cos::Image::Jpeg { width, height, .. } => (*width, *height),
        };
        let aspect = h as f32 / w.max(1) as f32;
        let shown = match boxed {
            Some([l, t, r, b]) => {
                // As big as fits in the box.
                let (bw, bh) = (r - l, b - t);
                let (w, h) = if bh / bw > aspect {
                    (bw, bw * aspect)
                } else {
                    (bh / aspect, bh)
                };
                [l, t, l + w, t + h]
            }
            None => {
                // A pixel to a point, as 96 pixels to the inch.
                let w = (w as f32 * 0.75).min(PICTURE_WIDTH);
                [at[0], at[1], at[0] + w, at[1] + w * aspect]
            }
        };
        let item = Insert::Image {
            rect: inv.rect(shown),
            image,
        };
        self.add_to_page(page, item, cx);
    }

    /// Ctrl+V while Edit text is on: a picture on the clipboard goes on
    /// the page in the middle of the window. Returns whether there was
    /// one.
    pub(super) fn paste_picture(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.text_editing || self.typing.is_some() {
            return false;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let Some(bytes) = item.entries().iter().find_map(|e| match e {
            gpui::ClipboardEntry::Image(image) => Some(image.bytes.clone()),
            _ => None,
        }) else {
            return false;
        };
        let Some((page, middle)) = self.middle_point() else {
            return false;
        };
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { katna_pdf_sign::page_image(&bytes) })
                .await;
            this.update(cx, |this, cx| match read {
                Ok(image) => {
                    // Centred on the middle of the window.
                    let (w, h) = match &image {
                        katna_pdf_cos::Image::Pixels(p) => (p.width, p.height),
                        katna_pdf_cos::Image::Jpeg { width, height, .. } => (*width, *height),
                    };
                    let width = (w as f32 * 0.75).min(PICTURE_WIDTH);
                    let height = width * h as f32 / w.max(1) as f32;
                    let at = [middle[0] - width / 2.0, middle[1] - height / 2.0];
                    this.add_picture(page, None, at, image, cx);
                }
                Err(error) => {
                    this.notice = Some(t("text-edit-failed", &[("error", &error)]));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        true
    }

    /// The page being read, and a place near the top of the part of it
    /// shown, across the middle.
    fn middle_point(&self) -> Option<(usize, [f32; 2])> {
        let shown = desktop_px(self.shown);
        let page = self.current_page();
        let b = *self.page_bounds.borrow().get(&page)?;
        let (w, h) = (b.size.width / shown, b.size.height / shown);
        // Its part scrolled out of sight above, then a little way down.
        let hidden = (self.list.logical_scroll_top().offset_in_item / shown).clamp(0.0, h);
        Some((page, [w / 2.0, (hidden + 200.0).min(h * 0.9)]))
    }

    /// The shape being dragged out on page `page`, as it will be.
    pub(super) fn insert_preview(&self, page: usize) -> Vec<AnyElement> {
        let Some(draft) = self.placing.as_ref().filter(|d| d.page == page) else {
            return Vec::new();
        };
        let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last()) else {
            return Vec::new();
        };
        let shown = self.shown;
        let ink = self.ink_colour();
        if self.adding == Some(Adding::Line) {
            return vec![
                gpui::canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let mut builder = gpui::PathBuilder::stroke(desktop_px(LINE_WIDTH * shown));
                        let at = |p: [f32; 2]| {
                            bounds.origin
                                + gpui::point(desktop_px(p[0] * shown), desktop_px(p[1] * shown))
                        };
                        builder.move_to(at(from));
                        builder.line_to(at(to));
                        if let Ok(path) = builder.build() {
                            window.paint_path(path, ink);
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .into_any_element(),
            ];
        }
        let (x0, y0) = (from[0].min(to[0]), from[1].min(to[1]));
        let (x1, y1) = (from[0].max(to[0]), from[1].max(to[1]));
        vec![
            div()
                .absolute()
                .left(desktop_px(x0 * shown))
                .top(desktop_px(y0 * shown))
                .w(desktop_px((x1 - x0) * shown))
                .h(desktop_px((y1 - y0) * shown))
                .border_2()
                .border_color(ink)
                .when(self.adding == Some(Adding::Ellipse), |d| d.rounded_full())
                .when(self.adding == Some(Adding::Picture), |d| {
                    d.border_dashed()
                        .border_color(gpui::rgba(self.theme.katna.accent))
                })
                .into_any_element(),
        ]
    }
}
