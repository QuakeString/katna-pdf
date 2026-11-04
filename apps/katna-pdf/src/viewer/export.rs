// SPDX-License-Identifier: GPL-3.0-or-later

//! Export (Ctrl+Shift+E): the pages drawn out as PNG or JPEG files at a
//! resolution, their text written out plain or as a Word file, or a
//! copy of the file as a scanner would see it, pictures only. The card
//! asks what and which pages, then where.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_render::export::{PictureKind, page_picture, scanned_page};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::sign::check_box;
use super::{Content, Viewer};
use crate::text::t;

/// What is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExportKind {
    Png,
    Jpeg,
    Text,
    Word,
    Scanned,
}

impl ExportKind {
    const ALL: [(ExportKind, &'static str); 5] = [
        (ExportKind::Png, "export-png"),
        (ExportKind::Jpeg, "export-jpeg"),
        (ExportKind::Text, "export-text"),
        (ExportKind::Word, "export-word"),
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
            ExportKind::Scanned => "pdf",
        }
    }
}

/// The resolutions offered, in dots to the inch.
const DPIS: [u16; 3] = [72, 150, 300];

pub(super) struct Exporting {
    kind: ExportKind,
    /// Into `DPIS`.
    dpi: usize,
    grey: bool,
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
        let sub = cx.subscribe(&pages, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.run_export(cx);
            }
        });
        self.exporting = Some(Exporting {
            kind: ExportKind::Png,
            dpi: 1,
            grey: false,
            pages,
            working: false,
            _sub: sub,
        });
        cx.notify();
    }

    pub(super) fn export_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let exporting = self.exporting.as_ref()?;
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
        let mut kinds = div().flex().flex_wrap().gap(px(space::S1));
        for (i, (kind, id)) in ExportKind::ALL.into_iter().enumerate() {
            kinds = kinds.child(
                chip(
                    ("export-kind", i).into(),
                    t(id, &[]),
                    exporting.kind == kind,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(e) = this.exporting.as_mut() {
                        e.kind = kind;
                    }
                    cx.notify();
                })),
            );
        }
        let mut card = div()
            .id("export-card")
            .absolute()
            .top(px(space::S3))
            .right(px(space::S5))
            .w(px(340.0))
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
                    .child(t("export-title", &[])),
            )
            .child(label("export-what"))
            .child(kinds);
        if exporting.kind.drawn() {
            let mut dpis = div().flex().gap(px(space::S1));
            for (i, dpi) in DPIS.into_iter().enumerate() {
                dpis = dpis.child(
                    chip(
                        ("export-dpi", i).into(),
                        t("export-dpi", &[("dpi", &dpi.to_string())]),
                        exporting.dpi == i,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.exporting.as_mut() {
                            e.dpi = i;
                        }
                        cx.notify();
                    })),
                );
            }
            card = card.child(label("export-dpi-label")).child(dpis);
        }
        if exporting.kind == ExportKind::Scanned {
            let grey = exporting.grey;
            card = card.child(
                div()
                    .id("export-grey")
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(e) = this.exporting.as_mut() {
                            e.grey = !e.grey;
                        }
                        cx.notify();
                    }))
                    .child(check_box(grey, th))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("export-grey", &[])),
                    ),
            );
        }
        card = card
            .child(label("export-pages-label"))
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
                    .child(exporting.pages.clone()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        widgets::outlined_button(
                            "export-cancel",
                            t("export-cancel", &[]),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_exporting(cx))),
                    )
                    .child(
                        widgets::filled_button(
                            "export-go",
                            t(
                                if exporting.working {
                                    "export-working"
                                } else {
                                    "export-go"
                                },
                                &[],
                            ),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.run_export(cx))),
                    ),
            );
        Some(card.into_any_element())
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
        let spec = exporting.pages.read(cx).text().trim().to_owned();
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
