// SPDX-License-Identifier: GPL-3.0-or-later

//! The stamp tool's row of stamps: the standard ones, the dynamic ones
//! that say who put them on and when, and the person's own, typed in
//! and kept; and files attached as comments.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, div, prelude::*};
use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, DYNAMIC_STAMPS, STAMPS, custom_stamp, stamp_label,
};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::Viewer;
use crate::annotate::Tool;
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
        for (i, name) in STAMPS.into_iter().enumerate() {
            let on = self.stamp == name;
            row = row.child(chip(("stamp", i).into(), stamp_label(name), on).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.stamp = name.into();
                    cx.notify();
                }),
            ));
        }
        row = row.child(super::separator(th));
        for (i, name) in DYNAMIC_STAMPS.into_iter().enumerate() {
            let on = self.stamp == name;
            row = row.child(
                chip(
                    ("dynamic-stamp", i).into(),
                    format!("{} +", stamp_label(name)),
                    on,
                )
                .tip(t("stamp-dynamic-tip", &[]), &th.katna)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.stamp = name.into();
                    cx.notify();
                })),
            );
        }
        row = row.child(super::separator(th));
        for (i, name) in self.custom_stamps.clone().into_iter().enumerate() {
            let on = self.stamp == name;
            let picked = name.clone();
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        chip(("custom-stamp", i).into(), stamp_label(&name), on).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.stamp.clone_from(&picked);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        widgets::icon_button(("forget-stamp", i), "close", 14.0, &th.katna)
                            .size(px(20.0))
                            .tip(t("stamp-custom-remove", &[]), &th.katna)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.custom_stamps.retain(|s| *s != name);
                                save_custom(&this.custom_stamps);
                                if this.stamp == name {
                                    this.stamp = "Approved".into();
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some((input, _)) = &self.stamp_input {
            row = row.child(
                div()
                    .w(px(220.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.field)
                    .text_size(px(text::SMALL))
                    .text_color(th.text)
                    .child(input.clone()),
            );
        }
        Some(row.into_any_element())
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
