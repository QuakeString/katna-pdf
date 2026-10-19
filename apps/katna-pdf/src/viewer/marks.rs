// SPDX-License-Identifier: GPL-3.0-or-later

//! Page marks (docs/PLAN.md §4.3, Phase 4): headers and footers,
//! watermarks, Bates numbers and backgrounds (a colour, a picture or
//! both) put on many pages at once, from a card opened on the edit row. They are drawn into each page's own content
//! (`katna_pdf_cos::stamp`), so every reader shows and prints them.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{Background, Bates, Stamp, StampPlace};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::insert::INKS;
use super::sign::check_box;
use super::{Content, Viewer};
use crate::text::t;

/// What kind of mark is being put on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkKind {
    Header,
    Watermark,
    Bates,
    Background,
}

/// Background colours: pale, for text to stay easy to read over them.
const PAPERS: [[f32; 3]; 5] = [
    [1.0, 0.98, 0.9],
    [1.0, 0.96, 0.75],
    [0.88, 0.94, 1.0],
    [0.9, 0.97, 0.88],
    [0.93, 0.93, 0.93],
];

/// The card's choices.
pub(super) struct Marking {
    kind: MarkKind,
    place: StampPlace,
    ink: usize,
    under: bool,
    words: Entity<TextInput>,
    size: Entity<TextInput>,
    pages: Entity<TextInput>,
    prefix: Entity<TextInput>,
    start: Entity<TextInput>,
    /// A background's colour, from [`PAPERS`], and picture.
    paper: Option<usize>,
    picture: Option<PathBuf>,
    /// The picture stretched over the whole page.
    stretch: bool,
    _inputs: Vec<Subscription>,
}

/// The six places of a header or footer, as the little page shows them.
const EDGES: [StampPlace; 6] = [
    StampPlace::TopLeft,
    StampPlace::TopCentre,
    StampPlace::TopRight,
    StampPlace::BottomLeft,
    StampPlace::BottomCentre,
    StampPlace::BottomRight,
];
/// What the words stand for in the page marks' messages.
const WORDS: &[(&str, &str)] = &[("page", "{page}"), ("pages", "{pages}")];
/// How faint a watermark is drawn.
const WATERMARK_OPACITY: f32 = 0.3;
/// How far from the page's edges, in points.
const MARGIN: f32 = 28.0;

impl Viewer {
    /// Opens the page marks card, or closes it.
    pub(super) fn toggle_marking(&mut self, cx: &mut Context<Self>) {
        if self.marking.take().is_some() {
            cx.notify();
            return;
        }
        self.pick_adding(None, cx);
        self.properties = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let field = |placeholder: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input
            })
        };
        let words = field(t("marks-text", &[]), cx);
        let size = field(t("marks-size", &[]), cx);
        let pages = field(t("marks-pages-all", &[]), cx);
        let prefix = field(t("marks-bates-prefix", &[]), cx);
        let start = field(t("marks-bates-start", &[]), cx);
        let inputs = [&words, &size, &pages, &prefix, &start]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.put_marks(cx);
                    }
                })
            })
            .collect();
        self.marking = Some(Marking {
            kind: MarkKind::Header,
            place: StampPlace::BottomCentre,
            ink: 0,
            under: false,
            words,
            size,
            pages,
            prefix,
            start,
            paper: Some(0),
            picture: None,
            stretch: false,
            _inputs: inputs,
        });
        self.set_mark_kind(MarkKind::Header, cx);
    }

    /// Switches the kind of mark, with that kind's usual words and size.
    fn set_mark_kind(&mut self, kind: MarkKind, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        marking.kind = kind;
        let (words, size, place, ink) = match kind {
            MarkKind::Header => (
                t("marks-header-default", WORDS),
                "10",
                StampPlace::BottomCentre,
                0,
            ),
            MarkKind::Watermark => (
                t("marks-watermark-default", &[]),
                "60",
                StampPlace::Across,
                1,
            ),
            MarkKind::Bates | MarkKind::Background => {
                (String::new(), "10", StampPlace::BottomRight, 0)
            }
        };
        marking.place = place;
        marking.ink = ink;
        marking.under = false;
        marking.words.update(cx, |f, cx| f.set_text(words, cx));
        marking
            .size
            .update(cx, |f, cx| f.set_text(size.to_string(), cx));
        marking
            .start
            .update(cx, |f, cx| f.set_text("1".to_string(), cx));
        cx.notify();
    }

    /// Puts the marks on the pages, as the card says.
    fn put_marks(&mut self, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_ref() else {
            return;
        };
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let count = self.page_count();
        let read = |input: &Entity<TextInput>| input.read(cx).text().trim().to_owned();
        let spec = read(&marking.pages);
        let pages = if spec.is_empty() {
            (0..count).collect()
        } else {
            match katna_pdf_cos::ranges::parse(&spec, count) {
                Ok(pages) => pages,
                Err(error) => {
                    self.notice = Some(t("marks-failed", &[("error", &error)]));
                    cx.notify();
                    return;
                }
            }
        };
        if marking.kind == MarkKind::Background {
            let background = Background {
                colour: marking.paper.map(|i| PAPERS[i]),
                image: None,
                stretch: marking.stretch,
                opacity: 1.0,
            };
            let picture = marking.picture.clone();
            if background.colour.is_none() && picture.is_none() {
                return;
            }
            self.marking = None;
            let shown = pages.clone();
            self.change_document(
                shown,
                "marks-failed",
                move |doc| {
                    let mut background = background;
                    if let Some(file) = picture {
                        let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                        background.image = Some(katna_pdf_sign::page_image(&bytes)?);
                    }
                    doc.background(&pages, &background)
                        .map_err(|e| e.to_string())
                },
                cx,
            );
            return;
        }
        let size = read(&marking.size)
            .replace(',', ".")
            .parse::<f32>()
            .ok()
            .filter(|s| s.is_finite() && *s > 0.0 && *s <= 500.0)
            .unwrap_or(10.0);
        let (words, bates) = match marking.kind {
            MarkKind::Bates => {
                let start = read(&marking.start).parse::<u64>().unwrap_or(1);
                let bates = Bates {
                    prefix: read(&marking.prefix),
                    suffix: String::new(),
                    start,
                    digits: 6,
                };
                ("{bates}".to_owned(), Some(bates))
            }
            _ => (read(&marking.words), None),
        };
        if words.is_empty() {
            return;
        }
        let stamp = Stamp {
            text: words,
            place: marking.place,
            font: "Helvetica".into(),
            size,
            colour: INKS[marking.ink],
            opacity: if marking.kind == MarkKind::Watermark {
                WATERMARK_OPACITY
            } else {
                1.0
            },
            margin: MARGIN,
            under: marking.under,
            bates,
        };
        self.marking = None;
        let shown = pages.clone();
        self.change_document(
            shown,
            "marks-failed",
            move |doc| doc.stamp(&pages, &stamp).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Asks for a picture to put under the pages.
    fn choose_background(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("marks-background-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(m) = this.marking.as_mut() {
                    m.picture = paths.into_iter().next();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A background's colours and picture, in the card.
    fn background_choices(&self, marking: &Marking, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let mut papers = div().flex().gap(px(space::S1));
        let swatch = |id: usize, on: bool| {
            div()
                .id(("marks-paper", id))
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .when(on, |d| {
                    d.border_2().border_color(gpui::rgba(th.katna.accent))
                })
                .hover(move |s| s.bg(th.hover))
        };
        papers = papers.child(
            swatch(PAPERS.len(), marking.paper.is_none())
                .tip(t("marks-background-no-colour", &[]), &th.katna)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(m) = this.marking.as_mut() {
                        m.paper = None;
                    }
                    cx.notify();
                }))
                .child(
                    gpui::svg()
                        .path("icons/close.svg")
                        .size(px(16.0))
                        .text_color(th.dim),
                ),
        );
        for (i, paper) in PAPERS.into_iter().enumerate() {
            papers = papers.child(
                swatch(i, marking.paper == Some(i))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.paper = Some(i);
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(18.0))
                            .rounded_full()
                            .border_1()
                            .border_color(th.line)
                            .bg(super::insert::colour(paper)),
                    ),
            );
        }
        let name = marking
            .picture
            .as_ref()
            .map(|p| super::file_name(p))
            .unwrap_or_else(|| t("marks-background-no-picture", &[]));
        let stretch = marking.stretch;
        div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("marks-background-colour", &[])),
            )
            .child(papers)
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("marks-background-picture", &[])),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(name),
                    )
                    .when(marking.picture.is_some(), |row| {
                        row.child(
                            widgets::icon_button("marks-picture-off", "close", 16.0, &th.katna)
                                .size(px(28.0))
                                .tip(t("marks-background-no-picture-tip", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(m) = this.marking.as_mut() {
                                        m.picture = None;
                                    }
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        widgets::outlined_button(
                            "marks-picture",
                            t("marks-background-choose", &[]),
                            &th.katna,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.choose_background(cx))),
                    ),
            )
            .child(
                div()
                    .id("marks-stretch")
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.stretch = !m.stretch;
                        }
                        cx.notify();
                    }))
                    .child(check_box(stretch, th))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("marks-background-stretch", &[])),
                    ),
            )
            .into_any_element()
    }

    /// The page marks card, while it is open.
    pub(super) fn marks_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let marking = self.marking.as_ref()?;
        let th = self.theme;
        let field = |input: &Entity<TextInput>| {
            div()
                .h(px(36.0))
                .flex()
                .items_center()
                .px(px(space::S3))
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::BODY))
                .text_color(th.text)
                .child(input.clone())
        };
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let chip = |id: (&'static str, usize), label: String, on: bool| {
            div()
                .id(id)
                .flex_1()
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::FULL))
                .text_size(px(text::SMALL))
                .font_weight(if on {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if on { th.text } else { th.dim })
                .when(on, |d| d.bg(th.field))
                .hover(move |s| s.bg(th.hover))
                .cursor_pointer()
                .child(label)
        };
        let kinds = [
            (MarkKind::Header, "marks-header"),
            (MarkKind::Watermark, "marks-watermark"),
            (MarkKind::Bates, "marks-bates"),
            (MarkKind::Background, "marks-background"),
        ];
        // Two rows of two, so each name fits.
        let mut kind_row = div().flex().flex_col().gap(px(space::S1));
        for (r, pair) in kinds.chunks(2).enumerate() {
            let mut row = div().flex().gap(px(space::S1));
            for (j, &(kind, id)) in pair.iter().enumerate() {
                row = row.child(
                    chip(("marks-kind", r * 2 + j), t(id, &[]), marking.kind == kind)
                        .on_click(cx.listener(move |this, _, _, cx| this.set_mark_kind(kind, cx))),
                );
            }
            kind_row = kind_row.child(row);
        }
        let place = marking.place;
        let pick_place = move |p: StampPlace| {
            move |this: &mut Viewer,
                  _: &gpui::ClickEvent,
                  _: &mut gpui::Window,
                  cx: &mut Context<Viewer>| {
                if let Some(m) = this.marking.as_mut() {
                    m.place = p;
                }
                cx.notify();
            }
        };
        let place_row = if marking.kind == MarkKind::Watermark {
            div()
                .flex()
                .gap(px(space::S1))
                .child(
                    chip(
                        ("marks-place", 0),
                        t("marks-across", &[]),
                        place == StampPlace::Across,
                    )
                    .on_click(cx.listener(pick_place(StampPlace::Across))),
                )
                .child(
                    chip(
                        ("marks-place", 1),
                        t("marks-middle", &[]),
                        place == StampPlace::Middle,
                    )
                    .on_click(cx.listener(pick_place(StampPlace::Middle))),
                )
                .into_any_element()
        } else {
            // A little page: a bar for each place along its top and foot.
            let bar = |i: usize, p: StampPlace| {
                let on = place == p;
                div()
                    .id(("marks-edge", i))
                    .w(px(28.0))
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::XS))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .tip(t(edge_tip(p), &[]), &th.katna)
                    .on_click(cx.listener(pick_place(p)))
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(4.0))
                            .rounded(px(radius::XS))
                            .bg(if on {
                                gpui::rgba(th.katna.accent).into()
                            } else {
                                th.dim
                            }),
                    )
            };
            let mut top = div().flex().justify_between();
            let mut foot = div().flex().justify_between();
            for (i, p) in EDGES.into_iter().enumerate() {
                if i < 3 {
                    top = top.child(bar(i, p));
                } else {
                    foot = foot.child(bar(i, p));
                }
            }
            div()
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(104.0))
                        .h(px(132.0))
                        .p(px(space::S1))
                        .flex()
                        .flex_col()
                        .justify_between()
                        .rounded(px(radius::XS))
                        .border_1()
                        .border_color(th.line)
                        .bg(th.card)
                        .child(top)
                        .child(foot),
                )
                .into_any_element()
        };
        let mut inks = div().flex().gap(px(space::S1));
        for (i, ink) in INKS.into_iter().enumerate() {
            let on = marking.ink == i;
            inks = inks.child(
                div()
                    .id(("marks-ink", i))
                    .size(px(28.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.border_2().border_color(gpui::rgba(th.katna.accent))
                    })
                    .hover(move |s| s.bg(th.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.ink = i;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(18.0))
                            .rounded_full()
                            .border_1()
                            .border_color(th.line)
                            .bg(super::insert::colour(ink)),
                    ),
            );
        }
        let under = marking.under;
        let text_mark = marking.kind != MarkKind::Background;
        let background = (!text_mark).then(|| self.background_choices(marking, cx));
        Some(
            div()
                .id("marks-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(320.0))
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
                        .child(t("marks-title", &[])),
                )
                .child(kind_row)
                .when(text_mark && marking.kind != MarkKind::Bates, |d| {
                    d.child(label("marks-text-label"))
                        .child(field(&marking.words))
                })
                .when(marking.kind == MarkKind::Header, |d| {
                    d.child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(t("marks-text-hint", WORDS)),
                    )
                })
                .when(marking.kind == MarkKind::Bates, |d| {
                    d.child(label("marks-bates-label")).child(
                        div()
                            .flex()
                            .gap(px(space::S2))
                            .child(div().flex_1().child(field(&marking.prefix)))
                            .child(div().w(px(96.0)).child(field(&marking.start))),
                    )
                })
                .children(background)
                .when(text_mark, |d| {
                    d.child(label("marks-place")).child(place_row)
                })
                .when(marking.kind == MarkKind::Watermark, |d| {
                    d.child(
                        div()
                            .id("marks-under")
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(m) = this.marking.as_mut() {
                                    m.under = !m.under;
                                }
                                cx.notify();
                            }))
                            .child(check_box(under, th))
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .text_color(th.text)
                                    .child(t("marks-under", &[])),
                            ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .when(text_mark, |row| {
                            row.child(
                                div()
                                    .w(px(80.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(space::S1))
                                    .child(label("marks-size-label"))
                                    .child(field(&marking.size)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(space::S1))
                                .child(label("marks-pages-label"))
                                .child(field(&marking.pages)),
                        ),
                )
                .when(text_mark, |d| d.child(inks))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(space::S2))
                        .child(
                            widgets::outlined_button(
                                "marks-cancel",
                                t("marks-cancel", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.marking = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            widgets::filled_button("marks-apply", t("marks-apply", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| this.put_marks(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn edge_tip(place: StampPlace) -> &'static str {
    match place {
        StampPlace::TopLeft => "marks-top-left",
        StampPlace::TopCentre => "marks-top",
        StampPlace::TopRight => "marks-top-right",
        StampPlace::BottomLeft => "marks-bottom-left",
        StampPlace::BottomRight => "marks-bottom-right",
        _ => "marks-bottom",
    }
}
