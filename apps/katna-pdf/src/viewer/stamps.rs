// SPDX-License-Identifier: GPL-3.0-or-later

//! The stamp tool's row of stamps: the standard ones, the dynamic ones
//! that say who put them on and when, and the person's own, typed in
//! and kept; and files attached as comments.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, div, prelude::*};
use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, DYNAMIC_STAMPS, MARKS, STAMPS, custom_stamp, stamp_label,
};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::Viewer;
use crate::annotate::Tool;
use crate::float::{self, FloatTip};
use crate::text::t;

/// Where the person's own stamps are kept, a line each.
fn custom_file() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("stamps.txt"))
}

/// The person's own stamps, as kept.
pub(super) fn load_custom() -> Vec<String> {
    custom_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn save_custom(stamps: &[String]) {
    let Some(file) = custom_file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut text = stamps.join("\n");
    text.push('\n');
    let _ = std::fs::write(file, text);
}

impl Viewer {
    /// The row under the tools while the stamp tool is in hand: which
    /// stamp the next click puts on.
    pub(super) fn stamp_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.tool != Some(Tool::Stamp) {
            self.stamp_input = None;
            return None;
        }
        let th = self.theme;
        if self.stamp_input.is_none() {
            let accent = gpui::rgba(th.katna.accent).into();
            let input = cx.new(|cx| {
                let mut input = TextInput::new(t("stamp-custom-placeholder", &[]), cx);
                input.set_accent(accent);
                input
            });
            let sub = cx.subscribe(&input, |this, input, event, cx| {
                if let InputEvent::Submit = event {
                    let words = input.read(cx).text().trim().to_owned();
                    if !words.is_empty() {
                        let name = custom_stamp(&words);
                        if !this.custom_stamps.contains(&name) {
                            this.custom_stamps.push(name.clone());
                            save_custom(&this.custom_stamps);
                        }
                        this.stamp = name;
                        input.update(cx, |input, cx| input.set_text(String::new(), cx));
                        cx.notify();
                    }
                }
            });
            self.stamp_input = Some((input, sub));
        }
        let tab = self.stamp_tab;
        let tabs = [
            "stamp-tab-standard",
            "stamp-tab-dated",
            "stamp-tab-mine",
            "stamp-tab-marks",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, words)| {
            float::chip(("stamp-tab", i), t(words, &[]), tab == i, &th).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.stamp_tab = i;
                    cx.notify();
                },
            ))
        });
        // A stamp as it will look: its words in its colour, in a box of
        // that colour.
        let cell = |id: gpui::ElementId, name: &str, under: Option<String>, on: bool| {
            let tone = tone(name, &th);
            div()
                .id(id)
                .w(px(124.0))
                .h(px(36.0))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .border_2()
                .border_color(tone)
                .text_color(tone)
                .text_size(px(text::CAPTION))
                .font_weight(gpui::FontWeight::BOLD)
                .cursor_pointer()
                .overflow_hidden()
                .when(on, |d| d.bg(tone.opacity(0.14)))
                .hover(move |s| s.bg(tone.opacity(0.08)))
                .child(div().truncate().px(px(space::S1)).child(stamp_label(name)))
                .children(under.map(|u| {
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(u)
                }))
        };
        let mut grid = div().flex().flex_wrap().gap(px(space::S2));
        match tab {
            0 => {
                for (i, name) in STAMPS.into_iter().enumerate() {
                    grid = grid.child(
                        cell(("stamp", i).into(), name, None, self.stamp == name).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.stamp = name.into();
                                cx.notify();
                            }),
                        ),
                    );
                }
            }
            1 => {
                let under = t("stamp-dated-under", &[]);
                for (i, name) in DYNAMIC_STAMPS.into_iter().enumerate() {
                    grid = grid.child(
                        cell(
                            ("dynamic-stamp", i).into(),
                            name,
                            Some(under.clone()),
                            self.stamp == name,
                        )
                        .ftip(t("stamp-dynamic-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stamp = name.into();
                            cx.notify();
                        })),
                    );
                }
            }
            3 => {
                for (i, name) in MARKS.into_iter().enumerate() {
                    let on = self.stamp == name;
                    grid = grid.child(
                        div()
                            .id(("mark", i))
                            .size(px(40.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(if on { th.accent } else { th.line })
                            .text_color(th.text)
                            .text_size(px(text::TITLE))
                            .cursor_pointer()
                            .when(on, |d| d.bg(th.accent.opacity(0.14)))
                            .hover(|s| s.bg(th.accent.opacity(0.08)))
                            .ftip(
                                t(
                                    &format!("stamp-mark-{}", name[2..].to_ascii_lowercase()),
                                    &[],
                                ),
                                &th,
                            )
                            .child(stamp_label(name))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stamp = name.into();
                                cx.notify();
                            })),
                    );
                }
            }
            _ => {
                for (i, name) in self.custom_stamps.clone().into_iter().enumerate() {
                    let picked = name.clone();
                    grid = grid.child(
                        div()
                            .relative()
                            .child(
                                cell(("custom-stamp", i).into(), &name, None, self.stamp == name)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stamp.clone_from(&picked);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div().absolute().top(px(-6.0)).right(px(-6.0)).child(
                                    div()
                                        .id(("forget-stamp", i))
                                        .size(px(18.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_full()
                                        .bg(th.inverse)
                                        .cursor_pointer()
                                        .ftip(t("stamp-custom-remove", &[]), &th)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.custom_stamps.retain(|s| *s != name);
                                            save_custom(&this.custom_stamps);
                                            if this.stamp == name {
                                                this.stamp = "Approved".into();
                                            }
                                            cx.notify();
                                        }))
                                        .child(float::icon("x", 11.0, th.on_inverse)),
                                ),
                            ),
                    );
                }
                if let Some((input, _)) = &self.stamp_input {
                    grid = grid.child(
                        div()
                            .w(px(256.0))
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .px(px(space::S3))
                            .rounded(px(4.0))
                            .border_1()
                            .border_dashed()
                            .border_color(th.line)
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(input.clone()),
                    );
                }
            }
        }
        Some(
            div()
                .id("stamp-card")
                .occlude()
                .w(px(420.0))
                .max_w_full()
                .p(px(space::S3))
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .rounded(px(10.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .child(div().flex().gap(px(space::S1)).children(tabs))
                .child(grid)
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(t("stamp-card-hint", &[])),
                )
                .into_any_element(),
        )
    }

    /// Asks which file to attach as comment `a` (a paper clip placed on
    /// the page), reads it and adds the comment.
    pub(super) fn attach_file(&mut self, a: Annotation, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_executor()
                .spawn(async move {
                    let data = std::fs::read(&file).map_err(|e| e.to_string())?;
                    Ok::<_, String>((super::file_name(&file), data))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok((name, data)) => {
                        let mut a = a;
                        a.kind = AnnotKind::FileAttachment {
                            name,
                            data: Some(data),
                        };
                        this.comment_next = true;
                        this.change_annotations(
                            AnnotChanges {
                                put: vec![a],
                                remove: Vec::new(),
                            },
                            cx,
                        );
                    }
                    Err(error) => this.notice = Some(t("attach-failed", &[("error", &error)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks where to save the file the chosen comment keeps, and saves
    /// it there.
    pub(super) fn save_attachment(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(a)) = (self.path.clone(), self.chosen_annotation().cloned()) else {
            return;
        };
        let AnnotKind::FileAttachment { name, .. } = &a.kind else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let suggested = Path::new(name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "attachment".into());
        let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
        let key = self.key.clone();
        let current = self.unsaved();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
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
                    let data = doc
                        .attached_file(&a)
                        .ok_or_else(|| "the file is not in the document".to_owned())?;
                    std::fs::write(&target, data).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t(
                        "comment-saved-file",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => t("comments-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A stamp's colour in the card, by what it says.
fn tone(name: &str, th: &crate::theme::Theme) -> gpui::Hsla {
    let word = name.trim_start_matches("#D").to_ascii_lowercase();
    if ["approved", "final", "received", "asis", "sold"]
        .iter()
        .any(|w| word == *w)
    {
        gpui::rgb(0x1f9d55).into()
    } else if word.starts_with("not")
        || ["confidential", "topsecret", "expired", "void", "rejected"]
            .iter()
            .any(|w| word == *w)
    {
        th.error
    } else {
        th.accent
    }
}
