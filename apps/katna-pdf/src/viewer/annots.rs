// SPDX-License-Identifier: GPL-3.0-or-later

//! The viewer's annotating (docs/PLAN.md §4.2): the row of tools, the
//! pointer making annotations with them, the comments tab, and writing
//! each change into the file (`katna_pdf_cos::annots`) to be drawn from
//! there.

use std::path::Path;
use std::sync::Arc;

use gpui::{AnyElement, Context, Focusable, Window, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, Ref};
use katna_pdf_render::Document;
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::{Content, PaneTab, Viewer, separator};
use crate::annotate::{self, COLOURS, Inverse, Tool};
use crate::text::t;

/// An annotation being drawn with the pointer: its page and the
/// pointer's path, in points from the page's top left.
pub(super) struct Draft {
    pub page: usize,
    pub path: Vec<[f32; 2]>,
}

/// The comment of the annotation chosen, being typed.
pub(super) struct Comment {
    pub id: Ref,
    pub input: gpui::Entity<TextInput>,
    pub _input: gpui::Subscription,
}

impl Viewer {
    /// Shows or hides the row of annotation tools; hiding it puts the
    /// tool down.
    pub(super) fn toggle_tools(&mut self, cx: &mut Context<Self>) {
        self.tools = !self.tools;
        if !self.tools {
            self.tool = None;
            self.draft = None;
        }
        cx.notify();
    }

    fn pick_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = (self.tool != Some(tool)).then_some(tool);
        self.cropping = false;
        self.draft = None;
        // Marking text that is already selected marks it now.
        if let Some(style) = tool.markup()
            && self.tool.is_some()
            && self.selection.is_some_and(|s| s.anchor != s.head)
        {
            self.mark_selection(style, cx);
        }
        cx.notify();
    }

    /// Puts the tool down, or lets go of the annotation chosen.
    pub(super) fn cancel_tool(&mut self, cx: &mut Context<Self>) {
        let put_down = self.draft.take().is_some() | self.tool.take().is_some();
        if !put_down {
            self.choose(None, cx);
        }
        cx.notify();
    }

    /// The row of tools and colours under the toolbar, while shown.
    pub(super) fn tools_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.tools || !matches!(self.content, Content::Ready(_)) {
            return None;
        }
        let th = self.theme;
        let mut row = div()
            .id("annotation-tools")
            .h(px(44.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S3))
            .border_b_1()
            .border_color(th.line)
            .overflow_x_scroll();
        for (i, tool) in Tool::ALL.into_iter().enumerate() {
            let on = self.tool == Some(tool);
            let colour = if on {
                th.katna.accent
            } else {
                th.katna.text_dim
            };
            if i == 4 || i == 6 {
                row = row.child(separator(th));
            }
            row = row.child(
                widgets::icon_button_colored(("tool", i), tool.icon(), 18.0, colour, &th.katna)
                    .when(on, |b| b.bg(th.field))
                    .tip(t(tool.tip(), &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_tool(tool, cx))),
            );
        }
        row = row.child(separator(th));
        for (i, [r, g, b]) in COLOURS.into_iter().enumerate() {
            let on = self.colour == i;
            let fill = gpui::rgb(u32::from_be_bytes([
                0,
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ]));
            row = row.child(
                div()
                    .id(("colour", i))
                    .size(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .when(on, |d| {
                        d.border_2().border_color(gpui::rgba(th.katna.accent))
                    })
                    .hover(move |s| s.bg(th.hover))
                    .tip(t("tool-colour", &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_colour(i, cx)))
                    .child(div().size(px(18.0)).rounded_full().bg(fill)),
            );
        }
        Some(row.into_any_element())
    }

    /// Picks colour `i` for what is drawn next; with no tool in hand, it
    /// is given to the annotation chosen.
    fn pick_colour(&mut self, i: usize, cx: &mut Context<Self>) {
        self.colour = i;
        if self.tool.is_none()
            && let Some(mut a) = self.chosen_annotation().cloned()
            && !matches!(a.kind, AnnotKind::Other(_))
        {
            a.colour = COLOURS[i];
            self.change_annotations(
                AnnotChanges {
                    put: vec![a],
                    remove: Vec::new(),
                },
                cx,
            );
        }
        cx.notify();
    }

    /// The page under `position` in the window and the place on it, in
    /// points from its top left.
    pub(super) fn page_point(
        &self,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<(usize, [f32; 2])> {
        let shown = desktop_px(self.shown);
        let bounds = self.page_bounds.borrow();
        let (&page, b) = bounds.iter().find(|(_, b)| b.contains(&position))?;
        Some((
            page,
            [
                (position.x - b.left()) / shown,
                (position.y - b.top()) / shown,
            ],
        ))
    }

    /// The map from page `page` shown back to its own space.
    pub(super) fn inverse(&self, page: usize) -> Option<Inverse> {
        let Content::Ready(opened) = &self.content else {
            return None;
        };
        Inverse::of(opened.doc.page_transform(page)?)
    }

    /// A press on the pages: starts drawing with the tool in hand, or
    /// chooses the annotation under it. Returns whether it did either, so
    /// the press does not also select text.
    pub(super) fn annot_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((page, point)) = self.page_point(position) else {
            return false;
        };
        match self.tool {
            Some(tool) if tool.markup().is_none() => {
                self.choose(None, cx);
                self.selection = None;
                self.draft = Some(Draft {
                    page,
                    path: vec![point],
                });
                cx.notify();
                true
            }
            // Markup tools select text, and mark it when let go.
            Some(_) => false,
            None => {
                let hit = self.inverse(page).and_then(|inv| {
                    let annots = self.annots.as_ref()?.get(page)?;
                    annots.get(annotate::at(annots, inv.map(point))?)?.id
                });
                self.choose(hit, cx);
                hit.is_some()
            }
        }
    }

    pub(super) fn annot_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(draft) = self.draft.as_ref() else {
            return false;
        };
        let page = draft.page;
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        // Clamped to the page.
        let point = [
            ((position.x - b.left()) / shown).clamp(0.0, (b.size.width / shown).max(0.0)),
            ((position.y - b.top()) / shown).clamp(0.0, (b.size.height / shown).max(0.0)),
        ];
        let ink = self.tool == Some(Tool::Ink);
        if let Some(draft) = self.draft.as_mut() {
            if ink || draft.path.len() < 2 {
                draft.path.push(point);
            } else if let Some(last) = draft.path.last_mut() {
                *last = point;
            }
        }
        cx.notify();
        true
    }

    /// The pointer let go: what was drawn, or the text selected with a
    /// markup tool, becomes an annotation. Returns whether it did.
    pub(super) fn annot_release(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(draft) = self.draft.take() {
            let made = self
                .tool
                .zip(self.inverse(draft.page))
                .and_then(|(tool, inv)| annotate::drawn(tool, draft.page, &draft.path, inv));
            if let Some(mut a) = made {
                a.colour = COLOURS[self.colour];
                if let AnnotKind::FreeText { .. } = a.kind {
                    // Black text unless a colour other than the first
                    // (yellow) was picked.
                    if self.colour == 0 {
                        a.colour = [0.0, 0.0, 0.0];
                    }
                }
                a.author = author();
                // Notes and free text are written in at once.
                self.comment_next = matches!(a.kind, AnnotKind::Note | AnnotKind::FreeText { .. });
                self.change_annotations(
                    AnnotChanges {
                        put: vec![a],
                        remove: Vec::new(),
                    },
                    cx,
                );
            }
            cx.notify();
            return true;
        }
        if let Some(style) = self.tool.and_then(Tool::markup)
            && self.selection.is_some_and(|s| s.anchor != s.head)
        {
            self.mark_selection(style, cx);
            return true;
        }
        false
    }

    /// Marks the text selected in `style`, one annotation per page.
    fn mark_selection(&mut self, style: katna_pdf_cos::MarkupStyle, cx: &mut Context<Self>) {
        let (Some(selection), Some(texts)) = (self.selection.take(), self.texts.clone()) else {
            return;
        };
        let (from, to) = if selection.anchor <= selection.head {
            (selection.anchor.page, selection.head.page)
        } else {
            (selection.head.page, selection.anchor.page)
        };
        let mut put = Vec::new();
        for page in from..=to {
            let (Some(text), Some(inv)) =
                (texts.get(page).and_then(|t| t.get()), self.inverse(page))
            else {
                continue;
            };
            let Some(range) = selection.on_page(page, text.boxes.len()) else {
                continue;
            };
            if let Some(mut a) = annotate::markup(style, page, &text.line_boxes(range), inv) {
                a.colour = COLOURS[self.colour];
                a.author = author();
                put.push(a);
            }
        }
        if !put.is_empty() {
            self.change_annotations(
                AnnotChanges {
                    put,
                    remove: Vec::new(),
                },
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn chosen_annotation(&self) -> Option<&Annotation> {
        let id = self.chosen?;
        self.annots
            .as_ref()?
            .iter()
            .flatten()
            .find(|a| a.id == Some(id))
    }

    /// Chooses annotation `id` (or none), keeping the comment typed for
    /// the one chosen before.
    pub(super) fn choose(&mut self, id: Option<Ref>, cx: &mut Context<Self>) {
        if self.chosen == id {
            return;
        }
        self.finish_comment(true, cx);
        self.chosen = id;
        cx.notify();
    }

    /// Opens the chosen annotation's comment for typing, in the comments
    /// tab.
    fn edit_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_annotation().cloned() else {
            return;
        };
        let Some(id) = a.id else {
            return;
        };
        if self.comment.as_ref().is_some_and(|c| c.id == id) {
            return;
        }
        self.finish_comment(true, cx);
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("comment-placeholder", &[]), cx);
            input.set_accent(accent);
            input.set_text(a.contents.clone(), cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_comment(true, cx),
            InputEvent::Cancel => this.finish_comment(false, cx),
            InputEvent::Changed => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        self.pane = true;
        self.tab = PaneTab::Comments;
        self.comment = Some(super::annots::Comment {
            id,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing a comment, keeping it or not.
    pub(super) fn finish_comment(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(comment) = self.comment.take() else {
            return;
        };
        cx.notify();
        if !keep {
            return;
        }
        let text = comment.input.read(cx).text().to_owned();
        let Some(mut a) = self
            .annots
            .as_ref()
            .and_then(|all| all.iter().flatten().find(|a| a.id == Some(comment.id)))
            .cloned()
        else {
            return;
        };
        if a.contents == text {
            return;
        }
        a.contents = text;
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// Deletes the annotation chosen, and the replies to it.
    pub(super) fn delete_chosen(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.chosen else {
            return;
        };
        self.comment = None;
        let mut remove = vec![id];
        if let Some(all) = &self.annots {
            remove.extend(
                all.iter()
                    .flatten()
                    .filter(|a| a.reply_to == Some(id))
                    .filter_map(|a| a.id),
            );
        }
        self.chosen = None;
        self.change_annotations(
            AnnotChanges {
                put: Vec::new(),
                remove,
            },
            cx,
        );
    }

    /// Writes `changes` into the file shown (as changed so far) on a
    /// background thread and shows the file written. Changes asked for
    /// while one is being written follow it.
    pub(super) fn change_annotations(&mut self, changes: AnnotChanges, cx: &mut Context<Self>) {
        self.finish_typing(true, cx);
        if self.annotating {
            self.annots_waiting.put.extend(changes.put);
            self.annots_waiting.remove.extend(changes.remove);
            return;
        }
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        if self
            .form
            .as_ref()
            .is_some_and(|f| f.dirty() && f.filled.is_none())
        {
            // A form change is still being written; it is lost otherwise.
            self.annots_waiting.put.extend(changes.put);
            self.annots_waiting.remove.extend(changes.remove);
            return;
        }
        let current = self.unsaved();
        let generation = self.generation;
        let mut pages: Vec<usize> = changes.put.iter().map(|a| a.page).collect();
        if let Some(all) = &self.annots {
            for (page, annots) in all.iter().enumerate() {
                if annots
                    .iter()
                    .any(|a| a.id.is_some_and(|id| changes.remove.contains(&id)))
                {
                    pages.push(page);
                }
            }
        }
        let new_on = changes.put.iter().find(|a| a.id.is_none()).map(|a| a.page);
        self.annotating = true;
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let base = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    let out = Arc::new(base.annotate(&changes).map_err(|e| e.to_string())?);
                    let doc = crate::engine::engine()
                        .open(out.clone(), "")
                        .map_err(|e| e.to_string())?;
                    let cos =
                        katna_pdf_cos::Document::load(out.to_vec()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((out, doc, cos))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.annotating = false;
                match written {
                    Ok((bytes, doc, cos)) => this.show_annotated(bytes, doc, cos, &pages, new_on),
                    Err(error) => {
                        this.comment_next = false;
                        this.notice = Some(t("annotate-failed", &[("error", &error)]));
                    }
                }
                // What was asked for meanwhile.
                let waiting = std::mem::take(&mut this.annots_waiting);
                if !waiting.put.is_empty() || !waiting.remove.is_empty() {
                    this.change_annotations(waiting, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows the file with its annotations changed, drawing `pages` again.
    fn show_annotated(
        &mut self,
        bytes: Arc<Vec<u8>>,
        doc: Arc<dyn Document>,
        cos: katna_pdf_cos::Document,
        pages: &[usize],
        new_on: Option<usize>,
    ) {
        let annots = cos.annotations();
        // A new note or text box is chosen, its comment open for typing.
        if let Some(page) = new_on
            && std::mem::take(&mut self.comment_next)
        {
            self.chosen = annots.get(page).and_then(|a| a.last()).and_then(|a| a.id);
            self.open_comment = true;
        }
        self.annots = Some(Arc::new(annots));
        // The form's changes are in the file now.
        let fields = cos.form();
        self.form = crate::form::Form::new(Arc::new(cos), fields);
        self.edited = Some(bytes);
        self.swap_doc(doc, pages);
    }

    /// Opens the comment of the annotation just made, once the window is
    /// at hand.
    pub(super) fn open_comment_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.open_comment) {
            self.edit_comment(window, cx);
        }
    }

    /// Page `page`'s annotation overlays: the outline of the one chosen
    /// and what is being drawn.
    pub(super) fn annot_overlays(
        &self,
        page: usize,
        transform: Option<[f64; 6]>,
    ) -> Vec<AnyElement> {
        let shown = self.shown;
        let mut out = Vec::new();
        if let (Some(a), Some(transform)) = (self.chosen_annotation(), transform)
            && a.page == page
        {
            let b = katna_pdf_render::map_rect(transform, a.rect);
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(b[0] * shown - 3.0))
                    .top(desktop_px(b[1] * shown - 3.0))
                    .w(desktop_px((b[2] - b[0]) * shown + 6.0))
                    .h(desktop_px((b[3] - b[1]) * shown + 6.0))
                    .rounded(px(radius::XS))
                    .border_2()
                    .border_color(gpui::rgba(self.theme.katna.accent))
                    .into_any_element(),
            );
        }
        let Some(draft) = self.draft.as_ref().filter(|d| d.page == page) else {
            return out;
        };
        let [r, g, b] = COLOURS[self.colour];
        let colour = gpui::rgb(u32::from_be_bytes([
            0,
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
        ]));
        match self.tool {
            Some(Tool::Ink | Tool::Line | Tool::Arrow) => {
                let path = draft.path.clone();
                out.push(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let mut builder =
                                gpui::PathBuilder::stroke(desktop_px(annotate::WIDTH * shown));
                            for (i, p) in path.iter().enumerate() {
                                let at = bounds.origin
                                    + gpui::point(
                                        desktop_px(p[0] * shown),
                                        desktop_px(p[1] * shown),
                                    );
                                if i == 0 {
                                    builder.move_to(at);
                                } else {
                                    builder.line_to(at);
                                }
                            }
                            if path.len() > 1
                                && let Ok(path) = builder.build()
                            {
                                window.paint_path(path, colour);
                            }
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .into_any_element(),
                );
            }
            Some(Tool::Rectangle | Tool::Ellipse | Tool::FreeText | Tool::Stamp) => {
                let (Some(from), Some(to)) = (draft.path.first(), draft.path.last()) else {
                    return out;
                };
                let (x0, y0) = (from[0].min(to[0]), from[1].min(to[1]));
                let (x1, y1) = (from[0].max(to[0]), from[1].max(to[1]));
                out.push(
                    div()
                        .absolute()
                        .left(desktop_px(x0 * shown))
                        .top(desktop_px(y0 * shown))
                        .w(desktop_px((x1 - x0) * shown))
                        .h(desktop_px((y1 - y0) * shown))
                        .border_2()
                        .border_color(colour)
                        .when(self.tool == Some(Tool::Ellipse), |d| d.rounded_full())
                        .into_any_element(),
                );
            }
            _ => {}
        }
        out
    }

    /// The buttons above the comments: save them as XFDF, add them from
    /// XFDF.
    pub(super) fn comment_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(space::S1))
            .px(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                widgets::icon_button("comments-import", "upload", 18.0, &th.katna)
                    .tip(t("comments-import", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| this.import_xfdf(cx))),
            )
            .child(
                widgets::icon_button("comments-export", "download", 18.0, &th.katna)
                    .tip(t("comments-export", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| this.export_xfdf(cx))),
            )
            .into_any_element()
    }

    /// Asks where to save the annotations as XFDF, and saves them there.
    fn export_xfdf(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(annots)) = (self.path.clone(), self.annots.clone()) else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&folder, Some(&format!("{stem}.xfdf")));
        let file = super::file_name(&path);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let xml = katna_pdf_cos::to_xfdf(&annots, &file);
                    std::fs::write(&target, xml).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t("comments-exported", &[("name", &super::file_name(&target))]),
                    Err(error) => t("comments-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks for an XFDF file and adds its annotations to the file shown;
    /// ones already here (by name) are replaced.
    fn import_xfdf(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let current = self.unsaved();
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
                    let xml = std::fs::read(&file).map_err(|e| e.to_string())?;
                    let xml = String::from_utf8_lossy(&xml);
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    doc.xfdf_changes(&xml).map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok(changes) if changes.put.is_empty() => {
                        this.notice = Some(t("comments-import-none", &[]));
                    }
                    Ok(changes) => this.change_annotations(changes, cx),
                    Err(error) => this.notice = Some(t("comments-failed", &[("error", &error)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The comments tab: every annotation, page by page, the one chosen
    /// with its comment open for typing.
    pub(super) fn comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(all) = self.annots.clone() else {
            return div().into_any_element();
        };
        let mut list = div()
            .id("comments-list")
            .size_full()
            .overflow_y_scroll()
            .py(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1));
        let mut any = false;
        for (page, annots) in all.iter().enumerate() {
            for a in annots {
                let Some(id) = a.id else {
                    continue;
                };
                any = true;
                let chosen = self.chosen == Some(id);
                let page_label = (page + 1).to_string();
                let what = if a.contents.is_empty() {
                    t(annotate::kind_message(&a.kind), &[])
                } else {
                    a.contents.clone()
                };
                let mut meta = t("comment-page", &[("page", &page_label)]);
                if !a.author.is_empty() {
                    meta = format!("{meta} · {}", a.author);
                }
                let [r, g, b] = a.colour;
                let swatch = gpui::rgb(u32::from_be_bytes([
                    0,
                    (r * 255.0) as u8,
                    (g * 255.0) as u8,
                    (b * 255.0) as u8,
                ]));
                let mut row = div()
                    .id(("comment", id.num as usize))
                    .mx(px(space::S2))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::SM))
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .cursor_pointer()
                    .when(chosen, |d| d.bg(th.field))
                    .hover(move |s| s.bg(th.hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(Some(id), cx);
                        this.edit_comment(window, cx);
                        this.go_to(katna_pdf_cos::Destination { page, top: None }, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(div().size(px(10.0)).flex_none().rounded_full().bg(swatch))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(text::SMALL))
                                    .text_color(th.text)
                                    .child(what),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(meta),
                    );
                if chosen {
                    if let Some(comment) = self.comment.as_ref().filter(|c| c.id == id) {
                        row = row.child(
                            div()
                                .px(px(space::S2))
                                .py(px(space::S1))
                                .rounded(px(radius::XS))
                                .bg(th.card)
                                .border_1()
                                .border_color(th.line)
                                .text_size(px(text::SMALL))
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(comment.input.clone()),
                        );
                    }
                    row = row.child(
                        div().flex().justify_end().child(
                            widgets::icon_button("comment-delete", "trash", 18.0, &th.katna)
                                .tip(t("comment-delete", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| this.delete_chosen(cx))),
                        ),
                    );
                }
                list = list.child(row);
            }
        }
        if !any {
            list = list.child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S4))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("comments-none", &[])),
            );
        }
        let _ = window;
        list.into_any_element()
    }
}

/// Who annotations are by: the name the person logged in with.
fn author() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}
