// SPDX-License-Identifier: GPL-3.0-or-later

//! The organizer: every page large in a grid in place of the pages, to
//! pick, turn, delete and drag into a new order (docs/PLAN.md, Phase 2,
//! page operations). Pages are dragged the same way in the pages pane.

use std::sync::Arc;

use gpui::{
    AnyElement, ClickEvent, Context, ImageSource, RenderImage, Window, div, img, prelude::*,
};
use katna_ui::tokens::{radius, space, text};
use katna_ui::{px, unpx};

use super::{Content, Pages, Viewer};
use crate::organize;
use crate::theme::Theme;
use crate::thumbnails;

/// The box each page is fitted into.
const BOX_W: f32 = thumbnails::DRAWN;
const BOX_H: f32 = BOX_W * 1.42;
/// A cell: the box, its padding and the page number.
const CELL_W: f32 = BOX_W + 2.0 * space::S5;
const ROW_H: f32 = BOX_H + 2.0 * space::S3 + space::S2 + 20.0;
/// Rows laid out beyond the ones on screen.
const SPARE_ROWS: usize = 1;

/// Pages being dragged, shown under the pointer as the first of them
/// with their count.
#[derive(Clone)]
pub struct PagesDragged {
    /// The pages, in order.
    pub pages: Vec<usize>,
    image: Option<Arc<RenderImage>>,
    /// The first page's shape, height over width.
    tall: f32,
    th: Theme,
}

impl Render for PagesDragged {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let th = self.th;
        let width = 72.0;
        div()
            .relative()
            .w(px(width))
            .h(px(width * self.tall))
            .bg(gpui::white())
            .shadow_lg()
            .opacity(0.9)
            .children(
                self.image
                    .clone()
                    .map(|image| img(ImageSource::Render(image)).size_full()),
            )
            .when(self.pages.len() > 1, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-space::S2))
                        .right(px(-space::S2))
                        .px(px(space::S2))
                        .rounded(px(radius::FULL))
                        .bg(gpui::rgba(th.katna.accent))
                        .text_size(px(text::CAPTION))
                        .text_color(gpui::white())
                        .child(self.pages.len().to_string()),
                )
            })
    }
}

impl Viewer {
    /// Shows or leaves the organizer; the annotation tools are put away
    /// while it shows.
    pub(super) fn toggle_organizer(&mut self, cx: &mut Context<Self>) {
        self.organizer = !self.organizer && matches!(self.content, Content::Ready(_));
        if self.organizer && self.tools {
            self.toggle_tools(cx);
        }
        if self.organizer && self.picked.is_empty() {
            let page = self.current_page();
            self.picked.insert(page);
            self.anchor = Some(page);
        }
        cx.notify();
    }

    /// What dragging page `page` drags: the pages picked when it is one
    /// of them, otherwise it alone.
    pub(super) fn dragged(&self, page: usize) -> PagesDragged {
        let pages = if self.picked.contains(&page) {
            self.picked.iter().copied().collect()
        } else {
            vec![page]
        };
        let first = pages[0];
        let tall = match &self.content {
            Content::Ready(opened) => opened
                .doc
                .page_size(first)
                .map_or(1.29, |(w, h)| h / w.max(1.0)),
            _ => 1.29,
        };
        PagesDragged {
            image: self.thumbs.image(first),
            pages,
            tall: tall.clamp(0.2, 5.0),
            th: self.theme,
        }
    }

    /// Pages dropped on page `page`: they take its place, before it
    /// when they came from later pages, after it when from earlier.
    pub(super) fn drop_pages(
        &mut self,
        dragged: &PagesDragged,
        page: usize,
        cx: &mut Context<Self>,
    ) {
        if self.changing_pages {
            return;
        }
        let pages = dragged.pages.iter().copied().collect();
        let to = if dragged.pages.first().is_some_and(|&first| first < page) {
            page + 1
        } else {
            page
        };
        let Some((order, picked)) = organize::move_to(self.page_count(), &pages, to) else {
            return;
        };
        let order = order.into_iter().map(|(p, turns)| (0, p, turns)).collect();
        self.change_pages(Pages::Order(order), picked, cx);
    }

    /// The organizer in place of the pages: the page actions, then the
    /// grid, laid out only near the rows on screen.
    pub(super) fn organizer_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let count = self.page_count();
        // The grid's width as laid out last frame, or the window less the
        // pane at first.
        let width = match unpx(self.grid.bounds().size.width) {
            w if w > 0.0 => w,
            _ => {
                let pane = if self.pane_shown() {
                    crate::contents::WIDTH
                } else {
                    0.0
                };
                unpx(window.viewport_size().width) - pane
            }
        };
        let columns = (((width - 2.0 * space::S5) / CELL_W).floor() as usize).max(1);
        let rows = count.div_ceil(columns);
        let top = -unpx(self.grid.offset().y);
        let height = match unpx(self.grid.bounds().size.height) {
            h if h > 0.0 => h,
            _ => unpx(window.viewport_size().height),
        };
        let first = ((top - space::S5) / ROW_H).floor().max(0.0) as usize;
        let last = ((top + height) / ROW_H).ceil() as usize;
        let shown = first.saturating_sub(SPARE_ROWS)..(last + SPARE_ROWS).min(rows);
        let grid_rows: Vec<AnyElement> = (0..rows)
            .map(|row| {
                let line = div().h(px(ROW_H)).flex_shrink_0().flex().justify_center();
                if !shown.contains(&row) {
                    return line.into_any_element();
                }
                let pages = row * columns..((row + 1) * columns).min(count);
                line.children(pages.map(|page| self.grid_cell(page, window, cx)))
                    // Cells of a short last row stay in their columns.
                    .children((count..(row + 1) * columns).map(|_| div().w(px(CELL_W))))
                    .into_any_element()
            })
            .collect();
        let grid = div()
            .id("organizer")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.grid)
            .py(px(space::S5))
            .flex()
            .flex_col()
            .children(grid_rows);
        let thumb = th.katna.text_dim & 0xffff_ff00 | 0x99;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(th.backdrop)
            .child(self.page_actions(cx))
            .child(div().flex_1().min_h_0().child(self.grid_bar.clone().wrap(
                "organizer-scroll",
                &self.grid,
                grid,
                thumb,
                window,
                cx,
            )))
            .into_any_element()
    }

    /// Page `page` in the organizer: click to pick (Ctrl, Shift as in the
    /// pane), double-click to read it, drag to move it.
    fn grid_cell(
        &mut self,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Content::Ready(opened) = &self.content else {
            return div().into_any_element();
        };
        let (w, h) = opened.doc.page_size(page).unwrap_or((612.0, 792.0));
        let fit = (BOX_W / w.max(1.0)).min(BOX_H / h.max(1.0));
        let image = self.thumbs.image(page);
        if image.is_none() {
            self.thumbs.want(page);
            self.pump_thumbs(window.scale_factor(), cx);
        }
        let picked = self.picked.contains(&page);
        let accent = th.katna.accent;
        div()
            .id(("organize", page))
            .w(px(CELL_W))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(space::S2))
            .py(px(space::S3))
            .rounded(px(radius::SM))
            .cursor_pointer()
            .when(picked, |d| d.bg(th.field))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.organizer = false;
                    this.go_to_page(page, cx);
                    return;
                }
                let keys = event.modifiers();
                this.picked = organize::click(
                    &this.picked,
                    this.anchor,
                    page,
                    keys.control || keys.platform,
                    keys.shift,
                );
                if !keys.shift {
                    this.anchor = Some(page);
                }
                cx.notify();
            }))
            .on_drag(self.dragged(page), |dragged, _, _, cx| {
                cx.new(|_| dragged.clone())
            })
            .drag_over::<PagesDragged>(move |style, _, _, _| {
                style.bg(gpui::rgba(accent & 0xffff_ff00 | 0x33))
            })
            .on_drop(cx.listener(move |this, dragged: &PagesDragged, _, cx| {
                this.drop_pages(dragged, page, cx)
            }))
            .child(
                div().h(px(BOX_H)).flex().items_center().child(
                    div()
                        .w(px(w * fit))
                        .h(px(h * fit))
                        .bg(self.paper())
                        .shadow_sm()
                        .border_2()
                        .border_color(if picked {
                            gpui::rgba(accent).into()
                        } else {
                            gpui::transparent_black()
                        })
                        .children(image.map(|image| img(ImageSource::Render(image)).size_full())),
                ),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(if picked { th.text } else { th.dim })
                    .child((page + 1).to_string()),
            )
            .into_any_element()
    }
}
