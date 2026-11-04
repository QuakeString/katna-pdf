// SPDX-License-Identifier: GPL-3.0-or-later

//! The Files pane (files kept in the document: added, renamed, saved,
//! opened, taken out) and the Layers pane (the document's layers shown
//! and hidden, added, renamed and taken out), and putting picked
//! objects on a layer.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{Attachment, Layer, Ref};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::Viewer;
use super::sign::check_box;
use crate::text::t;

/// What the two panes show and have chosen.
#[derive(Default)]
pub(super) struct Extras {
    pub(super) attachments: Vec<Attachment>,
    pub(super) layers: Vec<Layer>,
    chosen_file: Option<usize>,
    chosen_layer: Option<usize>,
    /// A name being typed for a file or a layer.
    renaming: Option<Renaming>,
}

struct Renaming {
    what: Named,
    input: Entity<TextInput>,
    _input: Subscription,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Named {
    File(usize),
    Layer(usize),
}

impl Extras {
    /// The file shown changed: both lists anew, choices kept where they
    /// still point at something.
    pub(super) fn set(&mut self, attachments: Vec<Attachment>, layers: Vec<Layer>) {
        self.attachments = attachments;
        self.layers = layers;
        self.chosen_file = self.chosen_file.filter(|i| *i < self.attachments.len());
        self.chosen_layer = self.chosen_layer.filter(|i| *i < self.layers.len());
        self.renaming = None;
    }
}

impl Viewer {
    /// The Files pane: the files kept in the document.
    pub(super) fn files_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let extras = &self.extras;
        if extras.attachments.is_empty() {
            return div()
                .p(px(space::S5))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("files-none", &[]))
                .into_any_element();
        }
        let mut list = div()
            .id("files-list")
            .size_full()
            .overflow_y_scroll()
            .py(px(space::S2))
            .flex()
            .flex_col();
        for (i, a) in extras.attachments.iter().enumerate() {
            let on = extras.chosen_file == Some(i);
            let name: AnyElement = match &extras.renaming {
                Some(r) if r.what == Named::File(i) => {
                    div().h(px(26.0)).child(r.input.clone()).into_any_element()
                }
                _ => div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(a.name.clone())
                    .into_any_element(),
            };
            let mut details = Vec::new();
            if let Some(size) = a.size {
                details.push(crate::library::size_text(size));
            }
            if !a.description.is_empty() {
                details.push(a.description.clone());
            }
            list = list.child(
                div()
                    .id(("file-row", i))
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .mx(px(space::S2))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(th.field))
                    .hover(move |s| s.bg(th.hover))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.extras.chosen_file = Some(i);
                            if event.click_count() >= 2 {
                                this.open_attachment(window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        gpui::svg()
                            .path("icons/attachment.svg")
                            .size(px(18.0))
                            .flex_none()
                            .mt(px(2.0))
                            .text_color(th.dim),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(name)
                            .when(!details.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(text::SMALL))
                                        .text_color(th.dim)
                                        .child(details.join(" · ")),
                                )
                            }),
                    ),
            );
        }
        list.into_any_element()
    }

    /// The Files pane's buttons.
    pub(super) fn files_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let picked = self.extras.chosen_file.is_some();
        let button = |id: &'static str, icon: &'static str, tip: &str, enabled: bool| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .size(px(32.0))
                .tip(t(tip, &[]), &th.katna)
                .when(!enabled, |b| b.opacity(0.4))
        };
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                button("file-add", "upload", "files-add", true)
                    .on_click(cx.listener(|this, _, _, cx| this.add_attachments(cx))),
            )
            .child(
                button("file-open", "folder", "files-open", picked)
                    .on_click(cx.listener(|this, _, window, cx| this.open_attachment(window, cx))),
            )
            .child(
                button("file-save", "download", "files-save", picked)
                    .on_click(cx.listener(|this, _, _, cx| this.save_kept_file(cx))),
            )
            .child(
                button("file-rename", "rename", "files-rename", picked).on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(i) = this.extras.chosen_file {
                            this.start_naming(Named::File(i), window, cx);
                        }
                    },
                )),
            )
            .child(
                button("file-delete", "trash", "files-delete", picked)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_attachment(cx))),
            )
            .into_any_element()
    }

    /// Asks which files to keep in the document, and keeps them.
    fn add_attachments(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t("files-add", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                this.change_document(
                    Vec::new(),
                    "files-failed",
                    move |doc| {
                        let mut files = Vec::new();
                        for path in paths {
                            let data = std::fs::read(&path).map_err(|e| e.to_string())?;
                            files.push((super::file_name(&path), data, String::new()));
                        }
                        doc.attach_files(&files).map_err(|e| e.to_string())
                    },
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Takes the chosen file out of the document.
    fn delete_attachment(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self
            .extras
            .chosen_file
            .and_then(|i| self.extras.attachments.get(i))
            .cloned()
        else {
            return;
        };
        self.change_document(
            Vec::new(),
            "files-failed",
            move |doc| {
                doc.remove_attachments(std::slice::from_ref(&a.name))
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Asks where to save the chosen file, and saves it there.
    fn save_kept_file(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self
            .extras
            .chosen_file
            .and_then(|i| self.extras.attachments.get(i))
            .cloned()
        else {
            return;
        };
        let folder = self
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let suggested = std::path::Path::new(&a.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "attachment".into());
        let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            this.update(cx, |this, cx| this.write_attachment(a, target, false, cx))
                .ok();
        })
        .detach();
    }

    /// Writes the chosen file to a folder of its own and opens it with
    /// whatever opens such files.
    fn open_attachment(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self
            .extras
            .chosen_file
            .and_then(|i| self.extras.attachments.get(i))
            .cloned()
        else {
            return;
        };
        let name = std::path::Path::new(&a.name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "attachment".into());
        let dir = std::env::temp_dir()
            .join("katna-pdf")
            .join(format!("{:x}", std::process::id()));
        let target = dir.join(name);
        self.write_attachment(a, target, true, cx);
    }

    /// Writes attachment `a` to `target`, then opens it when `open`.
    fn write_attachment(
        &mut self,
        a: Attachment,
        target: std::path::PathBuf,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let key = self.key.clone();
        let current = self.unsaved();
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let data = doc
                        .attachment_data(&a)
                        .ok_or_else(|| "the file is not in the document".to_owned())?;
                    if let Some(dir) = target.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                    }
                    std::fs::write(&target, data).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(target) => {
                        if open {
                            cx.open_url(&target.to_string_lossy());
                        } else {
                            this.notice = Some(t(
                                "comment-saved-file",
                                &[("name", &super::file_name(&target))],
                            ));
                        }
                    }
                    Err(error) => this.notice = Some(t("files-failed", &[("error", &error)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts typing a new name for a file or a layer.
    fn start_naming(&mut self, what: Named, window: &mut Window, cx: &mut Context<Self>) {
        let current = match what {
            Named::File(i) => self.extras.attachments.get(i).map(|a| a.name.clone()),
            Named::Layer(i) => self.extras.layers.get(i).map(|l| l.name.clone()),
        };
        let Some(current) = current else {
            return;
        };
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("files-name", &[]), cx);
            input.set_accent(accent);
            input.set_text(current, cx);
            input.select_all_text(cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_naming(true, cx),
            InputEvent::Cancel => this.finish_naming(false, cx),
            _ => {}
        });
        let handle = input.focus_handle(cx);
        window.focus(&handle, cx);
        cx.defer_in(window, move |_, window, cx| window.focus(&handle, cx));
        self.extras.renaming = Some(Renaming {
            what,
            input,
            _input: sub,
        });
        cx.notify();
    }

    fn finish_naming(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(renaming) = self.extras.renaming.take() else {
            return;
        };
        let name = renaming.input.read(cx).text().trim().to_owned();
        if !keep || name.is_empty() {
            cx.notify();
            return;
        }
        match renaming.what {
            Named::File(i) => {
                let Some(a) = self.extras.attachments.get(i).cloned() else {
                    return;
                };
                if a.name == name {
                    cx.notify();
                    return;
                }
                self.change_document(
                    Vec::new(),
                    "files-failed",
                    move |doc| {
                        doc.rename_attachment(&a, &name, &a.description)
                            .map_err(|e| e.to_string())
                    },
                    cx,
                );
            }
            Named::Layer(i) => {
                let Some(layer) = self.extras.layers.get(i).cloned() else {
                    return;
                };
                if layer.name == name {
                    cx.notify();
                    return;
                }
                self.change_document(
                    Vec::new(),
                    "layers-failed",
                    move |doc| doc.rename_layer(layer.id, &name).map_err(|e| e.to_string()),
                    cx,
                );
            }
        }
    }

    /// The Layers pane: each layer with whether it is shown.
    pub(super) fn layers_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let extras = &self.extras;
        if extras.layers.is_empty() {
            return div()
                .p(px(space::S5))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("layers-none", &[]))
                .into_any_element();
        }
        let mut list = div()
            .id("layers-list")
            .size_full()
            .overflow_y_scroll()
            .py(px(space::S2))
            .flex()
            .flex_col();
        for (i, layer) in extras.layers.iter().enumerate() {
            let on = extras.chosen_layer == Some(i);
            let id = layer.id;
            let shown = layer.on;
            let name: AnyElement = match &extras.renaming {
                Some(r) if r.what == Named::Layer(i) => div()
                    .h(px(26.0))
                    .flex_1()
                    .child(r.input.clone())
                    .into_any_element(),
                _ => div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(text::BODY))
                    .text_color(if shown { th.text } else { th.dim })
                    .child(layer.name.clone())
                    .into_any_element(),
            };
            list = list.child(
                div()
                    .id(("layer-row", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .h(px(36.0))
                    .px(px(space::S3))
                    .mx(px(space::S2))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .when(on, |d| d.bg(th.field))
                    .hover(move |s| s.bg(th.hover))
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.extras.chosen_layer = Some(i);
                            if event.click_count() >= 2 {
                                this.start_naming(Named::Layer(i), window, cx);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .id(("layer-shown", i))
                            .flex_none()
                            .cursor_pointer()
                            .tip(
                                t(if shown { "layers-hide" } else { "layers-show" }, &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.show_layer(id, !shown, cx);
                            }))
                            .child(check_box(shown, th)),
                    )
                    .child(name),
            );
        }
        list.into_any_element()
    }

    /// The Layers pane's buttons.
    pub(super) fn layers_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let picked = self.extras.chosen_layer.is_some();
        let button = |id: &'static str, icon: &'static str, tip: &str, enabled: bool| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .size(px(32.0))
                .tip(t(tip, &[]), &th.katna)
                .when(!enabled, |b| b.opacity(0.4))
        };
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                button("layer-add", "plus", "layers-add", true)
                    .on_click(cx.listener(|this, _, _, cx| this.add_layer(None, cx))),
            )
            .child(
                button("layer-rename", "rename", "layers-rename", picked).on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(i) = this.extras.chosen_layer {
                            this.start_naming(Named::Layer(i), window, cx);
                        }
                    },
                )),
            )
            .child(
                button("layer-delete", "trash", "layers-delete", picked)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_layer(cx))),
            )
            .into_any_element()
    }

    /// Shows or hides layer `id` on every page.
    fn show_layer(&mut self, id: Ref, shown: bool, cx: &mut Context<Self>) {
        let pages: Vec<usize> = (0..self.page_count()).collect();
        self.change_document(
            pages,
            "layers-failed",
            move |doc| doc.show_layers(&[(id, shown)]).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Adds a layer, named "Layer N"; with `onto`, the picked objects of
    /// that page go on it in the same change.
    fn add_layer(&mut self, onto: Option<(usize, Vec<usize>)>, cx: &mut Context<Self>) {
        let n = self.extras.layers.len() + 1;
        let name = t("layers-new-name", &[("n", &n.to_string())]);
        let pages = onto.as_ref().map(|(p, _)| vec![*p]).unwrap_or_default();
        self.change_document(
            pages,
            "layers-failed",
            move |doc| {
                let (bytes, id) = doc.add_layer(&name).map_err(|e| e.to_string())?;
                let Some((page, indices)) = onto else {
                    return Ok(bytes);
                };
                let doc = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                doc.put_on_layer(page, &indices, Some(id))
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Takes the chosen layer out; what was on it is drawn always.
    fn delete_layer(&mut self, cx: &mut Context<Self>) {
        let Some(layer) = self
            .extras
            .chosen_layer
            .and_then(|i| self.extras.layers.get(i))
            .cloned()
        else {
            return;
        };
        let pages: Vec<usize> = (0..self.page_count()).collect();
        self.change_document(
            pages,
            "layers-failed",
            move |doc| doc.remove_layer(layer.id).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// The layer the picked objects are on: the first's, when one.
    pub(super) fn picked_layer(&self) -> Option<Ref> {
        let (page, indices) = self.picked_indices()?;
        let objects = self.runs.objects.get(&page)?;
        objects.get(*indices.first()?)?.layer
    }

    /// Puts the picked objects on the next layer along: from none to the
    /// first, then each in turn, then none again. With no layer yet, a
    /// new one is made for them.
    pub(super) fn cycle_picked_layer(&mut self, cx: &mut Context<Self>) {
        let Some((page, indices)) = self.picked_indices() else {
            return;
        };
        if self.extras.layers.is_empty() {
            self.add_layer(Some((page, indices)), cx);
            return;
        }
        let now = self.picked_layer();
        let at = now.and_then(|id| self.extras.layers.iter().position(|l| l.id == id));
        let next = match at {
            None => Some(self.extras.layers[0].id),
            Some(i) => self.extras.layers.get(i + 1).map(|l| l.id),
        };
        self.change_page(
            page,
            move |doc| {
                doc.put_on_layer(page, &indices, next)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }
}
