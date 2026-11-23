// SPDX-License-Identifier: GPL-3.0-or-later

//! Export (Ctrl+Shift+E): the pages drawn out as PNG or JPEG files at a
//! resolution, their text written out plain or as a Word file, or a
//! copy of the file as a scanner would see it, pictures only. The card
//! asks what and which pages, then where.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AnyElement, App, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_render::export::{PictureKind, page_picture, scanned_page};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// What is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportKind {
    Png,
    Jpeg,
    Text,
    Word,
    /// The tables the pages' text makes, as CSV.
    Csv,
    Scanned,
}

impl ExportKind {
    const ALL: [(ExportKind, &'static str); 6] = [
        (ExportKind::Png, "export-png"),
        (ExportKind::Jpeg, "export-jpeg"),
        (ExportKind::Text, "export-text"),
        (ExportKind::Word, "export-word"),
        (ExportKind::Csv, "export-csv"),
        (ExportKind::Scanned, "export-scanned"),
    ];

    /// Whether pages are drawn, so the resolution matters.
    fn drawn(self) -> bool {
        matches!(
            self,
            ExportKind::Png | ExportKind::Jpeg | ExportKind::Scanned
        )
    }

    /// Whether many files go into a folder.
    fn folder(self) -> bool {
        matches!(self, ExportKind::Png | ExportKind::Jpeg)
    }

    fn extension(self) -> &'static str {
        match self {
            ExportKind::Png => "png",
            ExportKind::Jpeg => "jpg",
            ExportKind::Text => "txt",
            ExportKind::Word => "docx",
            ExportKind::Csv => "csv",
            ExportKind::Scanned => "pdf",
        }
    }
}

/// The resolutions offered, in dots to the inch.
const DPIS: [u16; 4] = [72, 150, 300, 600];

pub(super) struct Exporting {
    kind: ExportKind,
    /// Into `DPIS`.
    dpi: usize,
    grey: bool,
    /// Only the pages typed, not all.
    these: bool,
    pages: Entity<TextInput>,
    working: bool,
    _sub: Subscription,
}

impl Viewer {
    pub(super) fn toggle_exporting(&mut self, cx: &mut Context<Self>) {
        if self.exporting.take().is_some() {
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        self.properties = None;
        self.marking = None;
        self.redacting = None;
        self.designing = None;
        self.linking = None;
        self.numbering = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let pages = cx.new(|cx| {
            let mut input = TextInput::new(t("export-pages", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&pages, |this, _, event, cx| match event {
            InputEvent::Submit => this.run_export(cx),
            InputEvent::Changed => {
                if let Some(e) = this.exporting.as_mut() {
                    e.these = true;
                }
                cx.notify();
            }
            InputEvent::Cancel => {}
        });
        self.exporting = Some(Exporting {
            kind: ExportKind::Png,
            dpi: 1,
            grey: false,
            these: false,
            pages,
            working: false,
            _sub: sub,
        });
        cx.notify();
    }

    /// The export dialog (R18): what to write out as cards, then the
    /// resolution and colour for pages drawn, then which pages and the
    /// files that will be written.
    pub(super) fn export_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let exporting = self.exporting.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .pt(px(space::S2))
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let option = |i: usize, kind: ExportKind, id: &'static str| {
            float::option_card(
                ("export-kind", i),
                t(id, &[]),
                t(&format!("{id}-detail"), &[]),
                exporting.kind == kind,
                &th,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(e) = this.exporting.as_mut() {
                    e.kind = kind;
                }
                cx.notify();
            }))
        };
        let kinds = ExportKind::ALL;
        let grid = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .flex()
                    .gap(px(space::S2))
                    .children((0..3).map(|i| option(i, kinds[i].0, kinds[i].1))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S2))
                    .children((3..6).map(|i| option(i, kinds[i].0, kinds[i].1))),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(label("export-what"))
            .child(grid);
        if exporting.kind.drawn() {
            let mut dpis = float::seg(&th);
            for (i, dpi) in DPIS.into_iter().enumerate() {
                dpis = dpis.child(
                    float::seg_item(
                        ("export-dpi", i),
                        None,
                        Some(dpi.to_string().into()),
                        exporting.dpi == i,
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.exporting.as_mut() {
                            e.dpi = i;
                        }
                        cx.notify();
                    })),
                );
            }
            let dpis = dpis.child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::S2))
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child("dpi"),
            );
            let mut row = div().flex().gap(px(space::S5)).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("export-dpi-label"))
                    .child(dpis),
            );
            if exporting.kind == ExportKind::Scanned {
                let mut colours = float::seg(&th);
                for (grey, id) in [(false, "export-colour"), (true, "export-grey")] {
                    colours = colours.child(
                        float::seg_item(
                            ("export-colour", grey as usize),
                            None,
                            Some(t(id, &[]).into()),
                            exporting.grey == grey,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(e) = this.exporting.as_mut() {
                                e.grey = grey;
                            }
                            cx.notify();
                        })),
                    );
                }
                row = row.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("export-colour-label"))
                        .child(colours),
                );
            }
            body = body.child(row);
        }
        let count = self.page_count();
        let these = exporting.these;
        let choice = |id: &'static str, on: bool, words: String, pick: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(space::S2))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(e) = this.exporting.as_mut() {
                        e.these = pick;
                    }
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
        body = body
            .child(label("export-pages-label"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S4))
                    .child(choice(
                        "export-all",
                        !these,
                        t("print-all", &[("count", &count.to_string())]),
                        false,
                    ))
                    .child(choice("export-these", these, t("print-these", &[]), true))
                    .child(div().flex_1().min_w_0().child(float::input_box(
                        &exporting.pages,
                        window,
                        cx,
                        &th,
                    ))),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(self.export_outcome(exporting, cx)),
            );
        let foot = div()
            .flex()
            .gap(px(space::S2))
            .child(
                float::button("export-cancel", t("export-cancel", &[]), false, &th)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_exporting(cx))),
            )
            .child(
                float::button(
                    "export-go",
                    t(
                        if exporting.working {
                            "export-working"
                        } else {
                            "export-go"
                        },
                        &[],
                    ),
                    true,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.run_export(cx))),
            );
        Some(
            float::dialog_card(
                "export-card",
                t("export-title", &[]),
                Some("Ctrl Shift E"),
                520.0,
                body,
                foot,
                &th,
            )
            .into_any_element(),
        )
    }

    /// What the export will write, in a line: the files, or the file.
    fn export_outcome(&self, exporting: &Exporting, cx: &App) -> String {
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "page".into());
        let count = self.page_count();
        let spec = exporting.pages.read(cx).text().trim().to_owned();
        let pages = if exporting.these && !spec.is_empty() {
            match katna_pdf_cos::ranges::parse(&spec, count) {
                Ok(pages) => pages,
                Err(error) => return error.to_string(),
            }
        } else {
            (0..count).collect()
        };
        let kind = exporting.kind;
        match (kind.folder(), pages.first(), pages.last()) {
            (true, Some(first), Some(last)) => {
                let ext = kind.extension();
                t(
                    "export-files",
                    &[
                        ("count", &pages.len().to_string()),
                        ("first", &format!("{stem}-{}.{ext}", first + 1)),
                        ("last", &format!("{stem}-{}.{ext}", last + 1)),
                    ],
                )
            }
            _ => {
                let name = match kind {
                    ExportKind::Scanned => format!("{stem}-scanned.pdf"),
                    _ => format!("{stem}.{}", kind.extension()),
                };
                t(
                    "export-file",
                    &[("name", &name), ("count", &pages.len().to_string())],
                )
            }
        }
    }

    /// Asks where the export goes, then writes it there.
    fn run_export(&mut self, cx: &mut Context<Self>) {
        let (Some(exporting), Some(path), Content::Ready(opened)) =
            (self.exporting.as_ref(), self.path.clone(), &self.content)
        else {
            return;
        };
        if exporting.working {
            return;
        }
        let kind = exporting.kind;
        let dpi = f32::from(DPIS[exporting.dpi]);
        let grey = exporting.grey;
        let spec = if exporting.these {
            exporting.pages.read(cx).text().trim().to_owned()
        } else {
            String::new()
        };
        let count = opened.doc.page_count();
        let pages: Vec<usize> = if spec.is_empty() {
            (0..count).collect()
        } else {
            match katna_pdf_cos::ranges::parse(&spec, count) {
                Ok(pages) => pages,
                Err(error) => {
                    self.notice = Some(t("export-failed", &[("error", &error.to_string())]));
                    cx.notify();
                    return;
                }
            }
        };
        let job = Job {
            kind,
            dpi,
            grey,
            pages,
            stem: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "page".into()),
            drawn: opened.doc.clone(),
            current: self.unsaved(),
            key: self.key.clone(),
            path: path.clone(),
        };
        if let Some(e) = self.exporting.as_mut() {
            e.working = true;
        }
        cx.notify();
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if kind.folder() {
            let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some(t("export-choose-folder", &[]).into()),
            });
            cx.spawn(async move |this, cx| {
                let target = match chosen.await {
                    Ok(Ok(Some(mut paths))) => paths.pop(),
                    _ => None,
                };
                this.update(cx, |this, cx| this.export_to(target, job, cx))
                    .ok();
            })
            .detach();
        } else {
            let name = match kind {
                ExportKind::Scanned => format!("{}-scanned.pdf", job.stem),
                _ => format!("{}.{}", job.stem, kind.extension()),
            };
            let chosen = cx.prompt_for_new_path(&folder, Some(&name));
            cx.spawn(async move |this, cx| {
                let target = match chosen.await {
                    Ok(Ok(Some(target))) => Some(target),
                    _ => None,
                };
                this.update(cx, |this, cx| this.export_to(target, job, cx))
                    .ok();
            })
            .detach();
        }
    }

    /// Writes `job` to `target`, or puts the card back when none was
    /// chosen.
    fn export_to(&mut self, target: Option<PathBuf>, job: Job, cx: &mut Context<Self>) {
        let Some(target) = target else {
            if let Some(e) = self.exporting.as_mut() {
                e.working = false;
            }
            cx.notify();
            return;
        };
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move { export(&job, &target) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.exporting = None;
                this.notice = Some(match written {
                    Ok((name, n)) => {
                        t("export-done", &[("name", &name), ("count", &n.to_string())])
                    }
                    Err(error) => t("export-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// An export asked for, with what it needs of the file.
struct Job {
    kind: ExportKind,
    dpi: f32,
    grey: bool,
    pages: Vec<usize>,
    stem: String,
    drawn: Arc<dyn katna_pdf_render::Document>,
    current: Option<Arc<Vec<u8>>>,
    key: Option<Arc<str>>,
    path: PathBuf,
}

/// Writes the export, and says what was written and how many pages.
fn export(job: &Job, target: &Path) -> Result<(String, usize), String> {
    let Job {
        kind,
        dpi,
        grey,
        pages,
        stem,
        drawn,
        current,
        key,
        path,
    } = job;
    let (kind, dpi, grey) = (*kind, *dpi, *grey);
    let cos = || {
        let bytes = match current {
            Some(bytes) => bytes.to_vec(),
            None => std::fs::read(path).map_err(|e| e.to_string())?,
        };
        super::cos(bytes, key.as_deref())
    };
    let name = super::file_name(target);
    match kind {
        ExportKind::Png | ExportKind::Jpeg => {
            let picture = if kind == ExportKind::Png {
                PictureKind::Png
            } else {
                PictureKind::Jpeg { quality: 85 }
            };
            std::fs::create_dir_all(target).map_err(|e| e.to_string())?;
            for &page in pages {
                let data = page_picture(drawn.as_ref(), page, dpi, picture)
                    .ok_or_else(|| t("export-undrawn", &[("page", &(page + 1).to_string())]))?;
                let file = target.join(format!("{stem}-{}.{}", page + 1, picture.extension()));
                std::fs::write(&file, data).map_err(|e| e.to_string())?;
            }
            Ok((name, pages.len()))
        }
        ExportKind::Text => {
            let text = katna_pdf_cos::plain_text(&cos()?, pages);
            std::fs::write(target, text).map_err(|e| e.to_string())?;
            Ok((name, pages.len()))
        }
        ExportKind::Word => {
            let bytes = katna_pdf_cos::docx(&cos()?, pages);
            std::fs::write(target, bytes).map_err(|e| e.to_string())?;
            Ok((name, pages.len()))
        }
        ExportKind::Csv => {
            let tables = katna_pdf_cos::tables(&cos()?, pages);
            if tables.is_empty() {
                return Err(t("export-no-tables", &[]));
            }
            std::fs::write(target, katna_pdf_cos::csv(&tables)).map_err(|e| e.to_string())?;
            Ok((name, pages.len()))
        }
        ExportKind::Scanned => {
            let mut pictures = Vec::new();
            for &page in pages {
                let (data, width, height) = scanned_page(drawn.as_ref(), page, dpi, grey)
                    .ok_or_else(|| t("export-undrawn", &[("page", &(page + 1).to_string())]))?;
                let size = drawn.page_size(page).unwrap_or((612.0, 792.0));
                pictures.push((
                    katna_pdf_cos::Image::Jpeg {
                        data,
                        width,
                        height,
                        components: if grey { 1 } else { 3 },
                    },
                    size,
                ));
            }
            let bytes = katna_pdf_cos::pictures_pdf(pictures).map_err(|e| e.to_string())?;
            std::fs::write(target, bytes).map_err(|e| e.to_string())?;
            Ok((name, pages.len()))
        }
    }
}
