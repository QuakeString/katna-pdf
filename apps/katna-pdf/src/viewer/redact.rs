// SPDX-License-Identifier: GPL-3.0-or-later

//! Redacting (docs/PLAN.md §4.3, Phase 4): with the redact tool from the
//! edit row, boxes are dragged over what must go, or found from words,
//! PAN and Aadhaar numbers, phone numbers and email addresses; Redact
//! then takes what lies under them out of the file itself
//! (`katna_pdf_cos::redact`), not only out of sight, and draws a black
//! box over each.

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{Pattern, RedactOptions};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::annots::Draft;
use super::sign::check_box;
use super::{Content, Viewer};
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// The kinds of number that can be found, and their names in the card.
const FINDS: [(&str, Pattern); 4] = [
    ("redact-find-pan", Pattern::Pan),
    ("redact-find-aadhaar", Pattern::Aadhaar),
    ("redact-find-phone", Pattern::Phone),
    ("redact-find-email", Pattern::Email),
];

/// The redact tool, while in hand.
pub(super) struct Redacting {
    /// The areas marked: a page, and a box in its own space.
    areas: Vec<(usize, [f32; 4])>,
    /// Which of [`FINDS`] to look for.
    find: [bool; 4],
    words: Entity<TextInput>,
    /// The properties, scripts and attached files go too.
    scrub: bool,
    /// A box being dragged out on the page shown.
    drawing: Option<Draft>,
    /// Looking for text to mark.
    finding: bool,
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
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let words = cx.new(|cx| {
            let mut input = TextInput::new(t("redact-words", &[]), cx);
            input.set_accent(accent);
            input
        });
        let inputs = vec![cx.subscribe(&words, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.find_redactions(cx);
            }
        })];
        self.redacting = Some(Redacting {
            areas: Vec::new(),
            find: [false; 4],
            words,
            scrub: false,
            drawing: None,
            finding: false,
            _inputs: inputs,
        });
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
            redacting
                .areas
                .iter()
                .rposition(|(p, r)| *p == page && x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3])
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
            redacting.areas.push((draft.page, rect));
        }
        true
    }

    /// Marks every place on every page where the words typed, or the
    /// kinds of number picked, are written.
    fn find_redactions(&mut self, cx: &mut Context<Self>) {
        let Some(redacting) = self.redacting.as_mut() else {
            return;
        };
        let mut patterns: Vec<Pattern> = FINDS
            .iter()
            .zip(redacting.find)
            .filter(|(_, on)| *on)
            .map(|((_, pattern), _)| pattern.clone())
            .collect();
        let words = redacting.words.read(cx).text().trim().to_owned();
        if !words.is_empty() {
            patterns.push(Pattern::Words(words));
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        if patterns.is_empty() || redacting.finding {
            return;
        }
        redacting.finding = true;
        cx.notify();
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    let mut found = Vec::new();
                    for page in 0..doc.pages().len() {
                        for rect in doc.find_redactions(page, &patterns) {
                            found.push((page, rect));
                        }
                    }
                    Ok::<_, String>(found)
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(redacting) = this.redacting.as_mut() else {
                    return;
                };
                redacting.finding = false;
                match found {
                    Ok(found) => {
                        let count = found.len().to_string();
                        for area in found {
                            if !redacting.areas.contains(&area) {
                                redacting.areas.push(area);
                            }
                        }
                        this.notice = Some(t("redact-found", &[("count", &count)]));
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

    /// Takes what lies under the areas marked out of the file.
    fn apply_redactions(&mut self, cx: &mut Context<Self>) {
        let Some(redacting) = self.redacting.as_ref() else {
            return;
        };
        if redacting.areas.is_empty() && !redacting.scrub {
            return;
        }
        let areas = redacting.areas.clone();
        let options = RedactOptions {
            fill: Some([0.0; 3]),
            scrub: redacting.scrub,
        };
        self.redacting = None;
        let mut pages: Vec<usize> = areas.iter().map(|(p, _)| *p).collect();
        pages.sort_unstable();
        pages.dedup();
        self.change_document(
            pages,
            "redact-failed",
            move |doc| doc.redact(&areas, &options).map_err(|e| e.to_string()),
            cx,
        );
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
            for (p, rect) in &redacting.areas {
                if *p == page {
                    let b = katna_pdf_render::map_rect(transform, *rect);
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

    /// The redact card, while the tool is in hand.
    pub(super) fn redact_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let redacting = self.redacting.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let mut chips = div().flex().flex_wrap().gap(px(space::S1));
        for (i, (id, _)) in FINDS.iter().enumerate() {
            let on = redacting.find[i];
            chips = chips.child(
                div()
                    .id(("redact-find", i))
                    .h(px(32.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .rounded(px(radius::FULL))
                    .border_1()
                    .border_color(th.line)
                    .text_size(px(text::SMALL))
                    .text_color(th.text)
                    .when(on, |d| d.bg(th.field))
                    .hover(move |s| s.bg(th.hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(r) = this.redacting.as_mut() {
                            r.find[i] = !r.find[i];
                        }
                        cx.notify();
                    }))
                    .child(check_box(on, th))
                    .child(t(id, &[])),
            );
        }
        let count = redacting.areas.len().to_string();
        let scrub = redacting.scrub;
        let ready = !redacting.areas.is_empty() || scrub;
        Some(
            div()
                .id("redact-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(340.0))
                .p(px(space::S4))
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .rounded(px(radius::MD))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .shadow(widgets::elevation(&th.katna, 2.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.text)
                        .child(t("redact-title", &[])),
                )
                .child(label("redact-hint"))
                .child(label("redact-find-label"))
                .child(chips)
                .child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .h(px(36.0))
                                .flex()
                                .items_center()
                                .px(px(space::S3))
                                .rounded(px(radius::SM))
                                .bg(th.field)
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(redacting.words.clone()),
                        )
                        .child(
                            widgets::outlined_button(
                                "redact-mark",
                                t(
                                    if redacting.finding {
                                        "redact-finding"
                                    } else {
                                        "redact-mark"
                                    },
                                    &[],
                                ),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.find_redactions(cx))),
                        ),
                )
                .child(
                    div()
                        .id("redact-scrub")
                        .pt(px(space::S1))
                        .flex()
                        .items_center()
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
                )
                .child(
                    div()
                        .pt(px(space::S2))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child(t("redact-count", &[("count", &count)])),
                        )
                        .child(
                            widgets::outlined_button(
                                "redact-cancel",
                                t("redact-cancel", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.redacting = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            widgets::filled_button(
                                "redact-apply",
                                t("redact-apply", &[]),
                                &th.katna,
                            )
                            .when(!ready, |b| b.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.apply_redactions(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}
