// SPDX-License-Identifier: GPL-3.0-or-later

//! Print (Ctrl+P): a card asking how the pages go on the sheets (as
//! they are, a booklet, several a sheet, or a poster), the paper, and
//! whether to print as pictures (every page drawn, for a printer that
//! gets fonts or shapes wrong); then the sheets go to the desktop's
//! print dialog, or are saved as a PDF to print elsewhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AnyElement, App, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Parity, Sheet, SheetLayout, SheetScale};
use katna_pdf_render::export::scanned_page;
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::sign::check_box;
use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// The layouts offered, with the sheet on its side for those that want
/// it.
const LAYOUTS: [(&str, Option<SheetLayout>, bool); 8] = [
    ("print-as-is", None, false),
    ("print-booklet", Some(SheetLayout::Booklet), true),
    (
        "print-2-up",
        Some(SheetLayout::NUp { across: 2, down: 1 }),
        true,
    ),
    (
        "print-4-up",
        Some(SheetLayout::NUp { across: 2, down: 2 }),
        false,
    ),
    (
        "print-6-up",
        Some(SheetLayout::NUp { across: 3, down: 2 }),
        true,
    ),
    (
        "print-9-up",
        Some(SheetLayout::NUp { across: 3, down: 3 }),
        false,
    ),
    (
        "print-poster-4",
        Some(SheetLayout::Poster { across: 2, down: 2 }),
        false,
    ),
    (
        "print-poster-9",
        Some(SheetLayout::Poster { across: 3, down: 3 }),
        false,
    ),
];
const PAPERS: [(&str, (f32, f32)); 3] = [
    ("print-a4", Sheet::A4),
    ("print-letter", Sheet::LETTER),
    ("print-a3", (842.0, 1191.0)),
];
/// Which pages, as chips.
const PARITIES: [(&str, Parity); 3] = [
    ("print-all-pages", Parity::All),
    ("print-odd", Parity::Odd),
    ("print-even", Parity::Even),
];

/// What goes on the paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OnPaper {
    /// The pages and every comment and markup.
    All,
    /// The pages and form fields, no comments.
    Document,
    /// The pages, form fields and stamps.
    Stamps,
}

const CONTENTS: [(&str, OnPaper); 3] = [
    ("print-with-comments", OnPaper::All),
    ("print-document", OnPaper::Document),
    ("print-with-stamps", OnPaper::Stamps),
];

/// The size of each page on the paper, when printed as it is: as the
/// print dialog says, or one of these.
const SIZES: [(&str, Option<SheetScale>); 4] = [
    ("print-size-dialog", None),
    ("print-size-fit", Some(SheetScale::Fit)),
    ("print-size-actual", Some(SheetScale::Actual)),
    ("print-size-shrink", Some(SheetScale::Shrink)),
];

/// Pages printed as pictures are drawn at this many dots to the inch.
const PICTURE_DPI: f32 = 200.0;

pub(super) struct Printing {
    layout: usize,
    paper: usize,
    pictures: bool,
    parity: usize,
    reverse: bool,
    content: usize,
    size: usize,
    /// Which pages: all, the one shown, or those typed.
    which: Which,
    /// The page shown when the dialog opened.
    shown: usize,
    pages: Entity<TextInput>,
    working: bool,
    /// The sheets as they will print, for the preview.
    preview: Option<Preview>,
    /// Bumped with each change, so an older preview is dropped.
    asked: u64,
    _sub: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    All,
    Shown,
    These,
}

/// The sheets laid out as asked, and the one shown.
struct Preview {
    doc: Arc<dyn katna_pdf_render::Document>,
    count: usize,
    sheet: usize,
    image: Option<Arc<gpui::RenderImage>>,
}

/// The preview's room, in points of screen.
const PREVIEW_W: f32 = 300.0;
const PREVIEW_H: f32 = 380.0;

impl Viewer {
    /// Opens the print dialog, or closes it.
    pub(super) fn toggle_printing(&mut self, cx: &mut Context<Self>) {
        if self.printing.take().is_some() {
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        self.properties = None;
        self.exporting = None;
        self.marking = None;
        self.redacting = None;
        self.designing = None;
        self.linking = None;
        self.numbering = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let pages = cx.new(|cx| {
            let mut input = TextInput::new(t("print-pages", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&pages, |this, _, event, cx| match event {
            InputEvent::Submit => this.print_sheets(None, cx),
            InputEvent::Changed => {
                if let Some(p) = this.printing.as_mut() {
                    p.which = Which::These;
                }
                this.preview_sheets(cx);
            }
            InputEvent::Cancel => {}
        });
        self.printing = Some(Printing {
            layout: 0,
            paper: 0,
            pictures: false,
            parity: 0,
            reverse: false,
            content: 0,
            size: 0,
            which: Which::All,
            shown: self.current_page(),
            pages,
            working: false,
            preview: None,
            asked: 0,
            _sub: sub,
        });
        self.preview_sheets(cx);
        cx.notify();
    }

    /// Lays the sheets out again as the dialog now asks, and draws the
    /// first (or the one shown, if there still is one) for the preview.
    fn preview_sheets(&mut self, cx: &mut Context<Self>) {
        let Some(mut job) = self.sheets_quietly(cx) else {
            return;
        };
        // The preview is the same either way, and pictures are slow.
        job.pictures = false;
        let Some(printing) = self.printing.as_mut() else {
            return;
        };
        printing.asked += 1;
        let asked = printing.asked;
        let keep = printing.preview.as_ref().map_or(0, |p| p.sheet);
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let bytes = sheets(&job).ok()?;
                    let doc = crate::engine::engine().open(Arc::new(bytes), "").ok()?;
                    let count = doc.page_count();
                    let sheet = keep.min(count.saturating_sub(1));
                    let image = draw_sheet(doc.as_ref(), sheet);
                    Some(Preview {
                        doc,
                        count,
                        sheet,
                        image,
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.printing.as_mut()
                    && p.asked == asked
                {
                    p.preview = made;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows sheet `sheet` in the preview.
    fn preview_sheet(&mut self, sheet: usize, cx: &mut Context<Self>) {
        let Some(preview) = self.printing.as_mut().and_then(|p| p.preview.as_mut()) else {
            return;
        };
        if sheet >= preview.count || sheet == preview.sheet {
            return;
        }
        preview.sheet = sheet;
        let doc = preview.doc.clone();
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move { draw_sheet(doc.as_ref(), sheet) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(p) = this.printing.as_mut().and_then(|p| p.preview.as_mut())
                    && p.sheet == sheet
                {
                    p.image = image;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The print dialog (R19): the sheets as they will print on the
    /// left, a sheet at a time; how to print them on the right.
    pub(super) fn print_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let printing = self.printing.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .pt(px(space::S2))
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let layouts = div().flex().flex_wrap().gap(px(space::S2)).children(
            LAYOUTS.into_iter().enumerate().map(|(i, (id, _, _))| {
                float::chip(("print-layout", i), t(id, &[]), printing.layout == i, &th).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.printing.as_mut() {
                            p.layout = i;
                        }
                        this.preview_sheets(cx);
                        cx.notify();
                    }),
                )
            }),
        );
        let mut papers = float::seg(&th);
        for (i, (id, _)) in PAPERS.into_iter().enumerate() {
            papers = papers.child(
                float::seg_item(
                    ("print-paper", i),
                    None,
                    Some(t(id, &[]).into()),
                    printing.paper == i,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = this.printing.as_mut() {
                        p.paper = i;
                    }
                    this.preview_sheets(cx);
                    cx.notify();
                })),
            );
        }
        // A row of chips setting one of the card's choices.
        let row = |name: &'static str,
                   items: &[&'static str],
                   on: usize,
                   set: fn(&mut Printing, usize),
                   cx: &mut Context<Self>| {
            let mut row = div().flex().flex_wrap().gap(px(space::S1));
            for (i, id) in items.iter().enumerate() {
                row = row.child(float::chip((name, i), t(id, &[]), on == i, &th).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.printing.as_mut() {
                            set(p, i);
                        }
                        this.preview_sheets(cx);
                        cx.notify();
                    }),
                ));
            }
            row
        };
        let sizes = row(
            "print-size",
            &SIZES.map(|(id, _)| id),
            printing.size,
            |p, i| p.size = i,
            cx,
        );
        let contents = row(
            "print-content",
            &CONTENTS.map(|(id, _)| id),
            printing.content,
            |p, i| p.content = i,
            cx,
        );
        let parities = row(
            "print-parity",
            &PARITIES.map(|(id, _)| id),
            printing.parity,
            |p, i| p.parity = i,
            cx,
        );
        let parities = parities.child(
            div()
                .id("print-reverse")
                .ml(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(p) = this.printing.as_mut() {
                        p.reverse = !p.reverse;
                    }
                    this.preview_sheets(cx);
                    cx.notify();
                }))
                .child(check_box(printing.reverse, th))
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(t("print-reverse", &[])),
                ),
        );
        let count = self.page_count().to_string();
        let shown = (printing.shown + 1).to_string();
        let choice = |which: Which, words: String| {
            let on = printing.which == which;
            div()
                .id(("print-which", which as usize))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = this.printing.as_mut() {
                        p.which = which;
                    }
                    this.preview_sheets(cx);
                    cx.notify();
                }))
                .child(float::radio(on, &th))
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(words),
                )
        };
        let which = div()
            .flex()
            .items_center()
            .gap(px(space::S4))
            .child(choice(Which::All, t("print-all", &[("count", &count)])))
            .child(choice(Which::Shown, t("print-shown", &[("page", &shown)])))
            .child(choice(Which::These, t("print-these", &[])))
            .child(div().flex_1().min_w_0().child(float::input_box(
                &printing.pages,
                window,
                cx,
                &th,
            )));
        let pictures = div()
            .id("print-pictures")
            .flex()
            .items_center()
            .gap(px(space::S2))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.printing.as_mut() {
                    p.pictures = !p.pictures;
                }
                cx.notify();
            }))
            .child(check_box(printing.pictures, th))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(t("print-pictures", &[])),
            );
        let settings = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::S5))
                    .pt(px(space::S4))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("print-title", &[])),
                    )
                    .child(float::kbd("Ctrl P", &th)),
            )
            .child(
                div()
                    .id("print-settings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(space::S5))
                    .py(px(space::S2))
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .child(label("print-layout-label"))
                    .child(layouts)
                    .when(printing.layout == 0, |d| {
                        d.child(label("print-size-label")).child(sizes)
                    })
                    .when(printing.layout > 0 || printing.size > 0, |d| {
                        d.child(label("print-paper-label"))
                            .child(div().flex().child(papers))
                    })
                    .child(label("print-content-label"))
                    .child(contents)
                    .child(label("print-pages-label"))
                    .child(which)
                    .child(parities)
                    .child(div().pt(px(space::S2)).child(pictures))
                    .child(
                        div()
                            .pt(px(space::S2))
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(t("print-hint", &[])),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S5))
                    .py(px(space::S3))
                    .border_t_1()
                    .border_color(th.line)
                    .child(
                        float::button("print-save", t("print-save", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.save_sheets(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        float::button("print-cancel", t("print-cancel", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_printing(cx))),
                    )
                    .child(
                        float::button(
                            "print-go",
                            t(
                                if printing.working {
                                    "print-working"
                                } else {
                                    "print-go"
                                },
                                &[],
                            ),
                            true,
                            &th,
                        )
                        .child(float::icon("print", 14.0, th.on_accent))
                        .flex_row_reverse()
                        .on_click(cx.listener(|this, _, _, cx| this.print_sheets(None, cx))),
                    ),
            );
        let card = div()
            .id("print-card")
            .w(px(820.0))
            .h(px(540.0))
            .flex()
            .overflow_hidden()
            .rounded(px(12.0))
            .bg(th.card)
            .shadow(th.lifted())
            .when(th.dark, |d| d.border_1().border_color(th.line))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(self.print_preview(printing, cx))
            .child(div().w(px(1.0)).h_full().bg(th.line))
            .child(settings);
        Some(card.into_any_element())
    }

    /// The left side: the sheet shown, and the way to the others.
    fn print_preview(&self, printing: &Printing, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let (sheet, count, image, aspect) = match &printing.preview {
            Some(p) => {
                let (w, h) = p.doc.page_size(p.sheet).unwrap_or((595.0, 842.0));
                (p.sheet, p.count, p.image.clone(), w / h)
            }
            None => (0, 0, None, 595.0 / 842.0),
        };
        let (w, h) = if aspect > PREVIEW_W / PREVIEW_H {
            (PREVIEW_W, PREVIEW_W / aspect)
        } else {
            (PREVIEW_H * aspect, PREVIEW_H)
        };
        let arrow = |id: &'static str, glyph: &'static str, to: Option<usize>| {
            float::sized_ib(id, glyph, 28.0, 14.0, t(id, &[]), false, &th)
                .when(to.is_none(), |d| d.opacity(0.35))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(to) = to {
                        this.preview_sheet(to, cx);
                    }
                }))
        };
        div()
            .w(px(380.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(space::S3))
            .bg(if th.dark { th.sunk } else { th.field })
            .child(
                div()
                    .w(px(w))
                    .h(px(h))
                    .bg(gpui::white())
                    .shadow(th.lifted())
                    .children(
                        image.map(|image| gpui::img(gpui::ImageSource::Render(image)).size_full()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(arrow("print-back", "chevl", sheet.checked_sub(1)))
                    .child(
                        div()
                            .min_w(px(96.0))
                            .flex()
                            .justify_center()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(if count == 0 {
                                t("print-preview-working", &[])
                            } else {
                                t(
                                    "print-sheet-of",
                                    &[
                                        ("sheet", &(sheet + 1).to_string()),
                                        ("count", &count.to_string()),
                                    ],
                                )
                            }),
                    )
                    .child(arrow(
                        "print-next",
                        "chevr",
                        (sheet + 1 < count).then_some(sheet + 1),
                    )),
            )
            .into_any_element()
    }

    /// The pages asked for, or `None` when the list typed is wrong.
    fn chosen_pages(&self, cx: &App) -> Result<(Vec<usize>, bool), String> {
        let printing = self.printing.as_ref().ok_or_else(String::new)?;
        let count = self.page_count();
        let spec = printing.pages.read(cx).text().trim().to_owned();
        match printing.which {
            Which::All => Ok(((0..count).collect(), true)),
            Which::Shown => Ok((vec![printing.shown.min(count.saturating_sub(1))], false)),
            Which::These if spec.is_empty() => Ok(((0..count).collect(), true)),
            Which::These => katna_pdf_cos::ranges::parse(&spec, count)
                .map(|p| (p, false))
                .map_err(|e| e.to_string()),
        }
    }

    /// What the dialog asks for, without telling of a wrong list.
    fn sheets_quietly(&self, cx: &App) -> Option<Job> {
        let (Some(printing), Some(path), Content::Ready(opened)) =
            (self.printing.as_ref(), self.path.clone(), &self.content)
        else {
            return None;
        };
        let (pages, all_pages) = self.chosen_pages(cx).ok()?;
        if pages.is_empty() {
            return None;
        }
        let (_, layout, sideways) = LAYOUTS[printing.layout];
        // One page a sheet, sized as asked, each sheet the page's way up.
        let (layout, scale) = match (layout, SIZES[printing.size].1) {
            (None, Some(scale)) => (Some(SheetLayout::NUp { across: 1, down: 1 }), scale),
            (layout, _) => (layout, SheetScale::Fit),
        };
        let (parity, reverse) = (PARITIES[printing.parity].1, printing.reverse);
        let all_pages = all_pages && parity == Parity::All && !reverse;
        let pages = katna_pdf_cos::print_order(&pages, parity, reverse);
        if pages.is_empty() {
            return None;
        }
        Some(Job {
            layout,
            sheet: Sheet {
                scale,
                auto_turn: true,
                ..Sheet::of(PAPERS[printing.paper].1, sideways)
            },
            pictures: printing.pictures,
            all_pages,
            content: CONTENTS[printing.content].1,
            pages,
            // The pages as they are, not as Rotate view shows them.
            drawn: opened.plain.clone(),
            current: self.unsaved(),
            key: self.key.clone(),
            path,
            title: opened.name.clone(),
        })
    }

    /// What the dialog asks for, or `None` when the pages list is wrong
    /// (said in the notice).
    fn sheets_asked(&mut self, cx: &mut Context<Self>) -> Option<Job> {
        if self.printing.as_ref().is_none_or(|p| p.working) {
            return None;
        }
        if let Err(error) = self.chosen_pages(cx) {
            self.notice = Some(super::said("print-failed", &[("error", &error)]));
            cx.notify();
            return None;
        }
        self.sheets_quietly(cx)
    }

    /// Asks where to save the sheets as a PDF, and writes them there.
    fn save_sheets(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.sheets_asked(cx) else {
            return;
        };
        let folder = job.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = job
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&folder, Some(&format!("{stem}-print.pdf")));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(target))) = chosen.await {
                this.update(cx, |this, cx| this.print_sheets(Some(target), cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Makes the sheets and prints them, or writes them to `save`.
    fn print_sheets(&mut self, save: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(job) = self.sheets_asked(cx) else {
            return;
        };
        // Nothing to lay out: the file goes as it is.
        if save.is_none()
            && job.layout.is_none()
            && !job.pictures
            && job.all_pages
            && job.content == OnPaper::All
        {
            self.printing = None;
            self.print(cx);
            return;
        }
        if let Some(p) = self.printing.as_mut() {
            p.working = true;
        }
        cx.notify();
        let generation = self.generation;
        let title = job.title.clone();
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let bytes = sheets(&job)?;
                    let target = match save {
                        Some(target) => target,
                        None => temp_file()?,
                    };
                    std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            let target = match made {
                Ok(target) => target,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if let Some(p) = this.printing.as_mut() {
                            p.working = false;
                        }
                        this.notice = Some(super::said("print-failed", &[("error", &error)]));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let saved = this
                .update(cx, |this, cx| {
                    if this.generation == generation {
                        this.printing = None;
                    }
                    cx.notify();
                })
                .is_ok();
            if !saved {
                return;
            }
            let notice = if save_is(&target) {
                Some(super::said(
                    "print-saved",
                    &[("name", &super::file_name(&target))],
                ))
            } else {
                match crate::print::print(&title, &target).await {
                    crate::print::Printed::Done => None,
                    crate::print::Printed::NoDialog => Some(super::said("print-no-dialog", &[])),
                    crate::print::Printed::Failed(error) => {
                        Some(super::said("print-failed", &[("error", &error)]))
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.notice = notice;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A sheet drawn for the preview, sharp on a screen twice as dense.
fn draw_sheet(
    doc: &dyn katna_pdf_render::Document,
    sheet: usize,
) -> Option<Arc<gpui::RenderImage>> {
    let (w, h) = doc.page_size(sheet)?;
    let scale = (PREVIEW_W / w).min(PREVIEW_H / h) * 2.0;
    super::draw(
        doc,
        sheet,
        scale,
        katna_pdf_render::recolor::PageColors::Normal,
    )
}

/// Whether `target` was chosen to keep, not made to print.
fn save_is(target: &Path) -> bool {
    temp_dir().is_none_or(|dir| !target.starts_with(dir))
}

fn temp_dir() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|p| p.cache_dir().join("pdf").join("print"))
}

/// A file to hand the print dialog; the folder is emptied first.
fn temp_file() -> Result<PathBuf, String> {
    let dir = temp_dir().ok_or_else(|| "no cache folder".to_owned())?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(dir.join(format!("print-{n}.pdf")))
}

/// The sheets asked for, with what they need of the file.
struct Job {
    layout: Option<SheetLayout>,
    sheet: Sheet,
    pictures: bool,
    all_pages: bool,
    content: OnPaper,
    pages: Vec<usize>,
    drawn: Arc<dyn katna_pdf_render::Document>,
    current: Option<Arc<Vec<u8>>>,
    key: Option<Arc<str>>,
    path: PathBuf,
    title: String,
}

/// The PDF to print: the pages chosen, as pictures when asked, laid out
/// when asked.
fn sheets(job: &Job) -> Result<Vec<u8>, String> {
    let bytes = match &job.current {
        Some(bytes) => bytes.to_vec(),
        None => std::fs::read(&job.path).map_err(|e| e.to_string())?,
    };
    let mut doc = super::cos(bytes, job.key.as_deref())?;
    let mut drawn = job.drawn.clone();
    // Comments not wanted taken off first.
    if job.content != OnPaper::All {
        let remove = doc
            .annotations()
            .into_iter()
            .flatten()
            .filter(|a| !(job.content == OnPaper::Stamps && matches!(a.kind, AnnotKind::Stamp(_))))
            .filter_map(|a| a.id)
            .collect::<Vec<_>>();
        if !remove.is_empty() {
            let bytes = doc
                .annotate(&AnnotChanges {
                    put: Vec::new(),
                    remove,
                })
                .map_err(|e| e.to_string())?;
            if job.pictures {
                drawn = crate::engine::engine()
                    .open(Arc::new(bytes.clone()), job.key.as_deref().unwrap_or(""))
                    .map_err(|e| format!("{e:?}"))?;
            }
            doc = super::cos(bytes, job.key.as_deref())?;
        }
    }
    // Pictures first: the laid-out pages are then the pictures.
    let (doc, pages) = if job.pictures {
        let mut pictures = Vec::new();
        for &page in &job.pages {
            let (data, width, height) = scanned_page(drawn.as_ref(), page, PICTURE_DPI, false)
                .ok_or_else(|| t("export-undrawn", &[("page", &(page + 1).to_string())]))?;
            let size = drawn.page_size(page).unwrap_or((612.0, 792.0));
            pictures.push((
                katna_pdf_cos::Image::Jpeg {
                    data,
                    width,
                    height,
                    components: 3,
                },
                size,
            ));
        }
        let n = pictures.len();
        let bytes = katna_pdf_cos::pictures_pdf(pictures).map_err(|e| e.to_string())?;
        (super::cos(bytes, None)?, (0..n).collect())
    } else {
        (doc, job.pages.clone())
    };
    match job.layout {
        Some(layout) => {
            katna_pdf_cos::impose(&doc, &pages, layout, job.sheet).map_err(|e| e.to_string())
        }
        None if pages.len() == doc.pages().len()
            && pages.iter().enumerate().all(|(i, p)| i == *p) =>
        {
            Ok(doc.bytes().to_vec())
        }
        None => {
            let sources: Vec<katna_pdf_cos::PageSource<'_>> = pages
                .iter()
                .map(|&page| katna_pdf_cos::PageSource {
                    doc: &doc,
                    page,
                    turns: 0,
                    crop: None,
                })
                .collect();
            katna_pdf_cos::compose(Some(&doc), &sources).map_err(|e| e.to_string())
        }
    }
}
