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
use katna_pdf_cos::{Layout, PageObject, TextAlign, TextBlock};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_area::TextArea;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::{Content, Viewer, fonts};
use crate::text::t;

/// The paragraphs, pictures and shapes read so far, by page, and the
/// pages being read.
#[derive(Default)]
pub(super) struct Runs {
    pages: HashMap<usize, Arc<Vec<TextBlock>>>,
    pub(super) objects: HashMap<usize, Arc<Vec<PageObject>>>,
    reading: HashSet<usize>,
    /// A paragraph clicked or dragged while the one before it was being
    /// written into the file: its page and a point in it, in the page's
    /// space, to open or move once the page's paragraphs are read again.
    next: Option<(usize, [f32; 2], Next)>,
    /// Bumped when the file changes, so runs read from the old one are
    /// dropped.
    generation: u64,
    /// Whether the file is being changed: nothing is read from it until
    /// the change is in, as what was read would be of the old one.
    changing: bool,
}

/// What to do with a paragraph once its page is read again.
#[derive(Clone)]
enum Next {
    Edit,
    /// Move it by so much in the page's own space.
    Move([f32; 2]),
    /// Show it at a size, in a colour, each where given.
    Style(Option<f32>, Option<[f32; 3]>),
    /// Put it in a face.
    Font(FontChoice),
    /// Lay it out again: lined up, spaced.
    Layout(Layout),
}

/// The spacing fields open under the text inspector: between letters,
/// between words, line to line.
pub(super) struct SpacingMenu {
    fields: [Entity<TextInput>; 3],
    _fields: Vec<Subscription>,
}

/// A face chosen for a paragraph: a family installed here (or a
/// standard one) and its weight and slant.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct FontChoice {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
}

/// The list of fonts open under the text inspector, with a box to
/// narrow it.
pub(super) struct FontMenu {
    filter: Entity<TextInput>,
    _filter: Subscription,
}

/// A paragraph being dragged: from a place to a place on the page shown,
/// in points.
pub(super) struct TextMoving {
    page: usize,
    block: usize,
    from: [f32; 2],
    to: [f32; 2],
}

/// A drag shorter than this, in points shown, is a click.
const CLICK: f32 = 3.0;

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
        // `DejaVuSans` is `DejaVu Sans`.
        let plain = |s: &str| {
            s.chars()
                .filter(|c| !c.is_whitespace())
                .map(|c| c.to_ascii_lowercase())
                .collect::<String>()
        };
        let family = wanted
            .filter_map(|w| installed.iter().find(|f| plain(f) == plain(w)))
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

impl TextMoving {
    fn dragged(&self) -> bool {
        (self.to[0] - self.from[0]).hypot(self.to[1] - self.from[1]) >= CLICK
    }
}

impl Typing {
    /// The paragraph being typed in, if not new text.
    fn block(&self) -> Option<usize> {
        match self.target {
            Target::Block(i) => Some(i),
            Target::New(_) => None,
        }
    }
}

impl Viewer {
    /// Turns Edit text on or off; other tools are put down.
    pub(super) fn toggle_text_edit(&mut self, cx: &mut Context<Self>) {
        self.finish_text_edit(true, cx);
        self.runs.next = None;
        self.text_moving = None;
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
        self.text_moving = None;
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
        if self.runs.changing
            || self.runs.pages.contains_key(&page)
            || !self.runs.reading.insert(page)
        {
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
            self.runs.next = clicked.map(|(p, at)| (p, at, Next::Edit));
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
        self.change_page_told(
            typing.page,
            move |doc| {
                // Letters the font lacks are said, once the change shows.
                let told = doc.missing_glyphs(typing.page, &runs, &text).map(|m| {
                    t(
                        "text-edit-stood-in",
                        &[
                            ("font", &m.font),
                            ("chars", &m.chars),
                            ("standin", m.stand_in.as_deref().unwrap_or("")),
                        ],
                    )
                });
                let bytes = doc
                    .edit_block(typing.page, &runs, &text)
                    .map_err(|e| e.to_string())?;
                Ok((bytes, told))
            },
            cx,
        );
    }

    /// As [`Viewer::change_page`], with a notice to show once the change
    /// does, when `change` gives one.
    pub(super) fn change_page_told(
        &mut self,
        page: usize,
        change: impl FnOnce(&katna_pdf_cos::Document) -> Result<(Vec<u8>, Option<String>), String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = self.key.clone();
        self.change_document_told(vec![page], "text-edit-failed", key, change, cx);
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
        self.change_document_told(
            pages,
            failed,
            after,
            move |doc| change(doc).map(|bytes| (bytes, None)),
            cx,
        );
    }

    /// As [`Viewer::change_document_keyed`], with a notice to show once
    /// the change does, when `change` gives one.
    fn change_document_told(
        &mut self,
        pages: Vec<usize>,
        failed: &'static str,
        after: Option<Arc<str>>,
        change: impl FnOnce(&katna_pdf_cos::Document) -> Result<(Vec<u8>, Option<String>), String>
        + Send
        + 'static,
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
        self.runs.changing = true;
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let base = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let (out, told) = change(&base)?;
                    let out = Arc::new(out);
                    let doc = crate::engine::engine()
                        .open(out.clone(), after.as_deref().unwrap_or(""))
                        .map_err(|e| e.to_string())?;
                    let cos =
                        super::cos(out.to_vec(), after.as_deref()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((out, doc, cos, told))
                })
                .await;
            this.update(cx, |this, cx| {
                this.runs.changing = false;
                if this.generation != generation {
                    cx.notify();
                    return;
                }
                match written {
                    Ok((bytes, doc, cos, told)) => {
                        this.key = kept;
                        if told.is_some() {
                            this.notice = told;
                        }
                        this.forget_runs();
                        let design = this
                            .designing
                            .is_some()
                            .then(|| super::design::read_form(&cos));
                        let marks = this.marking.is_some().then(|| cos.page_marks());
                        this.show_annotated(bytes, doc, cos, &pages, None);
                        if let Some(read) = design {
                            this.design_changed(read, cx);
                        }
                        if let Some(marks) = marks {
                            this.marks_changed(marks);
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
        if let Some((_, [x, y], next)) = self.runs.next.take_if(|(p, _, _)| *p == page)
            && let Some(i) = blocks.iter().position(|b| {
                (b.rect[0]..=b.rect[2]).contains(&x) && (b.rect[1]..=b.rect[3]).contains(&y)
            })
        {
            cx.defer_in(window, move |this, window, cx| match next {
                Next::Edit => this.start_text_edit(page, i, window, cx),
                Next::Move(by) => this.move_text(page, i, by, cx),
                Next::Style(size, colour) => this.style_text(page, i, size, colour, cx),
                Next::Font(choice) => this.font_text(page, i, choice, cx),
                Next::Layout(layout) => this.layout_text(page, i, layout, cx),
            });
        }
        let dragging = self
            .text_moving
            .as_ref()
            .filter(|m| m.page == page && m.dragged())
            .map(|m| (m.block, [m.to[0] - m.from[0], m.to[1] - m.from[1]]));
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
                    let by = dragging.filter(|(b, _)| *b == i).map(|(_, by)| by);
                    return self.typing_box(block, b, width * shown, typing, by, cx);
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
                    // Not lit under a paragraph being dragged over it.
                    .when(dragging.is_none(), |d| {
                        d.hover(move |s| {
                            s.bg(gpui::rgba(accent | 0x14))
                                .border_1()
                                .border_color(gpui::rgba(accent | 0xaa))
                        })
                    })
                    .cursor(gpui::CursorStyle::IBeam)
                    .when_some(dragging.filter(|(b, _)| *b == i), |d, (_, [dx, dy])| {
                        // Dragged: its box goes along with the pointer.
                        d.left(desktop_px(b[0] + dx * shown))
                            .top(desktop_px(b[1] + dy * shown))
                            .border_1()
                            .border_color(gpui::rgba(accent | 0xcc))
                            .bg(gpui::rgba(accent | 0x14))
                            .cursor(gpui::CursorStyle::ClosedHand)
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                            // With Ctrl, the press picks objects under
                            // the paragraph instead.
                            if event.modifiers.control || event.modifiers.platform {
                                return;
                            }
                            cx.stop_propagation();
                            this.text_move_press(page, i, event.position, cx);
                        }),
                    )
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
        out.extend(self.objects_overlay(page, cx));
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
        dragged: Option<[f32; 2]>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = self.shown;
        let size = block.size * shown;
        // Along with the pointer while dragged by its grip.
        let [dx, dy] = dragged.map_or([0.0; 2], |[x, y]| [x * shown, y * shown]);
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
        // A turned paragraph is typed in a box the right way up at the
        // top left of where it is, as long as its lines run.
        let turned = block.angle != 0.0;
        let wide = if turned {
            block.width * shown
        } else if block.runs.len() == 1 {
            (b[2] - b[0]).max(page_width - 2.0 * b[0])
        } else {
            b[2] - b[0]
        } + 0.3 * size;
        let top = if turned { b[1] } else { top };
        let pad = 2.0;
        let [r, g, bl] = self.page_colors().pixel(block.colour);
        let look = &typing.look;
        let (page, index) = (typing.page, typing.block().unwrap_or(0));
        // A grip on its left edge, to drag the paragraph elsewhere.
        let grip = 10.0;
        let handle = div()
            .id("text-grip")
            .absolute()
            .left(desktop_px(-grip - 1.0))
            .top(desktop_px(-1.0))
            .w(desktop_px(grip))
            .h(desktop_px(lines * lead + 2.0 * pad))
            .rounded_l(px(radius::XS / 2.0))
            .bg(gpui::rgb(self.theme.katna.accent >> 8))
            .cursor(if dragged.is_some() {
                gpui::CursorStyle::ClosedHand
            } else {
                gpui::CursorStyle::OpenHand
            })
            .tip(t("text-grip", &[]), &self.theme.katna)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.text_move_press(page, index, event.position, cx);
                }),
            );
        div()
            .id("text-typing")
            .absolute()
            .left(desktop_px(b[0] - pad + dx))
            .top(desktop_px(top - pad + dy))
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
            .child(handle)
            .child(typing.input.clone())
            .into_any_element()
    }

    /// A press on paragraph `block` of page `page`: a drag from here
    /// moves it; let go without one, it opens for typing.
    fn text_move_press(
        &mut self,
        page: usize,
        block: usize,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((_, at)) = self.page_point(position) else {
            return;
        };
        self.text_moving = Some(TextMoving {
            page,
            block,
            from: at,
            to: at,
        });
        cx.notify();
    }

    pub(super) fn text_move_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self.text_moving.as_ref().map(|m| m.page) else {
            return false;
        };
        let shown = desktop_px(self.shown);
        if let Some(b) = self.page_bounds.borrow().get(&page).copied()
            && let Some(moving) = self.text_moving.as_mut()
        {
            moving.to = [
                (position.x - b.left()) / shown,
                (position.y - b.top()) / shown,
            ];
        }
        cx.notify();
        true
    }

    /// The pointer let go on a dragged paragraph: it moves where it was
    /// dragged to, or, after a click, opens for typing.
    pub(super) fn text_move_release(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(moving) = self.text_moving.take() else {
            return false;
        };
        cx.notify();
        let (page, block) = (moving.page, moving.block);
        if !moving.dragged() {
            self.start_text_edit(page, block, window, cx);
            return true;
        }
        let Some(inv) = self.inverse(page) else {
            return true;
        };
        let [x0, y0] = inv.map(moving.from);
        let [x1, y1] = inv.map(moving.to);
        let by = [x1 - x0, y1 - y0];
        // The paragraph being typed in is written first, then moved
        // once the page is read again.
        let typed = self
            .typing
            .as_ref()
            .is_some_and(|t| t.page == page && t.target == Target::Block(block));
        let at = self
            .runs
            .pages
            .get(&page)
            .and_then(|blocks| blocks.get(block))
            .map(|b| [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0]);
        if typed {
            self.finish_text_edit(true, cx);
            if self.runs.pages.contains_key(&page) {
                self.move_text(page, block, by, cx);
            } else if let Some(at) = at {
                self.runs.next = Some((page, at, Next::Move(by)));
            }
        } else {
            self.move_text(page, block, by, cx);
        }
        true
    }

    /// Moves paragraph `block` of page `page` by `by` in the page's own
    /// space, and opens it for typing where it lands.
    fn move_text(&mut self, page: usize, block: usize, by: [f32; 2], cx: &mut Context<Self>) {
        let Some((runs, at)) = self.block_runs(page, block) else {
            return;
        };
        self.change_page(
            page,
            move |doc| {
                doc.move_block(page, &runs, by[0], by[1])
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        self.runs.next = Some((page, [at[0] + by[0], at[1] + by[1]], Next::Edit));
    }

    /// Shows paragraph `block` of page `page` at `size` points and in
    /// `colour`, each where given, and opens it for typing again.
    fn style_text(
        &mut self,
        page: usize,
        block: usize,
        size: Option<f32>,
        colour: Option<[f32; 3]>,
        cx: &mut Context<Self>,
    ) {
        let Some((runs, at)) = self.block_runs(page, block) else {
            return;
        };
        self.change_page(
            page,
            move |doc| {
                doc.style_block(page, &runs, size, colour)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        self.runs.next = Some((page, at, Next::Edit));
    }

    /// Puts paragraph `block` of page `page` in face `choice`, its
    /// glyphs put into the file, and opens it for typing again.
    fn font_text(&mut self, page: usize, block: usize, choice: FontChoice, cx: &mut Context<Self>) {
        let Some((runs, at)) = self.block_runs(page, block) else {
            return;
        };
        let Some(text) = self
            .runs
            .pages
            .get(&page)
            .and_then(|blocks| blocks.get(block))
            .map(|b| b.text.clone())
        else {
            return;
        };
        self.change_document(
            vec![page],
            "text-font-failed",
            move |doc| {
                let font =
                    fonts::installed().new_font(&choice.family, choice.bold, choice.italic)?;
                doc.refont_block(page, &runs, &text, &font)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        self.runs.next = Some((page, at, Next::Edit));
    }

    /// Lays paragraph `block` of page `page` out as `layout` says and
    /// opens it for typing again.
    fn layout_text(&mut self, page: usize, block: usize, layout: Layout, cx: &mut Context<Self>) {
        let Some((runs, at)) = self.block_runs(page, block) else {
            return;
        };
        self.change_page(
            page,
            move |doc| {
                doc.layout_block(page, &runs, &layout)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        self.runs.next = Some((page, at, Next::Edit));
    }

    /// Paragraph `block` of page `page`'s lines and middle, in the
    /// page's own space.
    fn block_runs(&self, page: usize, block: usize) -> Option<(Vec<usize>, [f32; 2])> {
        let b = self.runs.pages.get(&page)?.get(block)?;
        Some((
            b.runs.clone(),
            [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0],
        ))
    }

    /// The paragraph being typed in, and its page and index.
    fn typed_block(&self) -> Option<(usize, usize, &TextBlock)> {
        let typing = self.typing.as_ref()?;
        let block = typing.block()?;
        let b = self.runs.pages.get(&typing.page)?.get(block)?;
        Some((typing.page, block, b))
    }

    /// Does `what` to the paragraph being typed in, once what was typed
    /// is in the file: now, or once the page is read again.
    fn restyle_typed(&mut self, what: Next, cx: &mut Context<Self>) {
        let Some((page, block, b)) = self.typed_block() else {
            return;
        };
        let at = [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0];
        self.finish_text_edit(true, cx);
        if self.runs.pages.contains_key(&page) {
            match what {
                Next::Style(size, colour) => self.style_text(page, block, size, colour, cx),
                Next::Move(by) => self.move_text(page, block, by, cx),
                Next::Font(choice) => self.font_text(page, block, choice, cx),
                Next::Layout(layout) => self.layout_text(page, block, layout, cx),
                Next::Edit => {}
            }
        } else {
            self.runs.next = Some((page, at, what));
        }
    }

    /// The paragraph being typed in shown `by` points bigger (or
    /// smaller).
    pub(super) fn resize_typed(&mut self, by: f32, cx: &mut Context<Self>) {
        let Some((_, _, b)) = self.typed_block() else {
            return;
        };
        let size = (b.size + by).clamp(4.0, 144.0);
        if (size - b.size).abs() > 0.01 {
            self.restyle_typed(Next::Style(Some(size), None), cx);
        }
    }

    /// The paragraph being typed in shown in `ink` (RGB, 0 to 1).
    pub(super) fn colour_typed(&mut self, ink: [f32; 3], cx: &mut Context<Self>) {
        if self.typed_block().is_some() {
            self.restyle_typed(Next::Style(None, Some(ink)), cx);
        }
    }

    /// The face the paragraph being typed in is in now, as the inspector
    /// shows it: the installed family nearest its font, or the font's
    /// own name, and whether it is bold and italic.
    fn typed_face(&self, cx: &Context<Self>) -> Option<FontChoice> {
        let (_, _, b) = self.typed_block()?;
        let look = Look::of(&b.font, &cx.text_system().all_font_names());
        let family = match look.family {
            Some(f) => f.to_string(),
            None => {
                let name = b.font.split_once('+').map_or(b.font.as_str(), |(_, n)| n);
                name.split(['-', ',']).next().unwrap_or(name).to_string()
            }
        };
        Some(FontChoice {
            family,
            bold: look.bold,
            italic: look.italic,
        })
    }

    /// The paragraph being typed in put in family `family`, keeping its
    /// weight and slant.
    pub(super) fn refont_typed(&mut self, family: String, cx: &mut Context<Self>) {
        let Some(face) = self.typed_face(cx) else {
            return;
        };
        self.font_menu = None;
        cx.notify();
        let choice = FontChoice {
            family,
            ..face.clone()
        };
        if choice != face {
            self.restyle_typed(Next::Font(choice), cx);
        }
    }

    /// The paragraph being typed in made bold, or not, and italic, or
    /// not, where given.
    pub(super) fn reweigh_typed(
        &mut self,
        bold: Option<bool>,
        italic: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(face) = self.typed_face(cx) else {
            return;
        };
        let choice = FontChoice {
            bold: bold.unwrap_or(face.bold),
            italic: italic.unwrap_or(face.italic),
            ..face.clone()
        };
        if choice != face {
            self.restyle_typed(Next::Font(choice), cx);
        }
    }

    /// The paragraph being typed in lined up as `align` says.
    pub(super) fn align_typed(&mut self, align: TextAlign, cx: &mut Context<Self>) {
        if self.typed_block().is_some_and(|(_, _, b)| b.align != align) {
            self.restyle_typed(
                Next::Layout(Layout {
                    align: Some(align),
                    ..Layout::default()
                }),
                cx,
            );
        }
    }

    /// Opens or closes the spacing fields under the inspector, filled
    /// with the paragraph's spacing now.
    pub(super) fn toggle_spacing_menu(&mut self, cx: &mut Context<Self>) {
        if self.spacing_menu.take().is_none() {
            let Some((_, _, b)) = self.typed_block() else {
                return;
            };
            let now = [b.char_spacing, b.word_spacing, b.leading];
            let accent = gpui::rgba(self.theme.katna.accent).into();
            let fields = now.map(|v| {
                cx.new(|cx| {
                    let mut input = TextInput::new("", cx);
                    input.set_accent(accent);
                    input.set_text(format!("{}", (v * 10.0).round() / 10.0), cx);
                    input
                })
            });
            let subs = fields
                .iter()
                .map(|f| {
                    cx.subscribe(f, |this, _, event, cx| match event {
                        InputEvent::Submit => this.apply_spacing(cx),
                        InputEvent::Cancel => {
                            this.spacing_menu = None;
                            cx.notify();
                        }
                        InputEvent::Changed => {}
                    })
                })
                .collect();
            self.spacing_menu = Some(SpacingMenu {
                fields,
                _fields: subs,
            });
        }
        cx.notify();
    }

    /// The spacing typed into the fields put on the paragraph being
    /// typed in, where it differs from what it has.
    fn apply_spacing(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = self.spacing_menu.take() else {
            return;
        };
        let Some((_, _, b)) = self.typed_block() else {
            return;
        };
        let typed = menu
            .fields
            .each_ref()
            .map(|f| super::objects::length(f.read(cx).text()));
        let differs = |v: Option<f32>, now: f32| v.filter(|v| (v - now).abs() > 0.01);
        let layout = Layout {
            align: None,
            char_spacing: differs(typed[0], b.char_spacing),
            word_spacing: differs(typed[1], b.word_spacing),
            leading: differs(typed[2], b.leading).map(|l| l.max(1.0)),
        };
        cx.notify();
        if layout != Layout::default() {
            self.restyle_typed(Next::Layout(layout), cx);
        }
    }

    /// The spacing fields, under the inspector's spacing button.
    fn spacing_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.spacing_menu.as_ref()?;
        let th = self.theme;
        let labels = [
            "text-spacing-letters",
            "text-spacing-words",
            "text-spacing-lines",
        ];
        let rows = labels.iter().zip(&menu.fields).map(|(label, field)| {
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(t(label, &[])),
                )
                .child(
                    div()
                        .w(px(72.0))
                        .h(px(28.0))
                        .px(px(space::S2))
                        .flex()
                        .items_center()
                        .rounded(px(radius::XS))
                        .bg(th.field)
                        .text_size(px(text::SMALL))
                        .child(field.clone()),
                )
                .child(
                    div()
                        .w(px(20.0))
                        .text_size(px(text::SMALL))
                        .text_color(gpui::rgba(th.katna.text_dim))
                        .child(t("text-spacing-pt", &[])),
                )
        });
        let card = div()
            .id("text-spacing-menu")
            .occlude()
            .w(px(280.0))
            .mt(px(space::S1))
            .p(px(space::S3))
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.spacing_menu = None;
                cx.notify();
            }))
            .children(rows)
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(gpui::rgba(th.katna.text_dim))
                    .child(t("text-spacing-hint", &[])),
            );
        Some(
            div()
                .absolute()
                .top_full()
                .left_0()
                .child(gpui::deferred(
                    katna_ui::anchored()
                        .anchor(gpui::Anchor::TopLeft)
                        .child(card),
                ))
                .into_any_element(),
        )
    }

    /// Opens or closes the list of fonts under the inspector.
    pub(super) fn toggle_font_menu(&mut self, cx: &mut Context<Self>) {
        if self.font_menu.take().is_none() {
            let accent = gpui::rgba(self.theme.katna.accent).into();
            let filter = cx.new(|cx| {
                let mut input = TextInput::new(t("text-font-find", &[]), cx);
                input.set_accent(accent);
                input
            });
            let sub = cx.subscribe(&filter, |this, _, event, cx| match event {
                InputEvent::Cancel => {
                    this.font_menu = None;
                    cx.notify();
                }
                _ => cx.notify(),
            });
            self.font_menu = Some(FontMenu {
                filter,
                _filter: sub,
            });
        }
        cx.notify();
    }

    /// The list of fonts, under the inspector's font field.
    fn font_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.font_menu.as_ref()?;
        let th = self.theme;
        let now = self.typed_face(cx)?;
        let wanted = menu.filter.read(cx).text().trim().to_lowercase();
        let rows = fonts::installed()
            .families()
            .iter()
            .filter(|f| wanted.is_empty() || f.to_lowercase().contains(&wanted))
            .take(400)
            .map(|family| {
                let chosen = family.eq_ignore_ascii_case(&now.family);
                let name = family.clone();
                div()
                    .id(gpui::SharedString::from(family.clone()))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S3))
                    .py(px(space::S1))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .font_family(gpui::SharedString::from(family.clone()))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.refont_typed(name.clone(), cx)),
                    )
                    .child(div().flex_1().child(family.clone()))
                    .when(chosen, |row| {
                        row.child(
                            gpui::svg()
                                .path("icons/check.svg")
                                .size(px(16.0))
                                .flex_none()
                                .text_color(gpui::rgb(th.katna.accent >> 8)),
                        )
                    })
            });
        let list = div()
            .id("text-font-menu")
            .occlude()
            .w(px(280.0))
            .mt(px(space::S1))
            .p(px(space::S1))
            .flex()
            .flex_col()
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.font_menu = None;
                cx.notify();
            }))
            .child(
                div()
                    .h(px(28.0))
                    .mb(px(space::S1))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .rounded(px(radius::XS))
                    .bg(th.field)
                    .child(menu.filter.clone()),
            )
            .child(
                div()
                    .id("text-font-list")
                    .max_h(px(320.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            );
        Some(
            div()
                .absolute()
                .top_full()
                .left_0()
                .child(gpui::deferred(
                    katna_ui::anchored()
                        .anchor(gpui::Anchor::TopLeft)
                        .child(list),
                ))
                .into_any_element(),
        )
    }

    /// Copies the paragraph being typed in, as it is in the file, for
    /// Ctrl+V on any page of any open file.
    fn copy_typed(&mut self, cx: &mut Context<Self>) {
        let Some((page, _, b)) = self.typed_block() else {
            return;
        };
        let runs = b.runs.clone();
        self.copy_block(page, runs, cx);
    }

    /// The paragraph being typed in taken out of the page.
    pub(super) fn delete_typed(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((page, _, b)) = self.typed_block() else {
            return false;
        };
        let runs = b.runs.clone();
        self.typing = None;
        self.change_page(
            page,
            move |doc| doc.edit_block(page, &runs, "").map_err(|e| e.to_string()),
            cx,
        );
        true
    }

    /// The paragraph being typed in, in the row of tools: its font and
    /// size, with buttons for bigger and smaller, and one to delete it.
    pub(super) fn text_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (_, _, block) = self.typed_block()?;
        let th = self.theme;
        let face = self.typed_face(cx)?;
        let menu = self.font_menu(cx);
        let spacing = self.spacing_menu(cx);
        let align = block.align;
        let field = |wide: f32| {
            div()
                .h(px(28.0))
                .px(px(space::S2))
                .flex()
                .items_center()
                .rounded(px(radius::XS))
                .bg(th.field)
                .text_size(px(text::SMALL))
                .when(wide > 0.0, |d| d.max_w(px(wide)).overflow_hidden())
        };
        let step = |id: &'static str, label: &'static str, tip: &'static str, by: f32| {
            div()
                .id(id)
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::XS))
                .cursor_pointer()
                .hover(move |s| s.bg(th.hover))
                .text_size(px(text::SMALL))
                .tip(t(tip, &[]), &th.katna)
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.resize_typed(by, cx)))
        };
        let toggle = |id: &'static str, icon: &'static str, tip: &'static str, set: bool| {
            let colour = if set {
                th.katna.accent >> 8
            } else {
                th.katna.text_dim
            };
            widgets::icon_button_colored(id, icon, 18.0, colour, &th.katna)
                .when(set, |b| b.bg(th.hover))
                .tip(t(tip, &[]), &th.katna)
        };
        let bold = face.bold;
        let italic = face.italic;
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(space::S1))
            .child(
                div()
                    .relative()
                    .child(
                        field(180.0)
                            .id("text-font")
                            .cursor_pointer()
                            .hover(move |s| s.bg(th.hover))
                            .tip(t("text-font-tip", &[]), &th.katna)
                            .font_family(gpui::SharedString::from(face.family.clone()))
                            .when(bold, |d| d.font_weight(gpui::FontWeight::BOLD))
                            .when(italic, |d| d.italic())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_font_menu(cx)))
                            .child(div().overflow_hidden().child(face.family.clone())),
                    )
                    .children(menu),
            )
            .child(
                toggle("text-bold", "format-bold", "text-bold", bold).on_click(
                    cx.listener(move |this, _, _, cx| this.reweigh_typed(Some(!bold), None, cx)),
                ),
            )
            .child(
                toggle("text-italic", "format-italic", "text-italic", italic).on_click(
                    cx.listener(move |this, _, _, cx| this.reweigh_typed(None, Some(!italic), cx)),
                ),
            )
            .child(step("text-smaller", "A−", "text-smaller", -1.0))
            .child(field(0.0).child(t(
                "text-size",
                &[("size", &format!("{}", block.size.round()))],
            )))
            .child(step("text-bigger", "A+", "text-bigger", 1.0))
            .child(super::separator(th))
            .children(
                [
                    (TextAlign::Left, "text-align-left"),
                    (TextAlign::Center, "text-align-centre"),
                    (TextAlign::Right, "text-align-right"),
                ]
                .map(|(how, id)| {
                    toggle(id, id, id, align == how)
                        .on_click(cx.listener(move |this, _, _, cx| this.align_typed(how, cx)))
                }),
            )
            .child(
                div()
                    .relative()
                    .child(
                        toggle("text-spacing", "spacing", "text-spacing", spacing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_spacing_menu(cx))),
                    )
                    .children(spacing),
            )
            .child(super::separator(th))
            .child(
                widgets::icon_button("text-copy", "copies", 18.0, &th.katna)
                    .tip(t("text-copy", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.copy_typed(cx);
                    })),
            )
            .child(
                widgets::icon_button("text-delete", "trash", 18.0, &th.katna)
                    .tip(t("text-delete", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.delete_typed(cx);
                    })),
            );
        Some(row.into_any_element())
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
