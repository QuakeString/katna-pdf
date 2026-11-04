// SPDX-License-Identifier: GPL-3.0-or-later

//! Print (Ctrl+P): a card asking how the pages go on the sheets (as
//! they are, a booklet, several a sheet, or a poster), the paper, and
//! whether to print as pictures (every page drawn, for a printer that
//! gets fonts or shapes wrong); then the sheets go to the desktop's
//! print dialog, or are saved as a PDF to print elsewhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{Sheet, SheetLayout};
use katna_pdf_render::export::scanned_page;
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::sign::check_box;
use super::{Content, Viewer};
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
const PAPERS: [(&str, (f32, f32)); 2] = [("print-a4", Sheet::A4), ("print-letter", Sheet::LETTER)];
/// Pages printed as pictures are drawn at this many dots to the inch.
const PICTURE_DPI: f32 = 200.0;

pub(super) struct Printing {
    layout: usize,
    paper: usize,
    pictures: bool,
    pages: Entity<TextInput>,
    working: bool,
    _sub: Subscription,
}

impl Viewer {
    /// Opens the print card, or closes it.
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
        let sub = cx.subscribe(&pages, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.print_sheets(None, cx);
            }
        });
        self.printing = Some(Printing {
            layout: 0,
            paper: 0,
            pictures: false,
            pages,
            working: false,
            _sub: sub,
        });
        cx.notify();
    }

    pub(super) fn print_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let printing = self.printing.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let chip = |id: gpui::ElementId, label: String, on: bool| {
            let chip = if on {
                widgets::filled_button(id, label, &th.katna)
            } else {
                widgets::outlined_button(id, label, &th.katna)
            };
            chip.h(px(28.0))
                .px(px(space::S3))
                .text_size(px(text::SMALL))
        };
        let mut layouts = div().flex().flex_wrap().gap(px(space::S1));
        for (i, (id, _, _)) in LAYOUTS.into_iter().enumerate() {
            layouts = layouts.child(
                chip(("print-layout", i).into(), t(id, &[]), printing.layout == i).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.printing.as_mut() {
                            p.layout = i;
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        let mut papers = div().flex().gap(px(space::S1));
        for (i, (id, _)) in PAPERS.into_iter().enumerate() {
            papers = papers.child(
                chip(("print-paper", i).into(), t(id, &[]), printing.paper == i).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.printing.as_mut() {
                            p.paper = i;
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        let pictures = printing.pictures;
        let mut card = div()
            .id("print-card")
            .absolute()
            .top(px(space::S3))
            .right(px(space::S5))
            .w(px(360.0))
            .p(px(space::S4))
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow(widgets::elevation(&th.katna, 2.0))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(text::BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(t("print-title", &[])),
            )
            .child(label("print-layout-label"))
            .child(layouts);
        if printing.layout > 0 {
            card = card.child(label("print-paper-label")).child(papers);
        }
        card = card
            .child(
                div()
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
                    .child(check_box(pictures, th))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("print-pictures", &[])),
                    ),
            )
            .child(label("print-pages-label"))
            .child(
                div()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(printing.pages.clone()),
            )
            .child(label("print-hint"))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        widgets::outlined_button("print-cancel", t("print-cancel", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_printing(cx))),
                    )
                    .child(
                        widgets::outlined_button("print-save", t("print-save", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.save_sheets(cx))),
                    )
                    .child(
                        widgets::filled_button(
                            "print-go",
                            t(
                                if printing.working {
                                    "print-working"
                                } else {
                                    "print-go"
                                },
                                &[],
                            ),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.print_sheets(None, cx))),
                    ),
            );
        Some(card.into_any_element())
    }

    /// What the card asks for, or `None` when the pages list is wrong
    /// (said in the notice).
    fn sheets_asked(&mut self, cx: &mut Context<Self>) -> Option<Job> {
        let (Some(printing), Some(path), Content::Ready(opened)) =
            (self.printing.as_ref(), self.path.clone(), &self.content)
        else {
            return None;
        };
        if printing.working {
            return None;
        }
        let (_, layout, sideways) = LAYOUTS[printing.layout];
        let count = opened.doc.page_count();
        let spec = printing.pages.read(cx).text().trim().to_owned();
        let pages: Vec<usize> = if spec.is_empty() {
            (0..count).collect()
        } else {
            match katna_pdf_cos::ranges::parse(&spec, count) {
                Ok(pages) => pages,
                Err(error) => {
                    self.notice = Some(t("print-failed", &[("error", &error.to_string())]));
                    cx.notify();
                    return None;
                }
            }
        };
        Some(Job {
            layout,
            sheet: Sheet::of(PAPERS[printing.paper].1, sideways),
            pictures: printing.pictures,
            all_pages: spec.is_empty(),
            pages,
            drawn: opened.doc.clone(),
            current: self.unsaved(),
            key: self.key.clone(),
            path,
            title: opened.name.clone(),
        })
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
        if save.is_none() && job.layout.is_none() && !job.pictures && job.all_pages {
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
                        this.notice = Some(t("print-failed", &[("error", &error)]));
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
                Some(t("print-saved", &[("name", &super::file_name(&target))]))
            } else {
                match crate::print::print(&title, &target).await {
                    crate::print::Printed::Done => None,
                    crate::print::Printed::NoDialog => Some(t("print-no-dialog", &[])),
                    crate::print::Printed::Failed(error) => {
                        Some(t("print-failed", &[("error", &error)]))
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
    let doc = super::cos(bytes, job.key.as_deref())?;
    // Pictures first: the laid-out pages are then the pictures.
    let (doc, pages) = if job.pictures {
        let mut pictures = Vec::new();
        for &page in &job.pages {
            let (data, width, height) = scanned_page(job.drawn.as_ref(), page, PICTURE_DPI, false)
                .ok_or_else(|| t("export-undrawn", &[("page", &(page + 1).to_string())]))?;
            let size = job.drawn.page_size(page).unwrap_or((612.0, 792.0));
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
