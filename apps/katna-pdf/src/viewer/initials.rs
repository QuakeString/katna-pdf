// SPDX-License-Identifier: GPL-3.0-or-later

//! Initials: a few letters typed in a hand-like face, a picture of
//! them, or drawn with the pointer on a pad, kept across files and put
//! into a page's own content with a click (or sized by a drag), or on
//! every page at the bottom right, like Master PDF Editor's. Made in
//! the Your initials card (E08); the ones kept show in a bar while the
//! tool is in hand. They are kept in `initials.txt` in the state
//! folder, a `key=value` block each; a picture's file is copied beside
//! it.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{AnyElement, Bounds, Context, Entity, Pixels, Subscription, div, prelude::*};
use katna_pdf_cos::Insert;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::{px, unpx};

use super::Viewer;
use super::insert::{Adding, INKS};
use crate::float::{self, FloatTip};
use crate::text::t;

/// How wide initials put on with a click are, in points.
const WIDTH: f32 = 72.0;
/// The size text initials put on with a click are, in points.
const TEXT_SIZE: f32 = 24.0;
/// How thick drawn initials are, in points, at [`WIDTH`].
const LINE_WIDTH: f32 = 1.5;
/// How far in from a page's bottom right corner initials put on every
/// page go, in points.
const MARGIN: f32 = 36.0;
/// How high the drawing pad is, and how thick its lines.
const PAD_HEIGHT: f32 = 130.0;
const PAD_LINE: f32 = 2.5;
/// The faces initials can be typed in: a plain, a serif and a typewriter
/// face, slanted and bold like a hand.
pub(super) const FONTS: [(&str, &str); 3] = [
    ("Helvetica-BoldOblique", "initials-font-plain"),
    ("Times-BoldItalic", "initials-font-serif"),
    ("Courier-BoldOblique", "initials-font-mono"),
];

/// One set of initials kept.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Initial {
    Text {
        text: String,
        font: String,
    },
    Picture {
        file: PathBuf,
    },
    /// Lines, their points scaled so the whole is one wide, y down.
    Drawn {
        strokes: Vec<Vec<[f32; 2]>>,
    },
}

impl Initial {
    pub(super) fn label(&self, n: usize) -> String {
        match self {
            Initial::Text { text, .. } => text.clone(),
            Initial::Picture { .. } => t("initials-picture", &[("n", &n.to_string())]),
            Initial::Drawn { .. } => t("initials-drawn", &[("n", &n.to_string())]),
        }
    }
}

/// Which way the card makes initials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Typed,
    Drawn,
    Picture,
}

/// The Your initials card (E08).
struct Card {
    tab: Tab,
    /// Lines drawn on the pad so far, in its own design pixels.
    strokes: Vec<Vec<[f32; 2]>>,
    pressed: bool,
    /// Put the initials made on every page as well.
    every_page: bool,
    /// Where the pad was last laid out.
    pad: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// The initials tool's state while it is in hand.
pub(super) struct Initials {
    pub kept: Vec<Initial>,
    /// Which of `kept` the next click puts on.
    pub chosen: usize,
    /// The face new typed initials get, into `FONTS`.
    font: usize,
    card: Option<Card>,
    input: Entity<TextInput>,
    _sub: Subscription,
}

fn folder() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf"))
}

fn file() -> Option<PathBuf> {
    folder().map(|f| f.join("initials.txt"))
}

pub(super) fn load() -> Vec<Initial> {
    let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    text.split("\n\n").filter_map(parse).collect()
}

/// Points as written: `x,y;x,y`, lines apart by `|`.
fn parse_strokes(value: &str) -> Vec<Vec<[f32; 2]>> {
    value
        .split('|')
        .map(|stroke| {
            stroke
                .split(';')
                .filter_map(|p| {
                    let (x, y) = p.split_once(',')?;
                    Some([x.parse().ok()?, y.parse().ok()?])
                })
                .collect::<Vec<_>>()
        })
        .filter(|stroke| stroke.len() >= 2)
        .collect()
}

fn parse(block: &str) -> Option<Initial> {
    let mut kind = "";
    let mut text = String::new();
    let mut font = FONTS[0].0.to_owned();
    let mut file = PathBuf::new();
    let mut strokes = Vec::new();
    for line in block.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "kind" => kind = value,
            "text" => text = value.to_owned(),
            "font" => font = value.to_owned(),
            "file" => file = PathBuf::from(value),
            "points" => strokes = parse_strokes(value),
            _ => {}
        }
    }
    match kind {
        "text" if !text.is_empty() => Some(Initial::Text { text, font }),
        "picture" if file.is_file() => Some(Initial::Picture { file }),
        "drawn" if !strokes.is_empty() => Some(Initial::Drawn { strokes }),
        _ => None,
    }
}

fn block(initial: &Initial) -> String {
    match initial {
        Initial::Text { text, font } => {
            format!("kind=text\ntext={}\nfont={font}\n", text.replace('\n', " "))
        }
        Initial::Picture { file } => format!("kind=picture\nfile={}\n", file.display()),
        Initial::Drawn { strokes } => {
            let strokes: Vec<String> = strokes
                .iter()
                .map(|stroke| {
                    let points: Vec<String> = stroke
                        .iter()
                        .map(|[x, y]| format!("{x:.4},{y:.4}"))
                        .collect();
                    points.join(";")
                })
                .collect();
            format!("kind=drawn\npoints={}\n", strokes.join("|"))
        }
    }
}

fn save(kept: &[Initial]) {
    let Some(file) = file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let blocks: Vec<String> = kept.iter().map(block).collect();
    let _ = std::fs::write(file, blocks.join("\n"));
}

/// Copies picture `from` into the state folder, so the initials outlive
/// the file they came from.
fn keep_picture(from: &Path) -> Result<PathBuf, String> {
    let dir = folder()
        .ok_or_else(|| "no state folder".to_owned())?
        .join("initials");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ext = from
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let n = (1..)
        .find(|n| !dir.join(format!("{n}.{ext}")).exists())
        .unwrap_or(1);
    let to = dir.join(format!("{n}.{ext}"));
    std::fs::copy(from, &to).map_err(|e| e.to_string())?;
    Ok(to)
}

/// Lines drawn (y down) scaled together to one wide, from their top
/// left.
fn normalise(strokes: &[Vec<[f32; 2]>]) -> Vec<Vec<[f32; 2]>> {
    let (mut x0, mut y0, mut x1) = (f32::MAX, f32::MAX, f32::MIN);
    for p in strokes.iter().flatten() {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
    }
    let w = (x1 - x0).max(1.0);
    strokes
        .iter()
        .map(|stroke| {
            stroke
                .iter()
                .map(|p| [(p[0] - x0) / w, (p[1] - y0) / w])
                .collect()
        })
        .collect()
}

/// How high drawn initials one wide are.
fn drawn_height(strokes: &[Vec<[f32; 2]>]) -> f32 {
    strokes.iter().flatten().fold(0.0, |h, p| h.max(p[1]))
}

impl Viewer {
    /// Gets the initials tool ready: the ones kept, and the card when
    /// there are none yet.
    fn initials_state(&mut self, cx: &mut Context<Self>) {
        if self.initials.is_some() {
            return;
        }
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("initials-type", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.use_card(cx),
            InputEvent::Changed => cx.notify(),
            _ => {}
        });
        let kept = load();
        let card = kept.is_empty().then(Card::new);
        self.initials = Some(Initials {
            chosen: kept.len().saturating_sub(1),
            kept,
            font: 0,
            card,
            input,
            _sub: sub,
        });
        self.focus_initials = true;
    }

    /// The initials tool's bar above the tools: the initials kept, to
    /// pick the one a click puts on, and ways to make more.
    pub(super) fn initials_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.adding != Some(Adding::Initials) {
            self.initials = None;
            return None;
        }
        self.initials_state(cx);
        let th = self.theme;
        let initials = self.initials.as_ref()?;
        let mut row = div()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S3))
            .py(px(space::S2));
        if initials.kept.is_empty() {
            row = row.child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("initials-none", &[])),
            );
        }
        for (i, initial) in initials.kept.iter().enumerate() {
            let on = initials.chosen == i;
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        float::chip(("initial", i), initial.label(i + 1), on, &th)
                            .ftip(t("initials-use", &[]), &th)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(initials) = this.initials.as_mut() {
                                    initials.chosen = i;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        float::sized_ib(
                            ("forget-initial", i),
                            "x",
                            22.0,
                            12.0,
                            t("initials-forget", &[]),
                            false,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(initials) = this.initials.as_mut()
                                && i < initials.kept.len()
                            {
                                initials.kept.remove(i);
                                initials.chosen =
                                    initials.chosen.min(initials.kept.len().saturating_sub(1));
                                save(&initials.kept);
                            }
                            cx.notify();
                        })),
                    ),
            );
        }
        row = row.child(float::vsep(&th)).child(
            float::chip("initials-new", t("initials-new", &[]), false, &th).on_click(cx.listener(
                |this, _, _, cx| {
                    if let Some(initials) = this.initials.as_mut() {
                        initials.card = Some(Card::new());
                    }
                    this.focus_initials = true;
                    cx.notify();
                },
            )),
        );
        if !initials.kept.is_empty() {
            row = row.child(
                float::chip(
                    "initials-every-page",
                    t("initials-every-page-now", &[]),
                    false,
                    &th,
                )
                .ftip(t("initials-every-page-tip", &[]), &th)
                .on_click(cx.listener(|this, _, _, cx| this.initials_on_every_page(cx))),
            );
        }
        Some(row.into_any_element())
    }

    /// Gives the typing field focus once the card opens on Typed, and
    /// the keys back to the pages once it is gone, for Ctrl+S and the
    /// rest.
    pub(super) fn focus_initials_field(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let focus = std::mem::take(&mut self.focus_initials);
        let Some(initials) = &self.initials else {
            return;
        };
        let handle = gpui::Focusable::focus_handle(initials.input.read(cx), cx);
        let typing = initials.card.as_ref().is_some_and(|c| c.tab == Tab::Typed);
        if focus && typing {
            window.focus(&handle, cx);
        } else if !typing && handle.is_focused(window) {
            window.focus(&self.focus, cx);
        }
    }

    /// The Your initials card (E08), over the dimmed page.
    pub(super) fn initials_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let initials = self.initials.as_ref()?;
        let card = initials.card.as_ref()?;
        let th = self.theme;
        let tabs = [
            (Tab::Typed, "initials-typed"),
            (Tab::Drawn, "initials-drawn-tab"),
            (Tab::Picture, "initials-a-picture"),
        ]
        .map(|(tab, words)| {
            float::chip(
                ("initials-tab", tab as usize),
                t(words, &[]),
                card.tab == tab,
                &th,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut()) {
                    card.tab = tab;
                }
                this.focus_initials = tab == Tab::Typed;
                cx.notify();
            }))
        });
        let body = match card.tab {
            Tab::Typed => {
                let fonts = FONTS.into_iter().enumerate().map(|(i, (_, label))| {
                    float::chip(("initials-font", i), t(label, &[]), initials.font == i, &th)
                        .ftip(t("initials-font-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(initials) = this.initials.as_mut() {
                                initials.font = i;
                            }
                            cx.notify();
                        }))
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .h(px(48.0))
                            .flex()
                            .items_center()
                            .px(px(space::S3))
                            .rounded(px(radius::SM))
                            .bg(th.field)
                            .border_1()
                            .border_color(th.line)
                            .text_size(px(text::TITLE))
                            .text_color(th.text)
                            .child(initials.input.clone()),
                    )
                    .child(div().flex().gap(px(space::S2)).children(fonts))
                    .into_any_element()
            }
            Tab::Drawn => self.initials_pad(card, cx),
            Tab::Picture => div()
                .h(px(PAD_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(space::S3))
                .rounded(px(radius::SM))
                .border_1()
                .border_dashed()
                .border_color(th.line)
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(t("initials-picture-how", &[])),
                )
                .child(
                    float::button(
                        "initials-from-picture",
                        t("initials-from-picture", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.choose_initials_picture(cx))),
                )
                .into_any_element(),
        };
        let clear = (card.tab == Tab::Drawn).then(|| {
            div()
                .id("initials-clear")
                .cursor_pointer()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.text)
                .child(t("initials-clear", &[]))
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut()) {
                        card.strokes.clear();
                    }
                    cx.notify();
                }))
        });
        let every = card.every_page;
        let tick = div()
            .id("initials-every")
            .flex()
            .items_center()
            .gap(px(space::S2))
            .cursor_pointer()
            .child(
                div()
                    .size(px(16.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .map(|d| {
                        if every {
                            d.bg(th.accent)
                        } else {
                            d.border_1().border_color(th.dim)
                        }
                    })
                    .children(every.then(|| float::icon("check", 12.0, th.on_accent))),
            )
            .child(
                div()
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(t("initials-every-page", &[])),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut()) {
                    card.every_page = !card.every_page;
                }
                cx.notify();
            }));
        let ready = match card.tab {
            Tab::Typed => !initials.input.read(cx).text().trim().is_empty(),
            Tab::Drawn => !card.strokes.is_empty(),
            Tab::Picture => false,
        };
        let card = div()
            .id("initials-card")
            .occlude()
            .w(px(440.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(th.card)
            .shadow(th.lifted())
            .when(th.dark, |d| d.border_1().border_color(th.line))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(space::S4))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .text_size(px(text::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(t("initials-title", &[])),
                    )
                    .child(div().flex().gap(px(space::S2)).children(tabs))
                    .child(body)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().children(clear))
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(t("initials-kept-here", &[])),
                            ),
                    )
                    .child(tick),
            )
            .child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .border_t_1()
                    .border_color(th.line_soft)
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        float::button("initials-cancel", t("initials-cancel", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_initials_card(cx);
                            })),
                    )
                    .child(
                        float::button(
                            "initials-use-card",
                            t("initials-use-button", &[]),
                            true,
                            &th,
                        )
                        .when(!ready, |d| d.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.use_card(cx))),
                    ),
            )
            .into_any_element();
        Some(super::dialog(
            th,
            card,
            cx.listener(|this, _, _, cx| this.close_initials_card(cx)),
        ))
    }

    /// The pad initials are drawn on with the pointer.
    fn initials_pad(&self, card: &Card, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let strokes = card.strokes.clone();
        let pad = card.pad.clone();
        let ink = th.text;
        div()
            .id("initials-pad")
            .relative()
            .h(px(PAD_HEIGHT))
            .rounded(px(radius::SM))
            .border_1()
            .border_dashed()
            .border_color(th.line)
            .cursor_crosshair()
            .child(
                gpui::canvas(
                    move |bounds, _, _| pad.set(Some(bounds)),
                    move |bounds, _, window, _| {
                        for stroke in &strokes {
                            let mut builder = gpui::PathBuilder::stroke(px(PAD_LINE));
                            for (i, p) in stroke.iter().enumerate() {
                                let at = bounds.origin + gpui::point(px(p[0]), px(p[1]));
                                if i == 0 {
                                    builder.move_to(at);
                                } else {
                                    builder.line_to(at);
                                }
                            }
                            if stroke.len() > 1
                                && let Ok(path) = builder.build()
                            {
                                window.paint_path(path, ink);
                            }
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut())
                        && let Some(at) = card.local(event.position)
                    {
                        card.pressed = true;
                        card.strokes.push(vec![at]);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut())
                    && card.pressed
                    && let Some(at) = card.local(event.position)
                    && let Some(stroke) = card.strokes.last_mut()
                {
                    if event.pressed_button != Some(gpui::MouseButton::Left) {
                        card.pressed = false;
                        return;
                    }
                    let far = stroke
                        .last()
                        .is_none_or(|l| (l[0] - at[0]).hypot(l[1] - at[1]) >= 1.5);
                    if far {
                        stroke.push(at);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(card) = this.initials.as_mut().and_then(|i| i.card.as_mut()) {
                        card.pressed = false;
                        // A dot is a line too short to keep.
                        card.strokes.retain(|s| s.len() > 1);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn close_initials_card(&mut self, cx: &mut Context<Self>) {
        if let Some(initials) = self.initials.as_mut() {
            initials.card = None;
        }
        cx.notify();
    }

    /// Keeps what the card made, and puts it on every page when asked.
    fn use_card(&mut self, cx: &mut Context<Self>) {
        let Some(initials) = self.initials.as_mut() else {
            return;
        };
        let Some(card) = initials.card.as_ref() else {
            return;
        };
        let initial = match card.tab {
            Tab::Typed => {
                let text = initials.input.read(cx).text().trim().to_owned();
                if text.is_empty() {
                    return;
                }
                let font = FONTS[initials.font].0.to_owned();
                initials
                    .input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                Initial::Text { text, font }
            }
            Tab::Drawn if !card.strokes.is_empty() => Initial::Drawn {
                strokes: normalise(&card.strokes),
            },
            _ => return,
        };
        let every_page = card.every_page;
        initials.card = None;
        self.keep_initials(initial);
        if every_page {
            self.initials_on_every_page(cx);
        }
        cx.notify();
    }

    /// Keeps `initial` and makes it the one the next click puts on.
    fn keep_initials(&mut self, initial: Initial) {
        let Some(initials) = self.initials.as_mut() else {
            return;
        };
        initials.kept.push(initial);
        initials.chosen = initials.kept.len() - 1;
        save(&initials.kept);
    }

    fn choose_initials_picture(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("initials-picture-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(from) = paths.into_iter().next() else {
                return;
            };
            let kept = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&from).map_err(|e| e.to_string())?;
                    katna_pdf_sign::page_image(&bytes)?;
                    keep_picture(&from)
                })
                .await;
            this.update(cx, |this, cx| {
                match kept {
                    Ok(file) => {
                        let every_page = this
                            .initials
                            .as_mut()
                            .and_then(|i| i.card.take())
                            .is_some_and(|c| c.every_page);
                        this.keep_initials(Initial::Picture { file });
                        if every_page {
                            this.initials_on_every_page(cx);
                        }
                    }
                    Err(error) => {
                        this.notice = Some(super::said("text-edit-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What putting the chosen initials on page `page` draws: in `boxed`
    /// (on the page shown) or [`WIDTH`] wide from `at`.
    fn initials_items(
        &self,
        initial: &Initial,
        page: usize,
        boxed: Option<[f32; 4]>,
        at: [f32; 2],
    ) -> Result<Vec<Insert>, String> {
        let inv = self.inverse(page).ok_or_else(|| "no page".to_owned())?;
        let ink = INKS[self.ink];
        let width = boxed.map_or(WIDTH, |[l, _, r, _]| (r - l).max(4.0));
        Ok(match initial {
            Initial::Text { text, font } => {
                let size = boxed.map_or(TEXT_SIZE, |[_, t, _, b]| ((b - t) / 1.2).max(4.0));
                vec![Insert::Text {
                    at: inv.map([at[0], at[1] + size * 0.95]),
                    text: text.clone(),
                    font: font.clone(),
                    size,
                    colour: ink,
                }]
            }
            Initial::Picture { file } => {
                let image = std::fs::read(file)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| katna_pdf_sign::page_image(&bytes))?;
                self.picture_item(page, boxed, at, image, WIDTH)
                    .into_iter()
                    .collect()
            }
            Initial::Drawn { strokes } => strokes
                .iter()
                .map(|points| Insert::Path {
                    points: points
                        .iter()
                        .map(|p| inv.map([at[0] + p[0] * width, at[1] + p[1] * width]))
                        .collect(),
                    stroke: ink,
                    width: LINE_WIDTH * width / WIDTH,
                })
                .collect(),
        })
    }

    /// Puts the chosen initials on page `page`, in `boxed` (on the page
    /// shown) or [`WIDTH`] wide from `at`.
    pub(super) fn place_initials(
        &mut self,
        page: usize,
        boxed: Option<[f32; 4]>,
        at: [f32; 2],
        cx: &mut Context<Self>,
    ) {
        let Some(initial) = self.chosen_initial() else {
            self.notice = Some(super::said("initials-none", &[]));
            cx.notify();
            return;
        };
        match self.initials_items(&initial, page, boxed, at) {
            Ok(items) if !items.is_empty() => self.change_page(
                page,
                move |doc| doc.insert(page, &items).map_err(|e| e.to_string()),
                cx,
            ),
            Ok(_) => {}
            Err(error) => {
                self.notice = Some(super::said("text-edit-failed", &[("error", &error)]));
                cx.notify();
            }
        }
    }

    fn chosen_initial(&self) -> Option<Initial> {
        self.initials
            .as_ref()
            .and_then(|i| i.kept.get(i.chosen))
            .cloned()
    }

    /// Puts the chosen initials on every page, [`MARGIN`] in from its
    /// bottom right corner.
    fn initials_on_every_page(&mut self, cx: &mut Context<Self>) {
        let Some(initial) = self.chosen_initial() else {
            return;
        };
        let super::Content::Ready(opened) = &self.content else {
            return;
        };
        let doc = opened.doc.clone();
        // How big the initials are on a page, in points.
        let (w, h) = match &initial {
            Initial::Text { text, .. } => (
                TEXT_SIZE * 0.6 * text.chars().count().max(1) as f32,
                TEXT_SIZE * 1.2,
            ),
            Initial::Picture { .. } => (WIDTH, WIDTH * 0.5),
            Initial::Drawn { strokes } => (WIDTH, WIDTH * drawn_height(strokes)),
        };
        let mut changes = Vec::new();
        for page in 0..self.page_count() {
            let Some((pw, ph)) = doc.page_size(page) else {
                continue;
            };
            let at = [(pw - MARGIN - w).max(0.0), (ph - MARGIN - h).max(0.0)];
            match self.initials_items(&initial, page, None, at) {
                Ok(items) => changes.push((page, items)),
                Err(error) => {
                    self.notice = Some(super::said("text-edit-failed", &[("error", &error)]));
                    cx.notify();
                    return;
                }
            }
        }
        if changes.is_empty() {
            return;
        }
        let pages: Vec<usize> = changes.iter().map(|(p, _)| *p).collect();
        let key = self.key.clone();
        self.change_document(
            pages,
            "text-edit-failed",
            move |doc| {
                let mut bytes: Option<Vec<u8>> = None;
                for (page, items) in changes {
                    let next = match &bytes {
                        None => doc.insert(page, &items),
                        Some(b) => super::cos(b.clone(), key.as_deref())?.insert(page, &items),
                    };
                    bytes = Some(next.map_err(|e| e.to_string())?);
                }
                bytes.ok_or_else(|| "no pages".to_owned())
            },
            cx,
        );
    }
}

impl Card {
    fn new() -> Self {
        Card {
            tab: Tab::Typed,
            strokes: Vec::new(),
            pressed: false,
            every_page: false,
            pad: Rc::new(Cell::new(None)),
        }
    }

    /// A point on the window as a point on the pad, when it is on it.
    fn local(&self, at: gpui::Point<Pixels>) -> Option<[f32; 2]> {
        let bounds = self.pad.get()?;
        bounds
            .contains(&at)
            .then(|| [unpx(at.x - bounds.origin.x), unpx(at.y - bounds.origin.y)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_initials_scale_to_one_wide() {
        let strokes = normalise(&[
            vec![[10.0, 20.0], [60.0, 40.0]],
            vec![[35.0, 10.0], [40.0, 12.0]],
        ]);
        assert_eq!(
            strokes,
            vec![vec![[0.0, 0.2], [1.0, 0.6]], vec![[0.5, 0.0], [0.6, 0.04]]]
        );
        assert!((drawn_height(&strokes) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn blocks_round_trip() {
        let drawn = Initial::Drawn {
            strokes: vec![vec![[0.0, 0.2], [1.0, 0.6]], vec![[0.5, 0.0], [0.5, 0.5]]],
        };
        assert_eq!(parse(&block(&drawn)), Some(drawn));
        // One line, as earlier versions wrote it.
        assert_eq!(
            parse("kind=drawn\npoints=0.0000,0.2000;1.0000,0.6000\n"),
            Some(Initial::Drawn {
                strokes: vec![vec![[0.0, 0.2], [1.0, 0.6]]]
            })
        );
        assert_eq!(
            parse("kind=text\ntext=MR\nfont=Times-BoldItalic\n"),
            Some(Initial::Text {
                text: "MR".into(),
                font: "Times-BoldItalic".into()
            })
        );
        assert_eq!(parse("kind=picture\nfile=/nowhere/1.png\n"), None);
    }
}
