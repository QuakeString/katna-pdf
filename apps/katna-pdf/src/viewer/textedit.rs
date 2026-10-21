// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing the text on a page (docs/PLAN.md §4.3, Phase 4): with Edit
//! text on, each paragraph of a page's text (a line alone, or lines one
//! under another) is outlined under the pointer; a click opens it for
//! typing in place, in a face like its own at its size, colour and line
//! spacing, Enter starting a new line, and Ctrl+Enter or a click
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
    /// A paragraph clicked while the one before it was being written
    /// into the file: its page and a point in it, in the page's space,
    /// to open once the page's paragraphs are read again.
    next: Option<(usize, [f32; 2])>,
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
    /// The installed face nearest the paragraph's own font.
    look: Look,
}

/// A face to type in: a family installed here, and its weight and slant.
#[derive(Clone, Default)]
pub(super) struct Look {
    family: Option<gpui::SharedString>,
    bold: bool,
    italic: bool,
}

impl Look {
    /// The face nearest PDF font `name` (`ABCDEF+Calibri-Bold`,
    /// `TimesNewRomanPS-ItalicMT`, `Arial,Bold`) among the families
    /// `installed`: its own family, one made to the same widths, or one
    /// of its kind (serif, sans or fixed).
    fn of(name: &str, installed: &[String]) -> Look {
        let name = name.split_once('+').map_or(name, |(_, n)| n);
        let base = name.split(['-', ',']).next().unwrap_or(name);
        let lower = name.to_ascii_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
        let bold = has(&["bold", "black", "heavy", "semibold", "demi"]);
        let italic = has(&["italic", "oblique"]);
        // `TimesNewRomanPSMT` as `Times New Roman`.
        let base = base.trim_end_matches("MT").trim_end_matches("PS");
        let mut spaced = String::new();
        for (i, c) in base.chars().enumerate() {
            if i > 0 && c.is_ascii_uppercase() && !spaced.ends_with(' ') {
                spaced.push(' ');
            }
            spaced.push(c);
        }
        let kind: &[&str] = if has(&["courier", "mono", "consol", "typewriter"]) {
            &[
                "Courier New",
                "Liberation Mono",
                "Nimbus Mono PS",
                "DejaVu Sans Mono",
            ]
        } else if has(&[
            "times", "serif", "roman", "georgia", "garamond", "cambria", "book", "caladea",
            "minion", "palatino",
        ]) && !has(&["sans"])
        {
            &[
                "Times New Roman",
                "Liberation Serif",
                "Nimbus Roman",
                "DejaVu Serif",
                "Noto Serif",
            ]
        } else {
            &[
                "Arial",
                "Liberation Sans",
                "Nimbus Sans",
                "DejaVu Sans",
                "Noto Sans",
            ]
        };
        // Families made to another's widths.
        let alike: &[&str] = match base.to_ascii_lowercase().as_str() {
            "calibri" => &["Carlito"],
            "cambria" => &["Caladea"],
            "arial" | "helvetica" => &["Liberation Sans", "Nimbus Sans"],
            "times" | "timesnewroman" => &["Liberation Serif", "Nimbus Roman"],
            "courier" | "couriernew" => &["Liberation Mono", "Nimbus Mono PS"],
            _ => &[],
        };
        let wanted = [base, spaced.as_str()]
            .into_iter()
            .chain(alike.iter().copied())
            .chain(kind.iter().copied());
        let family = wanted
            .filter_map(|w| installed.iter().find(|f| f.eq_ignore_ascii_case(w)))
            .next()
            .map(|f| f.clone().into());
        Look {
            family,
            bold,
            italic,
        }
    }
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
        self.runs.next = None;
        self.text_editing = !self.text_editing;
        self.adding = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
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
        self.runs.next = None;
        self.text_editing = false;
        self.adding = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
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
        let key = self.key.clone();
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
                    let doc = super::cos(bytes, key.as_deref()).ok()?;
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
        let clicked = self
            .runs
            .pages
            .get(&page)
            .and_then(|blocks| blocks.get(block))
            .map(|b| {
                (
                    page,
                    [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0],
                )
            });
        self.finish_text_edit(true, cx);
        let Some(text) = self
            .runs
            .pages
            .get(&page)
            .and_then(|blocks| blocks.get(block))
            .map(|b| b.text.clone())
        else {
            // The file is being changed: open it once read again.
            self.runs.next = clicked;
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
        let look = match target {
            Target::Block(i) => self
                .runs
                .pages
                .get(&page)
                .and_then(|blocks| blocks.get(i))
                .map(|b| Look::of(&b.font, &cx.text_system().all_font_names()))
                .unwrap_or_default(),
            Target::New(_) => Look::default(),
        };
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
            look,
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
        let key = self.key.clone();
        self.change_document_keyed(pages, failed, key, change, cx);
    }

    /// As [`Viewer::change_document`], the changed file opening with
    /// password `after` (one just put on, or none once taken off).
    pub(super) fn change_document_keyed(
        &mut self,
        pages: Vec<usize>,
        failed: &'static str,
        after: Option<Arc<str>>,
        change: impl FnOnce(&katna_pdf_cos::Document) -> Result<Vec<u8>, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = self.key.clone();
        let kept = after.clone();
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
                    let base = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let out = Arc::new(change(&base)?);
                    let doc = crate::engine::engine()
                        .open(out.clone(), after.as_deref().unwrap_or(""))
                        .map_err(|e| e.to_string())?;
                    let cos =
                        super::cos(out.to_vec(), after.as_deref()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((out, doc, cos))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match written {
                    Ok((bytes, doc, cos)) => {
                        this.key = kept;
                        this.forget_runs();
                        let design = this
                            .designing
                            .is_some()
                            .then(|| super::design::read_form(&cos));
                        this.show_annotated(bytes, doc, cos, &pages, None);
                        if let Some(read) = design {
                            this.design_changed(read, cx);
                        }
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
        width: f32,
        window: &mut Window,
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
        if let Some((_, [x, y])) = self.runs.next.filter(|(p, _)| *p == page) {
            self.runs.next = None;
            if let Some(i) = blocks.iter().position(|b| {
                (b.rect[0]..=b.rect[2]).contains(&x) && (b.rect[1]..=b.rect[3]).contains(&y)
            }) {
                cx.defer_in(window, move |this, window, cx| {
                    this.start_text_edit(page, i, window, cx)
                });
            }
        }
        let shown = self.shown;
        let accent = self.theme.katna.accent & 0xffff_ff00;
        // With a tool for adding in hand, the paragraphs are not boxes to
        // click.
        let listed = if self.adding.is_some()
            || self.redacting.is_some()
            || self.linking.is_some()
            || self.designing.is_some()
        {
            &[][..]
        } else {
            &blocks[..]
        };
        let mut out: Vec<AnyElement> = listed
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let b = katna_pdf_render::map_rect(transform, block.rect).map(|v| v * shown);
                let typing = self
                    .typing
                    .as_ref()
                    .filter(|t| t.page == page && t.target == Target::Block(i));
                if let Some(typing) = typing {
                    return self.typing_box(block, b, width * shown, typing);
                }
                // Outlined only under the pointer, on its own glyphs'
                // area, so neighbours' boxes never cross.
                div()
                    .id(("text-run", page * 100_000 + i))
                    .absolute()
                    .left(desktop_px(b[0]))
                    .top(desktop_px(b[1]))
                    .w(desktop_px(b[2] - b[0]))
                    .h(desktop_px(b[3] - b[1]))
                    .rounded(px(radius::XS / 2.0))
                    .hover(move |s| {
                        s.bg(gpui::rgba(accent | 0x14))
                            .border_1()
                            .border_color(gpui::rgba(accent | 0xaa))
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
        out.extend(self.redact_overlay(page));
        out.extend(self.link_overlay(page));
        out.extend(self.design_overlay(page));
        out
    }

    /// The box paragraph `block` is typed in, over its place `b` on a
    /// page `page_width` wide (both as shown): its first line's baseline
    /// where it was, in its size, colour and line spacing, and as wide as
    /// the file will wrap it to.
    fn typing_box(
        &self,
        block: &TextBlock,
        b: [f32; 4],
        page_width: f32,
        typing: &Typing,
    ) -> AnyElement {
        let shown = self.shown;
        let size = block.size * shown;
        let lead = if block.leading > block.size * 0.8 {
            block.leading * shown
        } else {
            1.2 * size
        };
        let lines = block.runs.len().max(1) as f32;
        // The glyphs' height on one line, centred in its line's height.
        let glyphs = (b[3] - b[1]) - (lines - 1.0) * lead;
        let top = b[1] - (lead - glyphs) / 2.0;
        // A line alone grows to a right margin as wide as its left one,
        // as `edit_block` wraps it; a little room for the face here being
        // a touch wider than the file's.
        let wide = if block.runs.len() == 1 {
            (b[2] - b[0]).max(page_width - 2.0 * b[0])
        } else {
            b[2] - b[0]
        } + 0.3 * size;
        let pad = 2.0;
        let [r, g, bl] = self.page_colors().pixel(block.colour);
        let look = &typing.look;
        div()
            .id("text-typing")
            .absolute()
            .left(desktop_px(b[0] - pad))
            .top(desktop_px(top - pad))
            .w(desktop_px(wide + 2.0 * pad))
            .min_h(desktop_px(lines * lead + 2.0 * pad))
            .p(desktop_px(pad - 1.0))
            .rounded(px(radius::XS / 2.0))
            .bg(self.paper())
            .border_1()
            .border_color(gpui::rgb(self.theme.katna.accent >> 8))
            .text_size(desktop_px(size))
            .line_height(desktop_px(lead))
            .text_color(gpui::rgb(u32::from_be_bytes([0, r, g, bl])))
            .when_some(look.family.clone(), |d, family| d.font_family(family))
            .when(look.bold, |d| d.font_weight(gpui::FontWeight::BOLD))
            .when(look.italic, |d| d.italic())
            .cursor(gpui::CursorStyle::IBeam)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(typing.input.clone())
            .into_any_element()
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
