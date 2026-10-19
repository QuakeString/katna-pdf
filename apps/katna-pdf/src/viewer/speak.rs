// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading aloud (docs/PLAN.md §4.1, P1): the text selected, or the pages
//! from the one shown to the end, read by the desktop's voice
//! ([`crate::speech`]) a sentence at a time, each sentence marked on the
//! page as it is read and the pages turned to follow it.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, Context, div, prelude::*};
use katna_pdf_render::text::PageText;
use katna_ui::px;
use katna_ui::tokens::{space, text};
use katna_ui::widgets;

use super::{Content, Viewer, icon_button};
use crate::speech;
use crate::text::t;

/// Reading aloud.
pub(super) struct Reading {
    /// Set to end this reading.
    stop: Arc<AtomicBool>,
    /// The page and sentence being read, its characters, and the page's
    /// text to mark it in.
    pub(super) at: Option<(usize, Range<usize>, Arc<PageText>)>,
    /// What is left to read: each page and, for a selection, the part
    /// of it.
    plan: Vec<(usize, Option<Range<usize>>)>,
    /// The plan's step and the sentence being read, to go on from.
    place: (usize, usize),
    /// Paused by the person.
    held: bool,
}

impl Viewer {
    /// Reads aloud the text selected, or from the page shown; stops if
    /// reading already.
    pub(super) fn toggle_reading(&mut self, cx: &mut Context<Self>) {
        if self.reading.is_some() {
            self.stop_reading(cx);
            return;
        }
        let count = self.page_count();
        if count == 0 {
            return;
        }
        let plan: Vec<(usize, Option<Range<usize>>)> = match (self.selection, self.texts.as_ref()) {
            (Some(selection), Some(texts)) if selection.anchor != selection.head => selection
                .pages()
                .filter_map(|page| {
                    let text = texts.get(page)?.get()?;
                    Some((page, Some(selection.on_page(page, text.boxes.len())?)))
                })
                .collect(),
            _ => (self.current_page()..count).map(|p| (p, None)).collect(),
        };
        self.reading = Some(Reading {
            stop: Arc::default(),
            at: None,
            plan,
            place: (0, 0),
            held: false,
        });
        self.read_from(0, 0, cx);
    }

    /// Reads the plan from step `step`, sentence `sentence`.
    fn read_from(&mut self, step: usize, sentence: usize, cx: &mut Context<Self>) {
        let (Content::Ready(opened), Some(reading)) = (&self.content, self.reading.as_mut()) else {
            return;
        };
        let doc = opened.doc.clone();
        let stop = Arc::new(AtomicBool::new(false));
        reading.stop = stop.clone();
        reading.place = (step, sentence);
        reading.held = false;
        let plan = reading.plan.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let voice = cx
                .background_executor()
                .spawn(async { speech::voice() })
                .await;
            let Some(voice) = voice else {
                this.update(cx, |this, cx| {
                    this.reading = None;
                    this.notice = Some(t("read-no-voice", &[]));
                    cx.notify();
                })
                .ok();
                return;
            };
            for (k, (page, part)) in plan.iter().enumerate().skip(step) {
                let doc = doc.clone();
                let page = *page;
                let text = Arc::new(
                    cx.background_executor()
                        .spawn(async move { doc.text(page) })
                        .await,
                );
                let all = speech::sentences(&text.text);
                let first = if k == step { sentence } else { 0 };
                for (i, range) in all.into_iter().enumerate().skip(first) {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    // In a selection, only the sentences it reaches, cut
                    // to it.
                    let range = match part {
                        Some(part) if range.end <= part.start || range.start >= part.end => {
                            continue;
                        }
                        Some(part) => range.start.max(part.start)..range.end.min(part.end),
                        None => range,
                    };
                    // Nothing to say in a row of dots or a lone number's
                    // stop.
                    let words = speech::spoken(&text.text, range.clone());
                    if !words.chars().any(char::is_alphabetic) {
                        continue;
                    }
                    let going = this
                        .update(cx, |this, cx| {
                            let Some(reading) = this.reading.as_mut() else {
                                return false;
                            };
                            reading.at = Some((page, range.clone(), text.clone()));
                            reading.place = (k, i);
                            if !this.visible.contains(&page) {
                                this.go_to_page(page, cx);
                            }
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !going {
                        return;
                    }
                    let said = cx
                        .background_executor()
                        .spawn(async move { voice.say(&words) })
                        .await;
                    if said.is_err() {
                        this.update(cx, |this, cx| {
                            this.reading = None;
                            this.notice = Some(t("read-voice-gone", &[]));
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                }
            }
            this.update(cx, |this, cx| {
                if !stop.load(Ordering::Relaxed) {
                    this.reading = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stops reading, keeping the place to go on from.
    fn pause_reading(&mut self, cx: &mut Context<Self>) {
        let Some(reading) = self.reading.as_mut() else {
            return;
        };
        // The sentence being read is read again from its start.
        reading.stop.store(true, Ordering::Relaxed);
        reading.held = true;
        stop_voice();
        cx.notify();
    }

    fn resume_reading(&mut self, cx: &mut Context<Self>) {
        let Some((step, sentence)) = self.reading.as_ref().map(|r| r.place) else {
            return;
        };
        self.read_from(step, sentence, cx);
    }

    pub(super) fn stop_reading(&mut self, cx: &mut Context<Self>) {
        self.end_reading();
        cx.notify();
    }

    /// Stops reading, when the file closes or another opens.
    pub(super) fn end_reading(&mut self) {
        if let Some(reading) = self.reading.take() {
            reading.stop.store(true, Ordering::Relaxed);
            stop_voice();
        }
    }

    /// The sentence being read, as boxes on page `page`.
    pub(super) fn reading_marks(&self, page: usize) -> Vec<[f32; 4]> {
        match self.reading.as_ref().and_then(|r| r.at.as_ref()) {
            Some((p, range, text)) if *p == page && range.end <= text.boxes.len() => {
                text.line_boxes(range.clone())
            }
            _ => Vec::new(),
        }
    }

    /// The bar under the toolbar while reading aloud.
    pub(super) fn reading_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reading = self.reading.as_ref()?;
        let th = self.theme;
        let status = match &reading.at {
            _ if reading.held => t("read-paused", &[]),
            Some((page, ..)) => t("read-page", &[("page", &(page + 1).to_string())]),
            None => t("read-starting", &[]),
        };
        Some(
            div()
                .h(px(44.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S4))
                .border_b_1()
                .border_color(th.line)
                .child(widgets::icon("speaker", th.katna.text_dim, 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(status),
                )
                .child(if reading.held {
                    icon_button(
                        "read-resume",
                        "play",
                        t("read-resume", &[]),
                        th,
                        cx.listener(|this, _, _, cx| this.resume_reading(cx)),
                    )
                    .into_any_element()
                } else {
                    icon_button(
                        "read-pause",
                        "pause",
                        t("read-pause", &[]),
                        th,
                        cx.listener(|this, _, _, cx| this.pause_reading(cx)),
                    )
                    .into_any_element()
                })
                .child(icon_button(
                    "read-stop",
                    "close",
                    t("read-stop", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.stop_reading(cx)),
                ))
                .into_any_element(),
        )
    }
}

/// Stops the voice mid-sentence, off the UI thread.
fn stop_voice() {
    std::thread::spawn(|| {
        if let Some(voice) = speech::voice() {
            voice.stop();
        }
    });
}
