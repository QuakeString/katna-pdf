// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing the text on a page (docs/PLAN.md §4.3, Phase 4): with Edit
//! text on, each paragraph of a page's text (a line alone, or lines one
//! under another) shows as a box to click; the click opens it for typing
//! in place, Enter starting a new line, and Ctrl+Enter or a click
//! elsewhere writes the change into the file (`katna_pdf_cos::edit`):
//! the words wrapped to the paragraph's width, in its own font where it
//! has the letters.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{PageObject, TextBlock};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_area::TextArea;
use katna_ui::text_input::InputEvent;
use katna_ui::tokens::radius;

use super::{Content, Viewer};
use crate::text::t;

/// The paragraphs, pictures and shapes read so far, by page, and the
/// pages being read.
#[derive(Default)]
pub(super) struct Runs {
    pages: HashMap<usize, Arc<Vec<TextBlock>>>,
    pub(super) objects: HashMap<usize, Arc<Vec<PageObject>>>,
    reading: HashSet<usize>,
    /// Bumped when the file changes, so runs read from the old one are
    /// dropped.
    generation: u64,
}

/// The paragraph being typed in, or new text.
pub(super) struct Typing {
    page: usize,
    target: Target,
    input: Entity<TextArea>,
    _input: Subscription,
}

/// What is being typed: paragraph `n` of the page's, or new text whose
/// box's top left is at a place on the page shown, in points.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Target {
    Block(usize),
    New([f32; 2]),
}

impl Viewer {
    /// Turns Edit text on or off; other tools are put down.
    pub(super) fn toggle_text_edit(&mut self, cx: &mut Context<Self>) {
        self.finish_text_edit(true, cx);
        self.text_editing = !self.text_editing;
        self.adding = None;
        self.placing = None;
        self.picked_object = None;
        self.moving = None;
        if self.text_editing {
            self.tools = false;
            self.tool = None;
            self.draft = None;
            self.cropping = false;
            self.organizer = false;
            self.signing = None;
            self.selection = None;
        }
        cx.notify();
    }

    /// Turns Edit text off, keeping what was typed.
    pub(super) fn leave_text_edit(&mut self, cx: &mut Context<Self>) {
        self.finish_text_edit(true, cx);
        self.text_editing = false;
        self.adding = None;
        self.placing = None;
        self.picked_object = None;
        self.moving = None;
    }

    /// Forgets the runs read, as the file changed.
    pub(super) fn forget_runs(&mut self) {
        self.runs.pages.clear();
        self.runs.objects.clear();
        self.runs.reading.clear();
        self.runs.generation += 1;
    }

    /// Reads page `page`'s paragraphs in the background, from the file as
    /// changed so far.
    fn read_runs(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.runs.pages.contains_key(&page) || !self.runs.reading.insert(page) {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let generation = self.runs.generation;
        cx.spawn(async move |this, cx| {
            let runs = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let doc = katna_pdf_cos::Document::load(bytes).ok()?;
                    Some((doc.text_blocks(page), doc.page_objects(page)))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.runs.generation != generation {
                    return;
                }
                this.runs.reading.remove(&page);
                let (blocks, objects) = runs.unwrap_or_default();
                this.runs.pages.insert(page, Arc::new(blocks));
                this.runs.objects.insert(page, Arc::new(objects));
                if this.picked_object_on(page) {
                    this.refresh_inspector(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens paragraph `block` of page `page` for typing.
    fn start_text_edit(
        &mut self,
        page: usize,
        block: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .typing
            .as_ref()
            .is_some_and(|t| t.page == page && t.target == Target::Block(block))
        {
            return;
        }
        self.finish_text_edit(true, cx);
        let Some(text) = self
            .runs
            .pages
            .get(&page)
            .and_then(|blocks| blocks.get(block))
            .map(|b| b.text.clone())
        else {
            return;
        };
        self.start_typing(page, Target::Block(block), text, window, cx);
    }

    /// Opens a box for typing `text` into `target` on page `page`.
    pub(super) fn start_typing(
        &mut self,
        page: usize,
        target: Target,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextArea::new("", cx);
            input.set_accent(accent);
            let end = text.len();
            input.set_text(text, end, cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_text_edit(true, cx),
            InputEvent::Cancel => this.finish_text_edit(false, cx),
            InputEvent::Changed => {}
        });
        // Once the press that opened it is done with: the viewer takes
        // the keys on a press.
        let handle = input.focus_handle(cx);
        window.focus(&handle, cx);
        cx.defer_in(window, move |_, window, cx| window.focus(&handle, cx));
        self.typing = Some(Typing {
            page,
            target,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing in a paragraph, writing the change into the file or
    /// not.
    pub(super) fn finish_text_edit(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        cx.notify();
        let text = typing.input.read(cx).text().to_owned();
        if !keep {
            return;
        }
        let block = match typing.target {
            Target::New(at) => {
                self.write_new_text(typing.page, at, text, cx);
                return;
            }
            Target::Block(block) => block,
        };
        let Some(block) = self
            .runs
            .pages
            .get(&typing.page)
            .and_then(|blocks| blocks.get(block))
            .filter(|b| b.text != text)
            .cloned()
        else {
            return;
        };
        let runs = block.runs;
        self.change_page(
            typing.page,
            move |doc| {
                doc.edit_block(typing.page, &runs, &text)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Changes page `page` with `change` on a background thread, from the
    /// file as changed so far, and shows the file it gives.
    pub(super) fn change_page(
        &mut self,
        page: usize,
        change: impl FnOnce(&katna_pdf_cos::Document) -> Result<Vec<u8>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.change_document(vec![page], "text-edit-failed", change, cx);
    }

    /// Changes the file with `change`, then shows it, `pages` drawn again;
    /// message `failed` says why when it could not.
    pub(super) fn change_document(
        &mut self,
        pages: Vec<usize>,
        failed: &'static str,
        change: impl FnOnce(&katna_pdf_cos::Document) -> Result<Vec<u8>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        self.finish_typing(true, cx);
        let current = self.unsaved();
        let generation = self.generation;
        self.forget_runs();
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let base = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    let out = Arc::new(change(&base)?);
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
                match written {
                    Ok((bytes, doc, cos)) => {
                        this.forget_runs();
                        this.show_annotated(bytes, doc, cos, &pages, None);
                    }
                    Err(error) => {
                        this.notice = Some(t(failed, &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Page `page`'s paragraphs to click while Edit text is on, and the
    /// one being typed in.
    pub(super) fn text_edit_overlay(
        &mut self,
        page: usize,
        transform: Option<[f64; 6]>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if !self.text_editing {
            return Vec::new();
        }
        let Some(transform) = transform else {
            return Vec::new();
        };
        let Some(blocks) = self.runs.pages.get(&page).cloned() else {
            self.read_runs(page, cx);
            return Vec::new();
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent & 0xffff_ff00;
        let ink = {
            let [r, g, b] = self.page_colors().ink();
            gpui::rgb(u32::from_be_bytes([0, r, g, b]))
        };
        // With a tool for adding in hand, the paragraphs are not boxes to
        // click.
        let listed = if self.adding.is_some() {
            &[][..]
        } else {
            &blocks[..]
        };
        let mut out: Vec<AnyElement> = listed
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let b = katna_pdf_render::map_rect(transform, block.rect);
                let area = div()
                    .id(("text-run", page * 100_000 + i))
                    .absolute()
                    .left(desktop_px(b[0] * shown - 2.0))
                    .top(desktop_px(b[1] * shown - 1.0))
                    .rounded(px(radius::XS / 2.0));
                let height = (b[3] - b[1]) * shown + 2.0;
                let typing = self
                    .typing
                    .as_ref()
                    .filter(|t| t.page == page && t.target == Target::Block(i));
                if let Some(typing) = typing {
                    // As wide as the paragraph, with room for a few more
                    // letters on a short one; as tall as its text wraps to.
                    let size = block.size;
                    let width = (b[2] - b[0]).max(size * 8.0) + size;
                    return area
                        .w(desktop_px(width * shown + 4.0))
                        .min_h(desktop_px(height))
                        .px(desktop_px(1.0))
                        .bg(self.paper())
                        .border_1()
                        .border_color(gpui::rgb(self.theme.katna.accent >> 8))
                        .text_size(desktop_px(size * shown))
                        .line_height(desktop_px(size * 1.2 * shown))
                        .text_color(ink)
                        .cursor(gpui::CursorStyle::IBeam)
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(typing.input.clone())
                        .into_any_element();
                }
                area.w(desktop_px((b[2] - b[0]) * shown + 4.0))
                    .h(desktop_px(height))
                    .border_1()
                    .border_color(gpui::rgba(accent | 0x40))
                    .hover(move |s| {
                        s.bg(gpui::rgba(accent | 0x22))
                            .border_color(gpui::rgba(accent | 0xcc))
                    })
                    .cursor(gpui::CursorStyle::IBeam)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.start_text_edit(page, i, window, cx)
                    }))
                    .into_any_element()
            })
            .collect();
        // New text being typed.
        if let Some(typing) = self.typing.as_ref().filter(|t| t.page == page)
            && let Target::New([x, y]) = typing.target
        {
            let size = super::insert::TEXT_SIZE;
            out.push(
                div()
                    .id(("new-text", page))
                    .absolute()
                    .left(desktop_px(x * shown - 2.0))
                    .top(desktop_px(y * shown - 1.0))
                    .w(desktop_px(size * 20.0 * shown))
                    .min_h(desktop_px(size * 1.2 * shown + 2.0))
                    .px(desktop_px(1.0))
                    .rounded(px(radius::XS / 2.0))
                    .border_1()
                    .border_color(gpui::rgb(self.theme.katna.accent >> 8))
                    .text_size(desktop_px(size * shown))
                    .line_height(desktop_px(size * 1.2 * shown))
                    .text_color(self.ink_colour())
                    .cursor(gpui::CursorStyle::IBeam)
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(typing.input.clone())
                    .into_any_element(),
            );
        }
        out.extend(self.objects_overlay(page));
        out.extend(self.insert_preview(page));
        out
    }

    /// A press on the pages while Edit text is on, outside the
    /// paragraphs:
    /// ends the typing. The pointer selects nothing meanwhile.
    pub(super) fn text_edit_press(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.text_editing {
            return false;
        }
        self.finish_text_edit(true, cx);
        true
    }
}
