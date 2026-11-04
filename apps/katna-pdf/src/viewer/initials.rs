// SPDX-License-Identifier: GPL-3.0-or-later

//! Initials: a few letters typed in a hand-like face, a picture of
//! them, or a line drawn with the pointer, kept across files and put
//! into a page's own content with a click (or sized by a drag), like
//! Master PDF Editor's. They are kept in `initials.txt` in the state
//! folder, a `key=value` block each; a picture's file is copied beside
//! it.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::Insert;
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::Viewer;
use super::insert::{Adding, INKS};
use crate::text::t;

/// How wide initials put on with a click are, in points.
const WIDTH: f32 = 72.0;
/// The size text initials put on with a click are, in points.
const TEXT_SIZE: f32 = 24.0;
/// How thick drawn initials are, in points, at [`WIDTH`].
const LINE_WIDTH: f32 = 1.5;
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
    /// Points on a line, scaled so it is one wide, y down.
    Drawn {
        points: Vec<[f32; 2]>,
    },
}

impl Initial {
    fn label(&self, n: usize) -> String {
        match self {
            Initial::Text { text, .. } => text.clone(),
            Initial::Picture { .. } => t("initials-picture", &[("n", &n.to_string())]),
            Initial::Drawn { .. } => t("initials-drawn", &[("n", &n.to_string())]),
        }
    }
}

/// The initials tool's state while it is in hand.
pub(super) struct Initials {
    pub kept: Vec<Initial>,
    /// Which of `kept` the next click puts on.
    pub chosen: usize,
    /// The face new typed initials get, into `FONTS`.
    font: usize,
    /// The next line drawn on a page is kept as initials.
    pub drawing: bool,
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

fn parse(block: &str) -> Option<Initial> {
    let mut kind = "";
    let mut text = String::new();
    let mut font = FONTS[0].0.to_owned();
    let mut file = PathBuf::new();
    let mut points = Vec::new();
    for line in block.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "kind" => kind = value,
            "text" => text = value.to_owned(),
            "font" => font = value.to_owned(),
            "file" => file = PathBuf::from(value),
            "points" => {
                points = value
                    .split(';')
                    .filter_map(|p| {
                        let (x, y) = p.split_once(',')?;
                        Some([x.parse().ok()?, y.parse().ok()?])
                    })
                    .collect();
            }
            _ => {}
        }
    }
    match kind {
        "text" if !text.is_empty() => Some(Initial::Text { text, font }),
        "picture" if file.is_file() => Some(Initial::Picture { file }),
        "drawn" if points.len() >= 2 => Some(Initial::Drawn { points }),
        _ => None,
    }
}

fn save(kept: &[Initial]) {
    let Some(file) = file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let blocks: Vec<String> = kept
        .iter()
        .map(|i| match i {
            Initial::Text { text, font } => {
                format!("kind=text\ntext={}\nfont={font}\n", text.replace('\n', " "))
            }
            Initial::Picture { file } => format!("kind=picture\nfile={}\n", file.display()),
            Initial::Drawn { points } => {
                let points: Vec<String> = points
                    .iter()
                    .map(|[x, y]| format!("{x:.4},{y:.4}"))
                    .collect();
                format!("kind=drawn\npoints={}\n", points.join(";"))
            }
        })
        .collect();
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

/// `points` from a page (y down) scaled to one wide, from their top
/// left.
fn normalise(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let (mut x0, mut y0, mut x1) = (f32::MAX, f32::MAX, f32::MIN);
    for p in points {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
    }
    let w = (x1 - x0).max(1.0);
    points
        .iter()
        .map(|p| [(p[0] - x0) / w, (p[1] - y0) / w])
        .collect()
}

impl Viewer {
    /// The initials tool's row under the tools: the initials kept, and
    /// how to make more.
    pub(super) fn initials_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.adding != Some(Adding::Initials) {
            self.initials = None;
            return None;
        }
        let th = self.theme;
        if self.initials.is_none() {
            let accent = gpui::rgba(th.katna.accent).into();
            let input = cx.new(|cx| {
                let mut input = TextInput::new(t("initials-type", &[]), cx);
                input.set_accent(accent);
                input
            });
            let sub = cx.subscribe(&input, |this, input, event, cx| {
                if let InputEvent::Submit = event {
                    let text = input.read(cx).text().trim().to_owned();
                    if !text.is_empty() {
                        let font = this
                            .initials
                            .as_ref()
                            .map_or(FONTS[0].0, |i| FONTS[i.font].0)
                            .to_owned();
                        this.keep_initials(Initial::Text { text, font });
                        input.update(cx, |input, cx| input.set_text(String::new(), cx));
                        cx.notify();
                    }
                }
            });
            let kept = load();
            self.initials = Some(Initials {
                chosen: kept.len().saturating_sub(1),
                kept,
                font: 0,
                drawing: false,
                input,
                _sub: sub,
            });
        }
        let initials = self.initials.as_ref()?;
        let chip = |id: gpui::ElementId, label: String, on: bool| {
            let chip = if on {
                widgets::filled_button(id, label, &th.katna)
            } else {
                widgets::outlined_button(id, label, &th.katna)
            };
            chip.h(px(26.0))
                .px(px(space::S3))
                .text_size(px(text::SMALL))
        };
        let mut row = div()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S3))
            .py(px(space::S1))
            .border_b_1()
            .border_color(th.line);
        if initials.kept.is_empty() {
            row = row.child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(gpui::rgba(th.katna.text_dim))
                    .child(t("initials-none", &[])),
            );
        }
        for (i, initial) in initials.kept.iter().enumerate() {
            let on = initials.chosen == i && !initials.drawing;
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        chip(("initial", i).into(), initial.label(i + 1), on)
                            .tip(t("initials-use", &[]), &th.katna)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(initials) = this.initials.as_mut() {
                                    initials.chosen = i;
                                    initials.drawing = false;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        widgets::icon_button(("forget-initial", i), "close", 14.0, &th.katna)
                            .size(px(20.0))
                            .tip(t("initials-forget", &[]), &th.katna)
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
        row = row.child(super::separator(th));
        row = row.child(
            div()
                .w(px(160.0))
                .h(px(28.0))
                .flex()
                .items_center()
                .px(px(space::S3))
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .child(initials.input.clone()),
        );
        for (i, (_, label)) in FONTS.into_iter().enumerate() {
            let on = initials.font == i;
            row = row.child(
                chip(("initials-font", i).into(), t(label, &[]), on)
                    .tip(t("initials-font-tip", &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(initials) = this.initials.as_mut() {
                            initials.font = i;
                        }
                        cx.notify();
                    })),
            );
        }
        row = row.child(super::separator(th));
        row = row.child(
            widgets::outlined_button(
                "initials-from-picture",
                t("initials-from-picture", &[]),
                &th.katna,
            )
            .h(px(26.0))
            .px(px(space::S3))
            .text_size(px(text::SMALL))
            .on_click(cx.listener(|this, _, _, cx| this.choose_initials_picture(cx))),
        );
        row = row.child(
            chip(
                "initials-draw".into(),
                t("initials-draw", &[]),
                initials.drawing,
            )
            .tip(t("initials-draw-tip", &[]), &th.katna)
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(initials) = this.initials.as_mut() {
                    initials.drawing = !initials.drawing;
                }
                this.placing = None;
                cx.notify();
            })),
        );
        Some(row.into_any_element())
    }

    /// Keeps `initial` and makes it the one the next click puts on.
    fn keep_initials(&mut self, initial: Initial) {
        let Some(initials) = self.initials.as_mut() else {
            return;
        };
        initials.kept.push(initial);
        initials.chosen = initials.kept.len() - 1;
        initials.drawing = false;
        save(&initials.kept);
    }

    /// Whether the next line drawn on a page is kept as initials.
    pub(super) fn drawing_initials(&self) -> bool {
        self.adding == Some(Adding::Initials) && self.initials.as_ref().is_some_and(|i| i.drawing)
    }

    /// A line drawn on a page while [`Initials::drawing`]: kept, not put
    /// on the page.
    pub(super) fn keep_drawn_initials(&mut self, path: &[[f32; 2]], cx: &mut Context<Self>) {
        if path.len() < 2 {
            if let Some(initials) = self.initials.as_mut() {
                initials.drawing = false;
            }
            return;
        }
        self.keep_initials(Initial::Drawn {
            points: normalise(path),
        });
        cx.notify();
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
                    Ok(file) => this.keep_initials(Initial::Picture { file }),
                    Err(error) => {
                        this.notice = Some(t("text-edit-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let Some(initial) = self
            .initials
            .as_ref()
            .and_then(|i| i.kept.get(i.chosen))
            .cloned()
        else {
            self.notice = Some(t("initials-none", &[]));
            cx.notify();
            return;
        };
        let Some(inv) = self.inverse(page) else {
            return;
        };
        let ink = INKS[self.ink];
        let width = boxed.map_or(WIDTH, |[l, _, r, _]| (r - l).max(4.0));
        match initial {
            Initial::Text { text, font } => {
                let size = boxed.map_or(TEXT_SIZE, |[_, t, _, b]| ((b - t) / 1.2).max(4.0));
                let item = Insert::Text {
                    at: inv.map([at[0], at[1] + size * 0.95]),
                    text,
                    font,
                    size,
                    colour: ink,
                };
                self.add_to_page(page, item, cx);
            }
            Initial::Picture { file } => {
                let image = std::fs::read(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| katna_pdf_sign::page_image(&bytes));
                match image {
                    Ok(image) => self.add_picture(page, boxed, at, image, WIDTH, cx),
                    Err(error) => {
                        self.notice = Some(t("text-edit-failed", &[("error", &error)]));
                        cx.notify();
                    }
                }
            }
            Initial::Drawn { points } => {
                let scale = width;
                let item = Insert::Path {
                    points: points
                        .iter()
                        .map(|p| inv.map([at[0] + p[0] * scale, at[1] + p[1] * scale]))
                        .collect(),
                    stroke: ink,
                    width: LINE_WIDTH * scale / WIDTH,
                };
                self.add_to_page(page, item, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_initials_scale_to_one_wide() {
        let points = normalise(&[[10.0, 20.0], [60.0, 40.0], [35.0, 10.0]]);
        assert_eq!(points, vec![[0.0, 0.2], [1.0, 0.6], [0.5, 0.0]]);
    }

    #[test]
    fn blocks_round_trip() {
        let block = "kind=drawn\npoints=0.0000,0.2000;1.0000,0.6000\n";
        assert_eq!(
            parse(block),
            Some(Initial::Drawn {
                points: vec![[0.0, 0.2], [1.0, 0.6]]
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
