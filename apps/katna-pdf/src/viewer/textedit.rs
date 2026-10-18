// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing the text on a page (docs/PLAN.md §4.3, Phase 4): with Edit
//! text on, each line of a page's text shows as a box to click; the
//! click opens it for typing in place, and Enter writes the change into
//! the file (`katna_pdf_cos::edit`), in the line's own font where it has
//! the letters.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::TextRun;
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::radius;

use super::{Content, Viewer};
use crate::text::t;

/// The text runs read so far, by page, and the pages being read.
#[derive(Default)]
pub(super) struct Runs {
    pages: HashMap<usize, Arc<Vec<TextRun>>>,
    reading: HashSet<usize>,
    /// Bumped when the file changes, so runs read from the old one are
    /// dropped.
    generation: u64,
}

/// The line being typed in.
pub(super) struct Typing {
    page: usize,
    run: usize,
    input: Entity<TextInput>,
    _input: Subscription,
}

impl Viewer {
    /// Turns Edit text on or off; other tools are put down.
    pub(super) fn toggle_text_edit(&mut self, cx: &mut Context<Self>) {
        self.finish_text_edit(true, cx);
        self.text_editing = !self.text_editing;
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
    }

    /// Forgets the runs read, as the file changed.
    pub(super) fn forget_runs(&mut self) {
        self.runs.pages.clear();
        self.runs.reading.clear();
        self.runs.generation += 1;
    }

    /// Reads page `page`'s runs in the background, from the file as
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
                    Some(doc.text_runs(page))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.runs.generation != generation {
                    return;
                }
                this.runs.reading.remove(&page);
                this.runs
                    .pages
                    .insert(page, Arc::new(runs.unwrap_or_default()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens run `run` of page `page` for typing.
    fn start_text_edit(
        &mut self,
        page: usize,
        run: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .typing
            .as_ref()
            .is_some_and(|t| t.page == page && t.run == run)
        {
            return;
        }
        self.finish_text_edit(true, cx);
        let Some(text) = self
            .runs
            .pages
            .get(&page)
            .and_then(|runs| runs.get(run))
            .map(|r| r.text.clone())
        else {
            return;
        };
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_accent(accent);
            input.set_text(text, cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_text_edit(true, cx),
            InputEvent::Cancel => this.finish_text_edit(false, cx),
            InputEvent::Changed => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        self.typing = Some(Typing {
            page,
            run,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing in a line, writing the change into the file or not.
    pub(super) fn finish_text_edit(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(typing) = self.typing.take() else {
            return;
        };
        cx.notify();
        let text = typing.input.read(cx).text().to_owned();
        let unchanged = self
            .runs
            .pages
            .get(&typing.page)
            .and_then(|runs| runs.get(typing.run))
            .is_none_or(|r| r.text == text);
        if !keep || unchanged {
            return;
        }
        self.write_text_edit(typing.page, typing.run, text, cx);
    }

    /// Writes the new text of run `run` on page `page` into the file on a
    /// background thread, and shows the file written.
    fn write_text_edit(&mut self, page: usize, run: usize, text: String, cx: &mut Context<Self>) {
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
                    let out = Arc::new(
                        base.edit_text(page, &[(run, text)])
                            .map_err(|e| e.to_string())?,
                    );
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
                        this.show_annotated(bytes, doc, cos, &[page], None);
                    }
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

    /// Page `page`'s lines to click while Edit text is on, and the one
    /// being typed in.
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
        let Some(runs) = self.runs.pages.get(&page).cloned() else {
            self.read_runs(page, cx);
            return Vec::new();
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent & 0xffff_ff00;
        let ink = {
            let [r, g, b] = self.page_colors().ink();
            gpui::rgb(u32::from_be_bytes([0, r, g, b]))
        };
        runs.iter()
            .enumerate()
            .map(|(i, run)| {
                let b = katna_pdf_render::map_rect(transform, run.rect);
                let area = div()
                    .id(("text-run", i))
                    .absolute()
                    .left(desktop_px(b[0] * shown - 2.0))
                    .top(desktop_px(b[1] * shown - 1.0))
                    .h(desktop_px((b[3] - b[1]) * shown + 2.0))
                    .rounded(px(radius::XS / 2.0));
                let typing = self
                    .typing
                    .as_ref()
                    .filter(|t| t.page == page && t.run == i);
                if let Some(typing) = typing {
                    // Wide enough to type more than was there.
                    let width = (b[2] - b[0]).max(run.size * 6.0) + run.size * 4.0;
                    return area
                        .w(desktop_px(width * shown + 4.0))
                        .flex()
                        .items_center()
                        .bg(self.paper())
                        .border_1()
                        .border_color(gpui::rgb(self.theme.katna.accent >> 8))
                        .text_size(desktop_px(run.size * shown))
                        .text_color(ink)
                        .cursor(gpui::CursorStyle::IBeam)
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(typing.input.clone())
                        .into_any_element();
                }
                area.w(desktop_px((b[2] - b[0]) * shown + 4.0))
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
            .collect()
    }

    /// A press on the pages while Edit text is on, outside the lines:
    /// ends the typing. The pointer selects nothing meanwhile.
    pub(super) fn text_edit_press(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.text_editing {
            return false;
        }
        self.finish_text_edit(true, cx);
        true
    }
}
