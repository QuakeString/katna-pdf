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
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::objects::length;
use super::{Content, Pages, PagesFrom, Viewer, file_name, organize};
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
        let (w, h) = self.page_size_unrotated(self.current_page());
        self.setup = Some(PageSetup {
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

    /// Adds blank pages of the chosen size after the pages picked, or at
    /// the end.
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
        let at = organize::insert_at(self.page_count(), &self.picked);
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
                    let at = organize::insert_at(this.page_count(), &this.picked);
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
            self.notice = Some(t("pages-card-bad-margins", &[]));
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
                    Ok((files, folder)) => t(
                        "pages-card-split-done",
                        &[
                            ("count", &files.to_string()),
                            ("folder", &file_name(&folder)),
                        ],
                    ),
                    Err(error) => t("pages-failed", &[("error", &error)]),
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
                .pt(px(space::S2))
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.text)
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
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .child(input.clone())
        };
        let chip = |id: gpui::ElementId, name: String, on: bool| {
            div()
                .id(id)
                .h(px(28.0))
                .px(px(space::S3))
                .flex_none()
                .flex()
                .items_center()
                .rounded(px(radius::FULL))
                .border_1()
                .border_color(if on {
                    gpui::rgba(th.katna.accent).into()
                } else {
                    th.line
                })
                .when(on, |d| d.bg(th.field))
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .hover(move |s| s.bg(th.hover))
                .cursor_pointer()
                .child(name)
        };
        // Sizes: the presets and the page shown's own.
        let mut sizes = div().flex().flex_wrap().gap(px(space::S1));
        let own = self.page_size_unrotated(self.current_page());
        for (i, (name, _, _)) in PRESETS.iter().enumerate() {
            sizes = sizes.child(
                chip(
                    ("pages-card-size", i).into(),
                    (*name).to_owned(),
                    setup.preset == i,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(s) = this.setup.as_mut() {
                        s.preset = i;
                    }
                    cx.notify();
                })),
            );
        }
        let mm = |v: f32| format!("{:.0}", v * 25.4 / 72.0);
        sizes = sizes.child(
            chip(
                "pages-card-size-own".into(),
                t(
                    "pages-card-size-own",
                    &[
                        ("width", &mm(own.0.min(own.1))),
                        ("height", &mm(own.0.max(own.1))),
                    ],
                ),
                setup.preset >= PRESETS.len(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(s) = this.setup.as_mut() {
                    s.preset = PRESETS.len();
                }
                cx.notify();
            })),
        );
        let lying = div()
            .flex()
            .gap(px(space::S1))
            .child(
                chip(
                    "pages-card-portrait".into(),
                    t("pages-card-portrait", &[]),
                    !setup.landscape,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = this.setup.as_mut() {
                        s.landscape = false;
                    }
                    cx.notify();
                })),
            )
            .child(
                chip(
                    "pages-card-landscape".into(),
                    t("pages-card-landscape", &[]),
                    setup.landscape,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(s) = this.setup.as_mut() {
                        s.landscape = true;
                    }
                    cx.notify();
                })),
            );
        let (w, h) = self.setup_size();
        let chosen = t(
            "pages-card-chosen",
            &[("width", &mm(w)), ("height", &mm(h))],
        );
        let act = |id: &'static str, label: String, enabled: bool| {
            widgets::outlined_button(id, label, &th.katna).when(!enabled, |b| b.opacity(0.4))
        };
        let pages_word = if some {
            "pages-card-picked"
        } else {
            "pages-card-all"
        };
        Some(
            div()
                .id("pages-card-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(440.0))
                .max_h(px(720.0))
                .p(px(space::S4))
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .rounded(px(radius::MD))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .shadow(widgets::elevation(&th.katna, 2.0))
                .overflow_y_scroll()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(text::BODY))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(th.text)
                                .child(t("pages-card-title", &[])),
                        )
                        .child(
                            widgets::icon_button("pages-card-close", "close", 18.0, &th.katna)
                                .tip(t("pages-card-close", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.setup = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(label("pages-card-hint"))
                .child(heading("pages-card-size-heading"))
                .child(sizes)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(lying)
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child(chosen),
                        ),
                )
                .child(heading("pages-card-add-heading"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::S2))
                        .child(field(&setup.count, 48.0))
                        .child(
                            act(
                                "pages-card-add-blank",
                                t("pages-card-add-blank", &[]),
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.add_blank_pages(cx))),
                        )
                        .child(
                            act(
                                "pages-card-add-pictures",
                                t("pages-card-add-pictures", &[]),
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.choose_pictures(cx))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            act("pages-card-add-file", t("pages-card-add-file", &[]), !busy)
                                .on_click(cx.listener(|this, _, _, cx| this.choose_insert(cx))),
                        )
                        .child(
                            act(
                                "pages-card-replace",
                                t("pages-card-replace", &[]),
                                !busy && some,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.choose_replacement(cx))),
                        ),
                )
                .child(heading("pages-card-resize-heading"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            chip(
                                "pages-card-scale".into(),
                                t("pages-card-scale", &[]),
                                setup.scale,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(s) = this.setup.as_mut() {
                                    s.scale = !s.scale;
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            act(
                                "pages-card-resize",
                                t("pages-card-resize", &[("pages", &t(pages_word, &[]))]),
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.resize_pages(cx))),
                        ),
                )
                .child(heading("pages-card-margins-heading"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_end()
                        .gap(px(space::S2))
                        .children(
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
                                    .child(field(input, 64.0))
                            }),
                        )
                        .child(
                            act(
                                "pages-card-crop",
                                t("pages-card-crop", &[("pages", &t(pages_word, &[]))]),
                                !busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.crop_by_margins(cx))),
                        ),
                )
                .child(heading("pages-card-split-heading"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(label("pages-card-every"))
                        .child(field(&setup.every, 48.0))
                        .child(label("pages-card-pages-each"))
                        .child(
                            act("pages-card-split", t("pages-card-split", &[]), !busy)
                                .on_click(cx.listener(|this, _, _, cx| this.choose_split(cx))),
                        ),
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
