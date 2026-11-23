// SPDX-License-Identifier: GPL-3.0-or-later

//! Redacting (docs/PLAN.md §4.3, Phase 4; storyboard E10, E11): with
//! the redact tool from the edit row, boxes are dragged over what must
//! go, or found from words, PAN and Aadhaar numbers, phone numbers and
//! email addresses, each kind counted as the panel opens. Applying asks
//! first, then takes what lies under the boxes out of a new copy of the
//! file (`katna_pdf_cos::redact`), not only out of sight, draws a black
//! box over each, and opens the copy; the file shown stays as it was.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{Pattern, RedactOptions};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::annots::Draft;
use super::sign::check_box;
use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// The kinds of number that can be found, and their names in the panel.
const FINDS: [(&str, Pattern); 4] = [
    ("redact-find-pan", Pattern::Pan),
    ("redact-find-aadhaar", Pattern::Aadhaar),
    ("redact-find-phone", Pattern::Phone),
    ("redact-find-email", Pattern::Email),
];

/// Where a box marked came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum From {
    Drawn,
    /// One of [`FINDS`].
    Kind(usize),
    /// The words typed.
    Words,
}

/// A box marked: a page, a box in its own space, and the words under
/// it when it was found.
#[derive(Debug, Clone, PartialEq)]
struct Area {
    page: usize,
    rect: [f32; 4],
    said: String,
    from: From,
}

/// The file as it is now (its unsaved bytes, if any), where it is, and
/// its password.
type Source = (Option<Arc<Vec<u8>>>, PathBuf, Option<Arc<str>>);

/// What a search found: on which page, where, and what it reads.
type Found = Vec<(usize, [f32; 4], String)>;

/// The redact tool, while in hand.
pub(super) struct Redacting {
    /// The areas marked.
    areas: Vec<Area>,
    /// Which of [`FINDS`] are marked.
    find: [bool; 4],
    /// Each of [`FINDS`] found in the file, once looked for.
    found: [Option<Found>; 4],
    words: Entity<TextInput>,
    /// How many places the words typed were found, once looked for.
    words_found: Option<usize>,
    /// Bumped as the words change, so an older search is dropped.
    asked: u64,
    /// Words written over each box, such as a reason code.
    overlay: Entity<TextInput>,
    /// The properties, scripts and attached files go too.
    scrub: bool,
    /// A box being dragged out on the page shown.
    drawing: Option<Draft>,
    /// The name of the copy, while Apply asks to be sure.
    confirm: Option<Entity<TextInput>>,
    working: bool,
    _inputs: Vec<Subscription>,
}

impl Viewer {
    /// Takes the redact tool in hand, or puts it down.
    pub(super) fn toggle_redacting(&mut self, cx: &mut Context<Self>) {
        if self.redacting.take().is_some() {
            cx.notify();
            return;
        }
        self.pick_adding(None, cx);
        self.marking = None;
        self.properties = None;
        self.designing = None;
        self.linking = None;
        self.numbering = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let words = cx.new(|cx| {
            let mut input = TextInput::new(t("redact-words", &[]), cx);
            input.set_accent(accent);
            input
        });
        let overlay = cx.new(|cx| {
            let mut input = TextInput::new(t("redact-overlay", &[]), cx);
            input.set_accent(accent);
            input
        });
        let inputs = vec![cx.subscribe(&words, |this, _, event, cx| match event {
            InputEvent::Submit => this.find_words(0, cx),
            InputEvent::Changed => this.find_words(400, cx),
            InputEvent::Cancel => {}
        })];
        self.redacting = Some(Redacting {
            areas: Vec::new(),
            find: [false; 4],
            found: Default::default(),
            words,
            words_found: None,
            asked: 0,
            overlay,
            scrub: true,
            drawing: None,
            confirm: None,
            working: false,
            _inputs: inputs,
        });
        self.count_redactions(cx);
        cx.notify();
    }

    /// A press on the pages with the redact tool in hand: a click on a
    /// box marked unmarks it; otherwise a box starts being dragged out.
    /// Returns whether the tool is in hand.
    pub(super) fn redact_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.redacting.is_none() {
            return false;
        }
        let Some((page, point)) = self.page_point(position) else {
            return true;
        };
        let hit = self.inverse(page).and_then(|inv| {
            let [x, y] = inv.map(point);
            let redacting = self.redacting.as_ref()?;
            redacting.areas.iter().rposition(|a| {
                let r = a.rect;
                a.page == page && x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
            })
        });
        let Some(redacting) = self.redacting.as_mut() else {
            return true;
        };
        match hit {
            Some(i) => {
                redacting.areas.remove(i);
            }
            None => {
                redacting.drawing = Some(Draft {
                    page,
                    path: vec![point],
                });
            }
        }
        cx.notify();
        true
    }

    pub(super) fn redact_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self
            .redacting
            .as_ref()
            .and_then(|r| r.drawing.as_ref())
            .map(|d| d.page)
        else {
            return false;
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            ((position.x - b.left()) / shown).clamp(0.0, (b.size.width / shown).max(0.0)),
            ((position.y - b.top()) / shown).clamp(0.0, (b.size.height / shown).max(0.0)),
        ];
        if let Some(draft) = self.redacting.as_mut().and_then(|r| r.drawing.as_mut()) {
            draft.path.truncate(1);
            draft.path.push(point);
        }
        cx.notify();
        true
    }

    /// The pointer let go: the box dragged out is marked.
    pub(super) fn redact_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(draft) = self.redacting.as_mut().and_then(|r| r.drawing.take()) else {
            return false;
        };
        cx.notify();
        let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last()) else {
            return true;
        };
        if (to[0] - from[0]).abs() < CLICK || (to[1] - from[1]).abs() < CLICK {
            return true;
        }
        let Some(inv) = self.inverse(draft.page) else {
            return true;
        };
        let rect = inv.rect([
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]),
            from[1].max(to[1]),
        ]);
        if let Some(redacting) = self.redacting.as_mut() {
            redacting.areas.push(Area {
                page: draft.page,
                rect,
                said: String::new(),
                from: From::Drawn,
            });
        }
        true
    }

    /// The file as it is now, with its password, to look in.
    fn redact_source(&self) -> Option<Source> {
        Some((self.unsaved(), self.path.clone()?, self.key.clone()))
    }

    /// Looks for each kind of number in the whole file, so the panel
    /// can say how many there are before any is marked.
    fn count_redactions(&mut self, cx: &mut Context<Self>) {
        let Some((current, path, key)) = self.redact_source() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    let mut found: [Found; 4] = Default::default();
                    for page in 0..doc.pages().len() {
                        for (i, (_, pattern)) in FINDS.iter().enumerate() {
                            for (rect, said) in
                                doc.find_redactions_said(page, std::slice::from_ref(pattern))
                            {
                                found[i].push((page, rect, said));
                            }
                        }
                    }
                    Ok::<_, String>(found)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(redacting) = this.redacting.as_mut() else {
                    return;
                };
                match found {
                    Ok(found) => redacting.found = found.map(Some),
                    Err(error) => {
                        this.notice = Some(t("redact-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Marks, or unmarks, every place kind `i` of [`FINDS`] was found.
    fn toggle_find(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(redacting) = self.redacting.as_mut() else {
            return;
        };
        let Some(found) = redacting.found[i].clone() else {
            return;
        };
        redacting.find[i] = !redacting.find[i];
        if redacting.find[i] {
            for (page, rect, said) in found {
                if !redacting
                    .areas
                    .iter()
                    .any(|a| a.page == page && a.rect == rect)
                {
                    redacting.areas.push(Area {
                        page,
                        rect,
                        said,
                        from: From::Kind(i),
                    });
                }
            }
        } else {
            redacting.areas.retain(|a| a.from != From::Kind(i));
        }
        cx.notify();
    }

    /// Marks every place the words typed are written, in place of what
    /// earlier words marked, `wait` milliseconds after the last key.
    fn find_words(&mut self, wait: u64, cx: &mut Context<Self>) {
        let Some((current, path, key)) = self.redact_source() else {
            return;
        };
        let Some(redacting) = self.redacting.as_mut() else {
            return;
        };
        let words = redacting.words.read(cx).text().trim().to_owned();
        redacting.asked += 1;
        let asked = redacting.asked;
        if words.chars().count() < 2 {
            redacting.areas.retain(|a| a.from != From::Words);
            redacting.words_found = None;
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(wait))
                .await;
            let still = this
                .update(cx, |this, _| {
                    this.redacting.as_ref().is_some_and(|r| r.asked == asked)
                })
                .unwrap_or(false);
            if !still {
                return;
            }
            let found = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    let pattern = [Pattern::Words(words)];
                    let mut found = Found::new();
                    for page in 0..doc.pages().len() {
                        for (rect, said) in doc.find_redactions_said(page, &pattern) {
                            found.push((page, rect, said));
                        }
                    }
                    Ok::<_, String>(found)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(redacting) = this.redacting.as_mut().filter(|r| r.asked == asked) else {
                    return;
                };
                match found {
                    Ok(found) => {
                        redacting.areas.retain(|a| a.from != From::Words);
                        redacting.words_found = Some(found.len());
                        for (page, rect, said) in found {
                            redacting.areas.push(Area {
                                page,
                                rect,
                                said,
                                from: From::Words,
                            });
                        }
                    }
                    Err(error) => {
                        this.notice = Some(t("redact-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Apply was pressed: asks to be sure, with the copy's name.
    fn confirm_redactions(&mut self, cx: &mut Context<Self>) {
        let name = self
            .path
            .as_deref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(redacting) = self.redacting.as_mut() else {
            return;
        };
        if redacting.areas.is_empty() && !redacting.scrub {
            return;
        }
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(String::new(), cx);
            input.set_accent(accent);
            input.set_text(t("redact-copy-name", &[("name", &name)]), cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.apply_redactions(cx);
            }
        });
        if let Some(redacting) = self.redacting.as_mut() {
            redacting.confirm = Some(input);
            redacting._inputs.push(sub);
        }
        cx.notify();
    }

    /// Takes what lies under the areas marked out of a new copy of the
    /// file, and opens the copy.
    fn apply_redactions(&mut self, cx: &mut Context<Self>) {
        let Some((current, path, key)) = self.redact_source() else {
            return;
        };
        let Some(redacting) = self.redacting.as_mut() else {
            return;
        };
        let Some(name) = redacting.confirm.as_ref() else {
            return;
        };
        if redacting.working {
            return;
        }
        let mut name = name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return;
        }
        if !name.to_ascii_lowercase().ends_with(".pdf") {
            name.push_str(".pdf");
        }
        let target = path.with_file_name(&name);
        if target == path || target.exists() {
            self.notice = Some(t("redact-copy-there", &[("name", &name)]));
            cx.notify();
            return;
        }
        let areas: Vec<(usize, [f32; 4])> =
            redacting.areas.iter().map(|a| (a.page, a.rect)).collect();
        let overlay = redacting.overlay.read(cx).text().trim().to_owned();
        let options = RedactOptions {
            fill: Some([0.0; 3]),
            scrub: redacting.scrub,
            overlay: (!overlay.is_empty()).then_some(overlay),
        };
        redacting.working = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    let out = doc.redact(&areas, &options).map_err(|e| e.to_string())?;
                    std::fs::write(&target, out).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(target) => {
                        this.redacting = None;
                        this.notice =
                            Some(t("redact-saved", &[("name", &super::file_name(&target))]));
                        cx.emit(super::ViewerEvent::Dropped(vec![target]));
                    }
                    Err(error) => {
                        if let Some(r) = this.redacting.as_mut() {
                            r.working = false;
                        }
                        this.notice = Some(t("redact-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The areas marked on page `page`, and the box being dragged out.
    pub(super) fn redact_overlay(&self, page: usize) -> Vec<AnyElement> {
        let Some(redacting) = self.redacting.as_ref() else {
            return Vec::new();
        };
        let shown = self.shown;
        let red = 0xd0_20_20_00u32;
        let area = |b: [f32; 4]| {
            div()
                .absolute()
                .left(desktop_px(b[0] * shown))
                .top(desktop_px(b[1] * shown))
                .w(desktop_px((b[2] - b[0]) * shown))
                .h(desktop_px((b[3] - b[1]) * shown))
                .border_2()
                .border_color(gpui::rgba(red | 0xff))
                .bg(gpui::rgba(red | 0x40))
        };
        let mut out = Vec::new();
        if let Some(transform) = self.redact_transform(page) {
            for a in &redacting.areas {
                if a.page == page {
                    let b = katna_pdf_render::map_rect(transform, a.rect);
                    out.push(area(b).into_any_element());
                }
            }
        }
        if let Some(draft) = redacting.drawing.as_ref().filter(|d| d.page == page)
            && let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last())
        {
            let b = [
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ];
            out.push(area(b).border_dashed().into_any_element());
        }
        out
    }

    fn redact_transform(&self, page: usize) -> Option<[f64; 6]> {
        match &self.content {
            Content::Ready(opened) => opened.doc.page_transform(page),
            _ => None,
        }
    }

    /// The redact panel (E10), while the tool is in hand: what to find,
    /// what is marked, what goes over the holes.
    pub(super) fn redact_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let redacting = self.redacting.as_ref()?;
        let th = self.theme;
        let heading = |words: String| {
            div()
                .pt(px(space::S3))
                .pb(px(space::S1))
                .text_size(px(text::MICRO))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.dim)
                .child(words.to_uppercase())
        };
        let count_of = |n: Option<usize>| {
            div()
                .flex_none()
                .text_size(px(text::CAPTION))
                .text_color(th.dim)
                .child(n.map_or_else(|| "…".to_owned(), |n| n.to_string()))
        };
        let mut finds = div().flex().flex_col();
        for (i, (id, _)) in FINDS.iter().enumerate() {
            let on = redacting.find[i];
            let n = redacting.found[i].as_ref().map(Vec::len);
            finds = finds.child(
                div()
                    .id(("redact-find", i))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .when(n == Some(0), |d| d.opacity(0.6))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_find(i, cx)))
                    .child(check_box(on, th))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t(id, &[])),
                    )
                    .child(count_of(n)),
            );
        }
        let words = float::input_box(&redacting.words, window, cx, &th)
            .gap(px(space::S2))
            .child(div().flex_1())
            .children(redacting.words_found.map(|n| count_of(Some(n))));
        let words = div().relative().child(words.pl(px(32.0))).child(
            div()
                .absolute()
                .left(px(space::S3))
                .top(px(10.0))
                .child(float::icon("search", 15.0, th.dim)),
        );
        let count = redacting.areas.len();
        let mut marked = div().flex().flex_col();
        for (i, area) in redacting.areas.iter().enumerate() {
            let said = if area.said.is_empty() {
                t("redact-area-drawn", &[])
            } else {
                area.said.clone()
            };
            let page = self.page_label(area.page);
            marked = marked.child(
                div()
                    .h(px(26.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .w(px(36.0))
                            .flex_none()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(t("redact-page", &[("page", &page)])),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(said),
                    )
                    .child(
                        float::sized_ib(
                            ("redact-unmark", i),
                            "x",
                            22.0,
                            12.0,
                            t("redact-unmark", &[]),
                            false,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(r) = this.redacting.as_mut()
                                && i < r.areas.len()
                            {
                                let gone = r.areas.remove(i);
                                // A kind with none left marked is unticked.
                                if let From::Kind(k) = gone.from
                                    && !r.areas.iter().any(|a| a.from == From::Kind(k))
                                {
                                    r.find[k] = false;
                                }
                            }
                            cx.notify();
                        })),
                    ),
            );
        }
        if count == 0 {
            marked = marked.child(
                div()
                    .py(px(space::S1))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("redact-none", &[])),
            );
        }
        let scrub = redacting.scrub;
        let ready = (count > 0 || scrub) && !redacting.working;
        let body = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.text2)
                    .child(t("redact-hint", &[])),
            )
            .child(heading(t("redact-find-label", &[])))
            .child(finds)
            .child(div().pt(px(space::S2)).child(words))
            .child(heading(t(
                "redact-marked",
                &[("count", &count.to_string())],
            )))
            .child(marked)
            .child(heading(t("redact-overlay-label", &[])))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .size(px(20.0))
                            .flex_none()
                            .rounded_full()
                            .bg(gpui::black()),
                    )
                    .child(div().flex_1().child(float::input_box(
                        &redacting.overlay,
                        window,
                        cx,
                        &th,
                    ))),
            )
            .child(
                div()
                    .id("redact-scrub")
                    .pt(px(space::S3))
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(r) = this.redacting.as_mut() {
                            r.scrub = !r.scrub;
                        }
                        cx.notify();
                    }))
                    .child(check_box(scrub, th))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("redact-scrub", &[])),
                    ),
            );
        let apply = if count == 0 {
            t("redact-apply", &[])
        } else {
            t("redact-apply-n", &[("count", &count.to_string())])
        };
        Some(
            div()
                .id("redact-card")
                .absolute()
                .top(px(space::S3))
                .bottom(px(space::S3))
                .right(px(space::S3))
                .w(px(300.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(12.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(space::S4))
                        .pt(px(space::S4))
                        .pb(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(text::SUBTITLE))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(th.text)
                                .child(t("redact-title", &[])),
                        )
                        .child(
                            float::sized_ib(
                                "redact-close",
                                "x",
                                28.0,
                                14.0,
                                t("redact-cancel", &[]),
                                false,
                                &th,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.redacting = None;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .id("redact-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .px(px(space::S4))
                        .pb(px(space::S3))
                        .child(body),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .px(px(space::S4))
                        .py(px(space::S3))
                        .border_t_1()
                        .border_color(th.line)
                        .child(
                            float::button("redact-apply", apply, true, &th)
                                .bg(th.error)
                                .border_color(th.error)
                                .text_color(gpui::white())
                                .when(!ready, |b| b.opacity(0.5))
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.confirm_redactions(cx)),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Asks to be sure before redacting (E11), with the copy's name.
    pub(super) fn redact_confirm_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let redacting = self.redacting.as_ref()?;
        let name = redacting.confirm.as_ref()?;
        let th = self.theme;
        let count = redacting.areas.len();
        let title = if count == 0 {
            t("redact-confirm-clean", &[])
        } else {
            t("redact-confirm-title", &[("count", &count.to_string())])
        };
        Some(
            div()
                .id("redact-confirm")
                .w(px(380.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(12.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .px(px(space::S4))
                        .pt(px(space::S4))
                        .flex()
                        .flex_col()
                        .gap(px(space::S3))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S3))
                                .child(div().w(px(18.0)).h(px(10.0)).rounded(px(2.0)).bg(th.error))
                                .child(
                                    div()
                                        .text_size(px(text::SUBTITLE))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(th.text)
                                        .child(title),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(th.text2)
                                .child(t("redact-confirm-detail", &[])),
                        )
                        .child(
                            div()
                                .relative()
                                .child(float::input_box(name, window, cx, &th).pl(px(32.0)))
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(space::S3))
                                        .top(px(10.0))
                                        .child(float::icon("file", 15.0, th.dim)),
                                ),
                        ),
                )
                .child(
                    div()
                        .mt(px(space::S4))
                        .flex()
                        .justify_end()
                        .gap(px(space::S2))
                        .px(px(space::S4))
                        .py(px(space::S3))
                        .border_t_1()
                        .border_color(th.line)
                        .child(
                            float::button("redact-back", t("redact-cancel", &[]), false, &th)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(r) = this.redacting.as_mut() {
                                        r.confirm = None;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            float::button(
                                "redact-go",
                                t(
                                    if redacting.working {
                                        "redact-working"
                                    } else {
                                        "redact-confirm-go"
                                    },
                                    &[],
                                ),
                                true,
                                &th,
                            )
                            .bg(th.error)
                            .border_color(th.error)
                            .text_color(gpui::white())
                            .on_click(cx.listener(|this, _, _, cx| this.apply_redactions(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Closes the confirm card, if it is open, and says whether it was.
    pub(super) fn close_redact_confirm(&mut self) -> bool {
        self.redacting
            .as_mut()
            .and_then(|r| r.confirm.take())
            .is_some()
    }
}
