// SPDX-License-Identifier: GPL-3.0-or-later

//! The Pages card (docs/PLAN.md §4.3, page operations), opened from the
//! Pages pane: blank pages added in a chosen size, pictures added as
//! pages, the pages of another PDF added, or put in place of the pages
//! picked; the pages picked made another size, with what they draw
//! scaled to fit or kept as big as it was; cropped by margins typed in;
//! and the file split into files of so many pages each.

use std::collections::BTreeSet;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::objects::length;
use super::{Content, Pages, PagesFrom, Viewer, file_name, organize};
use crate::float;
use crate::text::t;

/// The page sizes offered: name, width and height in points, upright.
const PRESETS: [(&str, f32, f32); 6] = [
    ("A4", 595.0, 842.0),
    ("A5", 420.0, 595.0),
    ("A3", 842.0, 1191.0),
    ("Letter", 612.0, 792.0),
    ("Legal", 612.0, 1008.0),
    ("Tabloid", 792.0, 1224.0),
];

/// The Pages card.
pub(super) struct PageSetup {
    /// The tab shown: 0 add, 1 replace, 2 page size, 3 margins, 4 split.
    tab: usize,
    /// Which of [`PRESETS`], or `PRESETS.len()` for the page shown's own
    /// size.
    preset: usize,
    landscape: bool,
    /// How many blank pages to add.
    count: Entity<TextInput>,
    /// Whether a page made another size has what it draws scaled to fit.
    scale: bool,
    /// Margins to crop by: top, right, bottom, left.
    margins: [Entity<TextInput>; 4],
    /// Pages a file split off holds.
    every: Entity<TextInput>,
    /// What is added: 0 blank pages, 1 a PDF's pages, 2 pictures.
    source: usize,
    /// All of [`PRESETS`] shown, not only the usual few.
    other: bool,
    /// Added before the page named, not after it.
    before: bool,
    /// The page added before or after: a number, first or last.
    at: Entity<TextInput>,
    _inputs: Vec<Subscription>,
}

impl Viewer {
    /// Opens the Pages card, or closes it.
    pub(super) fn toggle_setup(&mut self, cx: &mut Context<Self>) {
        if self.setup.take().is_some() {
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        self.numbering = None;
        self.properties = None;
        self.marking = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = |text: &str, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_accent(accent);
                input.set_text(text.to_owned(), cx);
                input
            })
        };
        let count = input("1", cx);
        let margins = [0, 1, 2, 3].map(|_| input("0", cx));
        let every = input("1", cx);
        let after = organize::insert_at(self.page_count(), &self.picked);
        let at = input(&after.max(1).to_string(), cx);
        let mut inputs = Vec::new();
        inputs.push(cx.subscribe(&count, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.add_blank_pages(cx);
            }
        }));
        for field in &margins {
            inputs.push(cx.subscribe(field, |this, _, event, cx| {
                if let InputEvent::Submit = event {
                    this.crop_by_margins(cx);
                }
            }));
        }
        // The page shown's size is the first choice, as it is what most
        // files add pages in.
        inputs.push(cx.subscribe(&at, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.add_pages(cx);
            }
        }));
        let (w, h) = self.page_size_unrotated(self.current_page());
        self.setup = Some(PageSetup {
            source: 0,
            other: false,
            before: false,
            at,
            tab: 0,
            preset: PRESETS.len(),
            landscape: w > h,
            count,
            scale: true,
            margins,
            every,
            _inputs: inputs,
        });
        cx.notify();
    }

    /// Page `page`'s media box size, before its turn.
    fn page_size_unrotated(&self, page: usize) -> (f32, f32) {
        let Content::Ready(opened) = &self.content else {
            return (612.0, 792.0);
        };
        let Some((w, h)) = opened.doc.page_size(page) else {
            return (612.0, 792.0);
        };
        // The size shown is the crop box turned; the media box is near
        // enough for the card's chip.
        if self.page_sideways(page) {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Whether page `page` is turned on its side by its `/Rotate`: its
    /// transform to the page shown turns.
    fn page_sideways(&self, page: usize) -> bool {
        let Content::Ready(opened) = &self.content else {
            return false;
        };
        opened
            .doc
            .page_transform(page)
            .is_some_and(|[_, b, c, _, _, _]| b.abs() > 1e-6 || c.abs() > 1e-6)
    }

    /// The size the card has chosen, width and height in points, lying
    /// as the card says.
    fn setup_size(&self) -> (f32, f32) {
        let Some(setup) = self.setup.as_ref() else {
            return (612.0, 792.0);
        };
        let (w, h) = match PRESETS.get(setup.preset) {
            Some(&(_, w, h)) => (w, h),
            None => {
                let (w, h) = self.page_size_unrotated(self.current_page());
                (w.min(h), w.max(h))
            }
        };
        if setup.landscape { (h, w) } else { (w, h) }
    }

    /// The pages the card works on: those picked, else all.
    fn setup_pages(&self) -> BTreeSet<usize> {
        if self.picked.is_empty() {
            (0..self.page_count()).collect()
        } else {
            self.picked.clone()
        }
    }

    /// Where pages added go: before or after the page the card names,
    /// or, with the card closed, after the pages picked or at the end.
    pub(super) fn insert_point(&self, cx: &gpui::App) -> usize {
        let count = self.page_count();
        let Some(setup) = self.setup.as_ref() else {
            return organize::insert_at(count, &self.picked);
        };
        let typed = setup.at.read(cx).text().trim().to_ascii_lowercase();
        let page = match typed.as_str() {
            "first" => 1,
            "last" | "" => count,
            n => n.parse::<usize>().unwrap_or(count).clamp(1, count.max(1)),
        };
        if setup.before { page - 1 } else { page }.min(count)
    }

    /// Adds what the Add tab says.
    fn add_pages(&mut self, cx: &mut Context<Self>) {
        match self.setup.as_ref().map(|s| s.source) {
            Some(0) => self.add_blank_pages(cx),
            Some(1) => self.choose_insert(cx),
            Some(_) => self.choose_pictures(cx),
            None => {}
        }
    }

    /// Adds blank pages of the chosen size where the card says.
    fn add_blank_pages(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.setup.as_ref() else {
            return;
        };
        let count = setup
            .count
            .read(cx)
            .text()
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
            .clamp(1, 500);
        let size = self.setup_size();
        let at = self.insert_point(cx);
        self.change_pages(
            Pages::Insert {
                from: PagesFrom::Blank(vec![size; count]),
                at,
            },
            (at..at + count).collect(),
            cx,
        );
    }

    /// Asks for picture files to add as pages after the pages picked.
    fn choose_pictures(&mut self, cx: &mut Context<Self>) {
        if self.changing_pages {
            return;
        }
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("pages-card-pictures-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && !paths.is_empty()
            {
                this.update(cx, |this, cx| {
                    let at = this.insert_point(cx);
                    let n = paths.len();
                    this.change_pages(
                        Pages::Insert {
                            from: PagesFrom::Pictures(paths),
                            at,
                        },
                        (at..at + n).collect(),
                        cx,
                    );
                })
                .ok();
            }
        })
        .detach();
    }

    /// Asks for a PDF whose pages go in place of the pages picked.
    fn choose_replacement(&mut self, cx: &mut Context<Self>) {
        if self.changing_pages || self.picked.is_empty() {
            return;
        }
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("pages-card-replace-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    let pages = this.picked.clone();
                    this.change_pages(Pages::Replace { file, pages }, BTreeSet::new(), cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// Makes the pages picked (or all) the chosen size.
    fn resize_pages(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.setup.as_ref() else {
            return;
        };
        let scale = setup.scale;
        let (w, h) = self.setup_size();
        let pages: Vec<usize> = self.setup_pages().into_iter().collect();
        let turned_pages: Vec<usize> = pages
            .iter()
            .copied()
            .filter(|&p| self.page_sideways(p))
            .collect();
        let shown = pages.clone();
        self.change_document(
            shown,
            "pages-failed",
            move |doc| {
                // A page turned on its side is made the size turned back,
                // so it shows as the card says.
                let mut out = doc.bytes().to_vec();
                let (upright, sideways): (Vec<usize>, Vec<usize>) =
                    pages.iter().partition(|&&p| !turned_pages.contains(&p));
                let mut current = None;
                if !upright.is_empty() {
                    out = doc
                        .resize_pages(&upright, (w, h), scale)
                        .map_err(|e| e.to_string())?;
                    current = Some(
                        katna_pdf_cos::Document::load(out.clone()).map_err(|e| e.to_string())?,
                    );
                }
                if !sideways.is_empty() {
                    let base = current.as_ref().unwrap_or(doc);
                    out = base
                        .resize_pages(&sideways, (h, w), scale)
                        .map_err(|e| e.to_string())?;
                }
                Ok(out)
            },
            cx,
        );
    }

    /// Crops the pages picked (or all) by the margins typed.
    fn crop_by_margins(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.setup.as_ref() else {
            return;
        };
        let typed: Option<Vec<f32>> = setup
            .margins
            .iter()
            .map(|m| length(m.read(cx).text()).filter(|v| *v >= 0.0))
            .collect();
        let Some(&[top, right, bottom, left]) = typed.as_deref() else {
            self.notice = Some(super::said("pages-card-bad-margins", &[]));
            cx.notify();
            return;
        };
        let pages = self.setup_pages();
        let picked = self.picked.clone();
        self.change_pages(
            Pages::Margins {
                pages,
                margins: [top, right, bottom, left],
            },
            picked,
            cx,
        );
    }

    /// Asks for a folder to split the file into, so many pages a file.
    fn choose_split(&mut self, cx: &mut Context<Self>) {
        let Some(setup) = self.setup.as_ref() else {
            return;
        };
        let every = setup
            .every
            .read(cx)
            .text()
            .trim()
            .parse::<usize>()
            .unwrap_or(0)
            .max(1);
        let key = self.key.clone();
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("pages-card-split-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let count = doc.pages().len();
                    let mut files = 0;
                    for (k, start) in (0..count).step_by(every).enumerate() {
                        let sources: Vec<_> = (start..(start + every).min(count))
                            .map(|page| katna_pdf_cos::PageSource {
                                doc: &doc,
                                page,
                                turns: 0,
                                crop: None,
                            })
                            .collect();
                        let out =
                            katna_pdf_cos::compose(None, &sources).map_err(|e| e.to_string())?;
                        let target = folder.join(format!("{stem}-{}.pdf", k + 1));
                        std::fs::write(&target, out).map_err(|e| e.to_string())?;
                        files += 1;
                    }
                    Ok::<_, String>((files, folder))
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok((files, folder)) => super::said(
                        "pages-card-split-done",
                        &[
                            ("count", &files.to_string()),
                            ("folder", &file_name(&folder)),
                        ],
                    ),
                    Err(error) => super::said("pages-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Pages card, while open.
    pub(super) fn setup_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let setup = self.setup.as_ref()?;
        let th = self.theme;
        let busy = self.changing_pages;
        let some = !self.picked.is_empty();
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let heading = |id: &str| {
            div()
                .text_size(px(text::CAPTION))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        let field = |input: &Entity<TextInput>, width: f32| {
            div()
                .w(px(width))
                .h(px(32.0))
                .flex_none()
                .flex()
                .items_center()
                .px(px(space::S2))
                .rounded(px(6.0))
                .border_1()
                .border_color(th.line)
                .bg(th.card)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .child(input.clone())
        };
        let set = |change: fn(&mut PageSetup)| {
            cx.listener(move |this, _, _, cx| {
                if let Some(s) = this.setup.as_mut() {
                    change(s);
                }
                cx.notify();
            })
        };
        // Sizes: the page shown's own, then the presets.
        let own = self.page_size_unrotated(self.current_page());
        let mm = |v: f32| format!("{:.0}", v * 25.4 / 72.0);
        let mut sizes = div().flex().flex_wrap().gap(px(space::S1)).child(
            float::chip(
                "pages-card-size-own",
                t(
                    "pages-card-size-own",
                    &[
                        ("width", &mm(own.0.min(own.1))),
                        ("height", &mm(own.0.max(own.1))),
                    ],
                ),
                setup.preset >= PRESETS.len(),
                &th,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(s) = this.setup.as_mut() {
                    s.preset = PRESETS.len();
                }
                cx.notify();
            })),
        );
        for (i, (name, _, _)) in PRESETS.iter().enumerate() {
            sizes = sizes.child(
                float::chip(("pages-card-size", i), *name, setup.preset == i, &th).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(s) = this.setup.as_mut() {
                            s.preset = i;
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        let (w, h) = self.setup_size();
        let lying = div()
            .flex()
            .items_center()
            .gap(px(space::S3))
            .child(
                float::seg(&th)
                    .w(px(220.0))
                    .child(
                        float::seg_item(
                            "pages-card-portrait",
                            None,
                            Some(t("pages-card-portrait", &[]).into()),
                            !setup.landscape,
                            &th,
                        )
                        .on_click(set(|s| s.landscape = false)),
                    )
                    .child(
                        float::seg_item(
                            "pages-card-landscape",
                            None,
                            Some(t("pages-card-landscape", &[]).into()),
                            setup.landscape,
                            &th,
                        )
                        .on_click(set(|s| s.landscape = true)),
                    ),
            )
            .child(div().text_size(px(text::SMALL)).text_color(th.dim).child(t(
                "pages-card-chosen",
                &[("width", &mm(w)), ("height", &mm(h))],
            )));
        let size_section = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(heading("pages-card-size-heading"))
            .child(sizes)
            .child(lying);
        let act = |id: &'static str, words: String, primary: bool, enabled: bool| {
            float::button(id, words, primary, &th).when(!enabled, |b| b.opacity(0.4))
        };
        let pages_word = if some {
            "pages-card-picked"
        } else {
            "pages-card-all"
        };
        let tab = setup.tab;
        let tabs = [
            "pages-card-tab-add",
            "pages-card-tab-replace",
            "pages-card-tab-size",
            "pages-card-tab-margins",
            "pages-card-tab-split",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, words)| {
            float::chip(("pages-card-tab", i), t(words, &[]), tab == i, &th).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(s) = this.setup.as_mut() {
                        s.tab = i;
                    }
                    cx.notify();
                },
            ))
        });
        let row = || div().flex().flex_wrap().items_center().gap(px(space::S2));
        let (body, actions): (gpui::Div, gpui::Div) = match tab {
            0 => {
                let mut sources = float::seg(&th);
                for (i, id) in [
                    "pages-card-source-blank",
                    "pages-card-source-file",
                    "pages-card-source-pictures",
                ]
                .into_iter()
                .enumerate()
                {
                    sources = sources.child(
                        float::seg_item(
                            ("pages-card-source", i),
                            None,
                            Some(t(id, &[]).into()),
                            setup.source == i,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.setup.as_mut() {
                                s.source = i;
                            }
                            cx.notify();
                        })),
                    );
                }
                let shown = (self.current_page() + 1).to_string();
                let mut quick = float::seg(&th);
                let usual = [
                    (PRESETS.len(), t("pages-card-like", &[("page", &shown)])),
                    (0, "A4".to_owned()),
                    (3, "Letter".to_owned()),
                ];
                for (i, (preset, words)) in usual.into_iter().enumerate() {
                    let on = !setup.other && setup.preset == preset;
                    quick = quick.child(
                        float::seg_item(("pages-card-quick", i), None, Some(words.into()), on, &th)
                            .flex_none()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.setup.as_mut() {
                                    s.preset = preset;
                                    s.other = false;
                                }
                                cx.notify();
                            })),
                    );
                }
                let usual_on = [PRESETS.len(), 0, 3].contains(&setup.preset);
                quick = quick.child(
                    float::seg_item(
                        "pages-card-other",
                        None,
                        Some(t("pages-card-other", &[]).into()),
                        setup.other || !usual_on,
                        &th,
                    )
                    .flex_none()
                    .on_click(set(|s| s.other = true)),
                );
                let column = |id: &str, el: gpui::AnyElement| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label(id))
                        .child(el)
                };
                let before = setup.before;
                let place = |on_before: bool, id: &'static str| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(s) = this.setup.as_mut() {
                                s.before = on_before;
                            }
                            cx.notify();
                        }))
                        .child(float::radio(before == on_before, &th))
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(t(id, &[])),
                        )
                };
                let where_row = row()
                    .gap(px(space::S4))
                    .child(place(true, "pages-card-before"))
                    .child(place(false, "pages-card-after"))
                    .child(field(&setup.at, 72.0))
                    .child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(t("pages-card-at-hint", &[])),
                    );
                let mut body = div().flex().flex_col().gap(px(space::S3)).child(sources);
                if setup.source == 0 {
                    body = body.child(
                        row()
                            .items_start()
                            .gap(px(space::S4))
                            .child(column(
                                "pages-card-how-many",
                                field(&setup.count, 64.0).into_any_element(),
                            ))
                            .child(column("pages-card-size-heading", quick.into_any_element())),
                    );
                    if setup.other || !usual_on {
                        body = body.child(size_section);
                    }
                }
                body = body.child(column("pages-card-where", where_row.into_any_element()));
                let count = setup
                    .count
                    .read(cx)
                    .text()
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(1)
                    .clamp(1, 500)
                    .to_string();
                let go = match setup.source {
                    0 if count == "1" => t("pages-card-add-one", &[]),
                    0 => t("pages-card-add-n", &[("count", &count)]),
                    1 => t("pages-card-add-file", &[]),
                    _ => t("pages-card-add-pictures", &[]),
                };
                (
                    body,
                    row().child(
                        act("pages-card-add", go, true, !busy)
                            .on_click(cx.listener(|this, _, _, cx| this.add_pages(cx))),
                    ),
                )
            }
            1 => (
                div().child(label("pages-card-replace-detail")),
                row().child(
                    act(
                        "pages-card-replace",
                        t("pages-card-replace", &[]),
                        true,
                        !busy && some,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_replacement(cx))),
                ),
            ),
            2 => (
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(size_section)
                    .child(
                        row()
                            .child(
                                float::switch("pages-card-scale", setup.scale, &th)
                                    .on_click(set(|s| s.scale = !s.scale)),
                            )
                            .child(label("pages-card-scale")),
                    ),
                row().child(
                    act(
                        "pages-card-resize",
                        t("pages-card-resize", &[("pages", &t(pages_word, &[]))]),
                        true,
                        !busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.resize_pages(cx))),
                ),
            ),
            3 => (
                row().children(
                    [
                        "pages-card-top",
                        "pages-card-right",
                        "pages-card-bottom",
                        "pages-card-left",
                    ]
                    .into_iter()
                    .zip(&setup.margins)
                    .map(|(id, input)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(label(id))
                            .child(field(input, 80.0))
                    }),
                ),
                row().child(
                    act(
                        "pages-card-crop",
                        t("pages-card-crop", &[("pages", &t(pages_word, &[]))]),
                        true,
                        !busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.crop_by_margins(cx))),
                ),
            ),
            _ => (
                row()
                    .child(label("pages-card-every"))
                    .child(field(&setup.every, 64.0))
                    .child(label("pages-card-pages-each")),
                row().child(
                    act("pages-card-split", t("pages-card-split", &[]), true, !busy)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_split(cx))),
                ),
            ),
        };
        Some(
            div()
                .id("pages-card-card")
                .occlude()
                .w(px(520.0))
                // One height for every tab, so the tabs stay put.
                .h(px(420.0))
                .max_h_full()
                .flex()
                .flex_col()
                .rounded(px(10.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .overflow_y_scroll()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex_1()
                        .p(px(space::S4))
                        .flex()
                        .flex_col()
                        .gap(px(space::S3))
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .font_weight(gpui::FontWeight::BOLD)
                                .text_color(th.text)
                                .child(t("pages-card-title", &[])),
                        )
                        .child(div().flex().flex_wrap().gap(px(space::S1)).children(tabs))
                        .when(tab > 0, |d| d.child(label("pages-card-hint")))
                        .child(body),
                )
                .child(
                    div()
                        .px(px(space::S4))
                        .py(px(space::S3))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .border_t_1()
                        .border_color(th.line_soft)
                        .child(
                            float::button(
                                "pages-card-close",
                                t("pages-card-close", &[]),
                                false,
                                &th,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.setup = None;
                                cx.notify();
                            })),
                        )
                        .child(div().flex_1())
                        .child(actions),
                )
                .into_any_element(),
        )
    }
}

/// Margins typed as the page shows (top, right, bottom, left) turned back
/// to the page's own sides before its `/Rotate` (left, bottom, right,
/// top), so the crop lands where the margins were drawn.
pub(super) fn margins_in_media(rotate: i64, [top, right, bottom, left]: [f32; 4]) -> [f32; 4] {
    match rotate.rem_euclid(360) {
        90 => [bottom, right, top, left],
        180 => [right, top, left, bottom],
        270 => [top, left, bottom, right],
        _ => [left, bottom, right, top],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn margins_turn_with_the_page() {
        // Upright: as typed.
        assert_eq!(
            margins_in_media(0, [1.0, 2.0, 3.0, 4.0]),
            [4.0, 3.0, 2.0, 1.0]
        );
        // Turned a quarter clockwise, the page's left side is at the top.
        assert_eq!(
            margins_in_media(90, [1.0, 2.0, 3.0, 4.0]),
            [3.0, 2.0, 1.0, 4.0]
        );
        assert_eq!(
            margins_in_media(180, [1.0, 2.0, 3.0, 4.0]),
            [2.0, 1.0, 4.0, 3.0]
        );
        assert_eq!(
            margins_in_media(270, [1.0, 2.0, 3.0, 4.0]),
            [1.0, 4.0, 3.0, 2.0]
        );
    }
}
