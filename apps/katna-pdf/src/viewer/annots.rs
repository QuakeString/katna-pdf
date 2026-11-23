// SPDX-License-Identifier: GPL-3.0-or-later

//! The viewer's annotating (docs/PLAN.md §4.2): the row of tools, the
//! pointer making annotations with them, the comments tab, and writing
//! each change into the file (`katna_pdf_cos::annots`) to be drawn from
//! there.

use std::path::Path;
use std::sync::Arc;

use gpui::{AnyElement, Context, Focusable, SharedString, Window, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, Ref};
use katna_pdf_render::Document;
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};

use super::{Content, PaneTab, Viewer};
use crate::annotate::{self, COLOURS, Inverse, Tool};
use crate::float::{self, FloatTip};
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
        if !self.tools && !self.allows(super::Allow::Comment, cx) {
            return;
        }
        self.tools = !self.tools;
        if self.tools {
            self.signing = None;
            self.leave_text_edit(cx);
        }
        if !self.tools {
            self.tool = None;
            self.draft = None;
        }
        cx.notify();
    }

    pub(super) fn pick_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = (self.tool != Some(tool)).then_some(tool);
        self.colour_chosen = false;
        self.cropping = false;
        self.draft = None;
        self.measuring = None;
        if tool.clicks_points() {
            self.start_measuring(cx);
        }
        // Highlighting tells the text from the space around it, and a
        // caret goes between its letters.
        if matches!(self.tool, Some(Tool::Highlight | Tool::Insert)) {
            let page = self.current_page();
            for p in page.saturating_sub(1)..(page + 2).min(self.page_count()) {
                self.read_text(p, cx);
            }
        }
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
        // A measurement being drawn goes first; the tool stays in hand.
        if self.measuring.take().is_some() {
            cx.notify();
            return;
        }
        let put_down = self.draft.take().is_some() | self.tool.take().is_some();
        if !put_down {
            self.choose(None, cx);
        }
        cx.notify();
    }

    /// The row of tools and colours over the dock, while shown, in
    /// `room` pixels: tools that do not fit go in a More menu at its end.
    pub(super) fn tools_row(&self, room: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.tools || !matches!(self.content, Content::Ready(_)) {
            return None;
        }
        let th = self.theme;
        let fit = tools_that_fit(room, self.tool.map(|t| t.name().len() * 7));
        let mut row = float::tray("annotation-tools", &th);
        for (i, tool) in Tool::ALL.into_iter().enumerate().take(fit) {
            let on = self.tool == Some(tool);
            if starts_group(tool) {
                row = row.child(float::vsep(&th));
            }
            row = row.child(
                float::dk(
                    ("tool", i),
                    tool.icon(),
                    on.then(|| SharedString::from(t(tool.name(), &[]))),
                    t(tool.tip(), &[]),
                    if on {
                        float::Pressed::Tool
                    } else {
                        float::Pressed::No
                    },
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.pick_tool(tool, cx))),
            );
        }
        if fit < Tool::ALL.len() {
            // The tool in hand, when it is one of those folded away,
            // shows on the More button.
            let hidden = self.tool.filter(|tool| {
                Tool::ALL
                    .iter()
                    .position(|t| t == tool)
                    .is_some_and(|i| i >= fit)
            });
            row = row.child(float::vsep(&th)).child(
                div()
                    .relative()
                    .child(
                        float::dk(
                            "tool-more",
                            hidden.map_or("more", |tool| tool.icon()),
                            hidden.map(|tool| SharedString::from(t(tool.name(), &[]))),
                            t("tool-more", &[]),
                            if hidden.is_some() || self.tools_more {
                                float::Pressed::Tool
                            } else {
                                float::Pressed::No
                            },
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tools_more = !this.tools_more;
                            cx.notify();
                        })),
                    )
                    .when(self.tools_more, |d| {
                        let mut menu = float::menu("tool-more-menu", 220.0, &th)
                            .max_h(px(480.0))
                            .overflow_y_scroll()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.tools_more = false;
                                cx.notify();
                            }));
                        for (i, tool) in Tool::ALL.into_iter().enumerate().skip(fit) {
                            if i > fit && starts_group(tool) {
                                menu = menu.child(float::menu_sep(&th));
                            }
                            menu = menu.child(
                                float::menu_item(
                                    ("tool-more-item", i),
                                    Some(tool.icon()),
                                    t(tool.name(), &[]),
                                    None,
                                    &th,
                                )
                                .when(self.tool == Some(tool), |d| d.bg(th.accent_soft))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.tools_more = false;
                                        this.pick_tool(tool, cx);
                                    },
                                )),
                            );
                        }
                        d.child(float::popover(menu, true, true))
                    }),
            );
        }
        row = row.child(float::vsep(&th));
        // The look kept for the tool in hand shows its colour as picked,
        // and text put in or replaced shows blue for yellow.
        let kept = self.kept_look().map(|l| l.colour).or_else(|| {
            matches!(self.tool, Some(Tool::Insert | Tool::Replace)).then(|| self.edit_colour())
        });
        for (i, [r, g, b]) in COLOURS.into_iter().enumerate() {
            let on = match kept {
                Some(c) => c.iter().zip([r, g, b]).all(|(x, y)| (x - y).abs() < 0.005),
                None => self.colour == i,
            };
            let fill: gpui::Hsla = gpui::rgb(u32::from_be_bytes([
                0,
                (r * 255.0) as u8,
                (g * 255.0) as u8,
                (b * 255.0) as u8,
            ]))
            .into();
            row = row.child(
                float::swatch(("colour", i), fill, on, &th)
                    .ftip(t("tool-colour", &[]), &th)
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_colour(i, cx))),
            );
        }
        Some(row.into_any_element())
    }

    /// Picks colour `i` for what is drawn next; with no tool in hand, it
    /// is given to the annotation chosen.
    fn pick_colour(&mut self, i: usize, cx: &mut Context<Self>) {
        self.colour = i;
        self.colour_chosen = true;
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

    /// Whether `point` (points from page `page`'s top left) is on or
    /// next to its text; taken as yes while the text is still being read
    /// (and the reading started).
    fn on_text(&mut self, page: usize, point: [f32; 2], cx: &mut Context<Self>) -> bool {
        let Some(texts) = self.texts.clone() else {
            return true;
        };
        let Some(text) = texts.get(page).and_then(|t| t.get()) else {
            self.read_text(page, cx);
            return true;
        };
        const NEAR: f32 = 3.0;
        text.boxes.iter().any(|b| {
            b[2] > b[0]
                && point[0] >= b[0] - NEAR
                && point[0] <= b[2] + NEAR
                && point[1] >= b[1] - NEAR
                && point[1] <= b[3] + NEAR
        })
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
        let area = self.tool == Some(Tool::Highlight) && !self.on_text(page, point, cx);
        match self.tool {
            // Highlighting away from the text marks the area dragged
            // over, as on a scanned page.
            Some(tool) if tool.markup().is_none() || area => {
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

    /// The pointer moved, no button down: the comment under it, if it
    /// has words, has them shown beside it.
    pub(super) fn annot_hover_move(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let hit = if self.tool.is_some() {
            None
        } else {
            self.page_point(position).and_then(|(page, point)| {
                let inv = self.inverse(page)?;
                let annots = self.annots.as_ref()?.get(page)?;
                let a = annots.get(annotate::at(annots, inv.map(point))?)?;
                (!a.contents.trim().is_empty() && !a.flags.hidden)
                    .then_some(a.id)
                    .flatten()
            })
        };
        if hit != self.annot_hover {
            self.annot_hover = hit;
            cx.notify();
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
        let ink = matches!(self.tool, Some(Tool::Ink | Tool::Eraser));
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

    /// Types today's date where the page was clicked, as text with no
    /// box, black unless another colour was picked.
    fn type_today(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let (Some(inv), Some(&at)) = (self.inverse(draft.page), draft.path.first()) else {
            return;
        };
        let words = super::stamps::today();
        let size = annotate::TEXT_SIZE;
        // About as wide as the words in Helvetica, the line's height tall,
        // its middle where the click was.
        let width = words.chars().count() as f32 * size * 0.56 + 6.0;
        let height = size * 1.5;
        let mut a = Annotation::new(draft.page, AnnotKind::FreeText { size });
        a.rect = inv.rect([
            at[0],
            at[1] - height / 2.0,
            at[0] + width,
            at[1] + height / 2.0,
        ]);
        a.width = 0.0;
        a.contents = words;
        a.colour = if self.colour == 0 {
            [0.0, 0.0, 0.0]
        } else {
            COLOURS[self.colour]
        };
        a.author = author();
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// Rubs out the parts of the pen strokes on the draft's page that the
    /// eraser passed over; locked ones are left.
    fn erase_ink(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let (Some(inv), Some(all)) = (self.inverse(draft.page), self.annots.clone()) else {
            return;
        };
        let path: Vec<[f32; 2]> = draft.path.iter().map(|p| inv.map(*p)).collect();
        // About eight pixels either side, however far in it is zoomed.
        let reach = (8.0 / self.shown).max(1.0);
        let mut changes = AnnotChanges {
            put: Vec::new(),
            remove: Vec::new(),
        };
        for a in all.get(draft.page).into_iter().flatten() {
            let (Some(id), AnnotKind::Ink(strokes)) = (a.id, &a.kind) else {
                continue;
            };
            if a.flags.locked {
                continue;
            }
            match annotate::erase(strokes, &path, reach + a.width / 2.0) {
                None => {}
                Some(left) if left.is_empty() => changes.remove.push(id),
                Some(left) => {
                    let mut a = a.clone();
                    a.kind = AnnotKind::Ink(left);
                    a.rect = a.bounds();
                    changes.put.push(a);
                }
            }
        }
        if !changes.put.is_empty() || !changes.remove.is_empty() {
            self.change_annotations(changes, cx);
        }
    }

    /// The pointer let go: what was drawn, or the text selected with a
    /// markup tool, becomes an annotation. Returns whether it did.
    pub(super) fn annot_release(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tool == Some(Tool::Insert)
            && let Some(draft) = self.draft.take()
        {
            self.insert_caret(draft, cx);
            cx.notify();
            return true;
        }
        if self.tool == Some(Tool::Eraser)
            && let Some(draft) = self.draft.take()
        {
            self.erase_ink(draft, cx);
            cx.notify();
            return true;
        }
        if self.tool == Some(Tool::Stamp)
            && self.stamp == super::stamps::TODAY
            && let Some(draft) = self.draft.take()
        {
            self.type_today(draft, cx);
            cx.notify();
            return true;
        }
        if let Some(draft) = self.draft.take() {
            let made = self
                .tool
                .zip(self.inverse(draft.page))
                .and_then(|(tool, inv)| annotate::drawn(tool, draft.page, &draft.path, inv));
            if let Some(mut a) = made {
                a.colour = COLOURS[self.colour];
                if let AnnotKind::Stamp(name) = &mut a.kind {
                    name.clone_from(&self.stamp);
                    if katna_pdf_cos::stamp_is_mark(name) {
                        // A quick mark is small, black unless another
                        // colour was picked, centred where it was put.
                        let [x0, y0, x1, y1] = a.rect;
                        let (cx_, cy_) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                        let (hw, hh) = if name == "#MLine" {
                            (14.0, 6.0)
                        } else {
                            (7.0, 7.0)
                        };
                        a.rect = [cx_ - hw, cy_ - hh, cx_ + hw, cy_ + hh];
                        if self.colour == 0 {
                            a.colour = [0.0, 0.0, 0.0];
                        }
                    } else if self.colour == 0 {
                        // Stamps read red unless another colour was picked.
                        a.colour = [0.8, 0.1, 0.1];
                    }
                }
                a.author = author();
                if let AnnotKind::FileAttachment { .. } = a.kind {
                    self.attach_file(a, cx);
                    cx.notify();
                    return true;
                }
                if let AnnotKind::FreeText { .. } = a.kind {
                    // Black text unless a colour other than the first
                    // (yellow) was picked.
                    if self.colour == 0 {
                        a.colour = [0.0, 0.0, 0.0];
                    }
                }
                self.with_kept_look(&mut a);
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

    /// Puts a caret where `draft` was let go: between the letters
    /// nearest on the line there, or just there off the text. Its words
    /// are typed next.
    fn insert_caret(&mut self, draft: Draft, cx: &mut Context<Self>) {
        let Some(&point) = draft.path.last() else {
            return;
        };
        let Some(inv) = self.inverse(draft.page) else {
            return;
        };
        let text = self
            .texts
            .as_ref()
            .and_then(|t| t.get(draft.page))
            .and_then(|t| t.get());
        // The letter boxes on the line under the pointer; the edge
        // nearest it.
        let near = text.and_then(|text| {
            text.boxes
                .iter()
                .zip(text.text.chars())
                .filter(|(b, c)| {
                    !c.is_whitespace()
                        && b[2] > b[0]
                        && point[1] >= b[1] - 2.0
                        && point[1] <= b[3] + 2.0
                })
                .map(|(b, _)| b)
                .flat_map(|b| [(b[0], b), (b[2], b)])
                .min_by(|x, y| (x.0 - point[0]).abs().total_cmp(&(y.0 - point[0]).abs()))
                .filter(|(x, _)| (x - point[0]).abs() < 24.0)
                .map(|(x, b)| (x, b[3], b[3] - b[1]))
        });
        let (x, bottom, height) = near.unwrap_or((point[0], point[1] + 6.0, 12.0));
        let mut a = annotate::caret(draft.page, x, bottom, height, inv);
        a.colour = self.edit_colour();
        a.author = author();
        self.with_kept_look(&mut a);
        self.comment_next = true;
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// The colour of text to put in or replace: blue unless another
    /// colour, or a look kept for the tool, was picked; yellow would not
    /// show on paper.
    fn edit_colour(&self) -> [f32; 3] {
        if self.kept_look().is_none() && self.colour == 0 {
            COLOURS[2]
        } else {
            self.drawing_colour()
        }
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
        let replace = self.tool == Some(Tool::Replace);
        let mut put = Vec::new();
        let mut last_line = None;
        for page in from..=to {
            let (Some(text), Some(inv)) =
                (texts.get(page).and_then(|t| t.get()), self.inverse(page))
            else {
                continue;
            };
            let Some(range) = selection.on_page(page, text.boxes.len()) else {
                continue;
            };
            let lines = text.line_boxes(range);
            if let Some(mut a) = annotate::markup(style, page, &lines, inv) {
                a.colour = COLOURS[self.colour];
                a.author = author();
                if replace {
                    // Struck out as part of the caret that follows.
                    a.grouped = true;
                    a.colour = self.edit_colour();
                    a.answers = Some(String::new());
                } else {
                    self.with_kept_look(&mut a);
                }
                put.push(a);
                last_line = lines.last().map(|&l| (page, l, inv));
            }
        }
        // A replacement: a caret after the last word struck out, the
        // words to put in typed next.
        if replace && let Some((page, [_, top, right, bottom], inv)) = last_line {
            let mut caret = annotate::caret(page, right, bottom, bottom - top, inv);
            caret.colour = self.edit_colour();
            caret.author = author();
            self.with_kept_look(&mut caret);
            caret.name = format!("katna-caret-{}", unique_suffix());
            for strike in &mut put {
                strike.answers = Some(caret.name.clone());
                strike.colour = caret.colour;
                strike.opacity = caret.opacity;
            }
            put.push(caret);
            self.comment_next = true;
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
    pub(super) fn edit_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_annotation().cloned() else {
            return;
        };
        let Some(id) = a.id else {
            return;
        };
        if a.flags.locked || self.comment.as_ref().is_some_and(|c| c.id == id) {
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
        if self.chosen_annotation().is_some_and(|a| a.flags.locked) {
            return;
        }
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
        let key = self.key.clone();
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
        let new_on = changes
            .put
            .iter()
            .rev()
            .find(|a| a.id.is_none())
            .map(|a| a.page);
        let what = if self.tool == Some(Tool::Eraser) {
            "undo-what-erase"
        } else if new_on.is_some() {
            "undo-what-add-comment"
        } else if changes.put.is_empty() {
            "undo-what-delete-comment"
        } else {
            "undo-what-change-comment"
        };
        self.annotating = true;
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let base = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let out = Arc::new(base.annotate(&changes).map_err(|e| e.to_string())?);
                    let doc = crate::engine::engine()
                        .open(out.clone(), key.as_deref().unwrap_or(""))
                        .map_err(|e| e.to_string())?;
                    let cos =
                        super::cos(out.to_vec(), key.as_deref()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((out, doc, cos))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.annotating = false;
                match written {
                    Ok((bytes, doc, cos)) => {
                        this.doing = what;
                        this.show_annotated(bytes, doc, cos, &pages, new_on)
                    }
                    Err(error) => {
                        this.comment_next = false;
                        this.notice = Some(super::said("annotate-failed", &[("error", &error)]));
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
    pub(super) fn show_annotated(
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
        let links = cos.links();
        self.links = links.iter().any(|l| !l.is_empty()).then(|| Arc::new(links));
        self.set_label_ranges(cos.page_label_ranges());
        self.extras.set(cos.attachments(), cos.layers());
        // The form's changes are in the file now.
        let fields = cos.form();
        self.form = self.new_form(Arc::new(cos), fields);
        self.record_change();
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
    /// The words of the comment under the pointer, in a card under it,
    /// over everything else on page `page`.
    pub(super) fn annot_hover_card(
        &self,
        page: usize,
        transform: Option<[f64; 6]>,
    ) -> Option<AnyElement> {
        let shown = self.shown;
        if let (Some(id), Some(transform)) = (self.annot_hover, transform)
            && self.chosen != Some(id)
            && let Some(a) = self
                .annots
                .as_ref()
                .and_then(|all| all.get(page))
                .and_then(|on| on.iter().find(|a| a.id == Some(id)))
        {
            let th = self.theme;
            let b = katna_pdf_render::map_rect(transform, a.rect);
            let words: String = if a.contents.chars().count() > 400 {
                a.contents.chars().take(398).chain("…".chars()).collect()
            } else {
                a.contents.clone()
            };
            return Some(
                div()
                    .absolute()
                    .left(desktop_px(b[0] * shown))
                    .top(desktop_px(b[3] * shown + 6.0))
                    .w(px(280.0))
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .p(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.card)
                    .border_1()
                    .border_color(th.line)
                    .shadow(th.lifted())
                    .when(!a.author.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(text::CAPTION))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(th.dim)
                                .child(a.author.clone()),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(words),
                    )
                    .into_any_element(),
            );
        }
        None
    }

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
        let [r, g, b] = self.drawing_colour();
        let colour = gpui::rgb(u32::from_be_bytes([
            0,
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
        ]));
        match self.tool {
            Some(Tool::Eraser) => {
                let path = draft.path.clone();
                let grey = gpui::hsla(0.0, 0.0, 0.5, 0.35);
                out.push(
                    gpui::canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let mut builder = gpui::PathBuilder::stroke(desktop_px(16.0));
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
                                window.paint_path(path, grey);
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
            Some(Tool::Ink | Tool::Line | Tool::Arrow | Tool::Callout) => {
                // A callout shows its line from the first point to the
                // last; the box comes when the pointer lets go.
                let path = if self.tool == Some(Tool::Callout) {
                    draft
                        .path
                        .first()
                        .into_iter()
                        .chain(draft.path.last())
                        .copied()
                        .collect()
                } else {
                    draft.path.clone()
                };
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
            Some(Tool::Highlight) => {
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
                        .bg(colour)
                        .opacity(0.35)
                        .into_any_element(),
                );
            }
            Some(
                Tool::Rectangle | Tool::Ellipse | Tool::FreeText | Tool::Stamp | Tool::Typewriter,
            ) => {
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

    /// Asks where to save the annotations as XFDF, and saves them there.
    pub(super) fn export_xfdf(&mut self, cx: &mut Context<Self>) {
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
                    // FDF when that is the name given, else XFDF.
                    let fdf = target
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("fdf"));
                    let bytes = if fdf {
                        katna_pdf_cos::to_fdf(&annots, &file)
                    } else {
                        katna_pdf_cos::to_xfdf(&annots, &file).into_bytes()
                    };
                    std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => {
                        super::said("comments-exported", &[("name", &super::file_name(&target))])
                    }
                    Err(error) => super::said("comments-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks for an XFDF file and adds its annotations to the file shown;
    /// ones already here (by name) are replaced.
    pub(super) fn import_xfdf(&mut self, cx: &mut Context<Self>) {
        let key = self.key.clone();
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
                    let data = std::fs::read(&file).map_err(|e| e.to_string())?;
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    if data.starts_with(b"%FDF") {
                        doc.fdf_changes(&data).map_err(|e| e.to_string())
                    } else {
                        doc.xfdf_changes(&String::from_utf8_lossy(&data))
                            .map_err(|e| e.to_string())
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok(changes) if changes.put.is_empty() => {
                        this.notice = Some(super::said("comments-import-none", &[]));
                    }
                    Ok(changes) => this.change_annotations(changes, cx),
                    Err(error) => {
                        this.notice = Some(super::said("comments-failed", &[("error", &error)]))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Who annotations are by: the name the person logged in with.
/// A suffix that makes a name not given before.
fn unique_suffix() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{now:x}")
}

pub(super) fn author() -> String {
    if let Some(name) = crate::profile::Profile::load().author() {
        return name;
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default()
}

/// Whether `tool` starts a group of the row, after a line.
fn starts_group(tool: Tool) -> bool {
    matches!(tool, Tool::Note | Tool::Ink | Tool::Stamp | Tool::Distance)
}

/// How many of [`Tool::ALL`] fit in a row `room` pixels wide beside the
/// colours, with the tool in hand `label` pixels wider for its name; all
/// of them when they fit, else as many as leave room for More.
fn tools_that_fit(room: f32, label: Option<usize>) -> usize {
    const TOOL: f32 = 40.0 + 4.0;
    const LINE: f32 = 1.0 + 2.0 * 8.0 + 4.0;
    // The tray's padding and border, the line before the colours, and
    // the colours.
    let fixed = 12.0 + LINE + COLOURS.len() as f32 * (26.0 + 4.0);
    // A label shows on whichever button holds the tool in hand.
    let label = label.map_or(0.0, |w| w as f32 + 6.0);
    let width = |n: usize| {
        Tool::ALL
            .iter()
            .take(n)
            .map(|&t| TOOL + if starts_group(t) { LINE } else { 0.0 })
            .sum::<f32>()
    };
    let all = Tool::ALL.len();
    if fixed + label + width(all) <= room {
        return all;
    }
    let more = TOOL + LINE;
    (0..all)
        .rev()
        .find(|&n| fixed + label + more + width(n) <= room)
        .unwrap_or(0)
}

#[cfg(test)]
mod fit_tests {
    use super::*;

    #[test]
    fn tools_fold_into_more_when_the_window_is_narrow() {
        assert_eq!(tools_that_fit(4000.0, None), Tool::ALL.len());
        let some = tools_that_fit(900.0, None);
        assert!(some > 0 && some < Tool::ALL.len());
        assert!(tools_that_fit(900.0, Some(80)) <= some);
        assert_eq!(tools_that_fit(100.0, None), 0);
    }
}
