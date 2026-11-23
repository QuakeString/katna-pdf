// SPDX-License-Identifier: GPL-3.0-or-later

//! Page numbers (docs/PLAN.md §4.3, page labels): the labels a file gives
//! its pages, such as i, ii for the front matter and 1, 2 after it, shown
//! beside the page count and under the thumbnails, and a card, opened by
//! clicking the page count, to number the pages in ranges
//! (`katna_pdf_cos::labels`).

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{LabelRange, LabelStyle};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::Tip;

use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// The styles, in the order a click on the style goes through them, with
/// how each is shown.
const STYLES: [(LabelStyle, &str); 6] = [
    (LabelStyle::Decimal, "numbering-decimal"),
    (LabelStyle::LowerRoman, "numbering-lower-roman"),
    (LabelStyle::UpperRoman, "numbering-upper-roman"),
    (LabelStyle::LowerLetters, "numbering-lower-letters"),
    (LabelStyle::UpperLetters, "numbering-upper-letters"),
    (LabelStyle::None, "numbering-none"),
];

/// One range's row: its first page, style, prefix and first number.
struct Row {
    from: Entity<TextInput>,
    style: LabelStyle,
    prefix: Entity<TextInput>,
    first: Entity<TextInput>,
}

/// The page numbers card.
pub(super) struct Numbering {
    rows: Vec<Row>,
    /// The row being changed.
    editing: usize,
    _inputs: Vec<Subscription>,
}

impl Viewer {
    /// The file's numbering, as read.
    pub(super) fn set_label_ranges(&mut self, ranges: Vec<LabelRange>) {
        self.label_ranges = ranges;
    }

    /// Page `page`'s label: the file's own, or its number.
    pub(super) fn page_label(&self, page: usize) -> String {
        match self.label_ranges.iter().rev().find(|r| r.start <= page) {
            Some(range) => range.label(page - range.start),
            None => (page + 1).to_string(),
        }
    }

    /// Opens the page numbers card, or closes it.
    pub(super) fn toggle_numbering(&mut self, cx: &mut Context<Self>) {
        if self.numbering.take().is_some() {
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        self.marking = None;
        self.redacting = None;
        self.linking = None;
        self.designing = None;
        self.properties = None;
        let mut numbering = Numbering {
            rows: Vec::new(),
            editing: 0,
            _inputs: Vec::new(),
        };
        let ranges = if self.label_ranges.is_empty() {
            vec![LabelRange {
                start: 0,
                style: LabelStyle::Decimal,
                prefix: String::new(),
                first: 1,
            }]
        } else {
            self.label_ranges.clone()
        };
        for range in ranges {
            self.add_row(&mut numbering, range, cx);
        }
        self.numbering = Some(numbering);
        cx.notify();
    }

    fn add_row(&self, numbering: &mut Numbering, range: LabelRange, cx: &mut Context<Self>) {
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = |text: String, placeholder: &str, cx: &mut Context<Self>| {
            let placeholder = t(placeholder, &[]);
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input.set_text(text, cx);
                input
            })
        };
        let row = Row {
            from: input((range.start + 1).to_string(), "numbering-from", cx),
            style: range.style,
            prefix: input(range.prefix, "numbering-prefix", cx),
            first: input(range.first.to_string(), "numbering-first", cx),
        };
        for field in [&row.from, &row.prefix, &row.first] {
            numbering
                ._inputs
                .push(cx.subscribe(field, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.apply_numbering(cx);
                    }
                }));
        }
        numbering.rows.push(row);
    }

    /// The ranges the card says, or the message saying what is wrong.
    fn numbering_ranges(&self, cx: &Context<Self>) -> Result<Vec<LabelRange>, String> {
        let Some(numbering) = self.numbering.as_ref() else {
            return Ok(Vec::new());
        };
        let count = self.page_count();
        let mut out = Vec::new();
        for row in &numbering.rows {
            let from = row.from.read(cx).text().trim().to_owned();
            let start = from
                .parse::<usize>()
                .ok()
                .filter(|p| (1..=count).contains(p))
                .ok_or_else(|| t("numbering-bad-page", &[("count", &count.to_string())]))?;
            let first = row.first.read(cx).text().trim().to_owned();
            let first = if first.is_empty() {
                1
            } else {
                first
                    .parse::<u32>()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| t("numbering-bad-first", &[]))?
            };
            out.push(LabelRange {
                start: start - 1,
                style: row.style,
                prefix: row.prefix.read(cx).text().to_owned(),
                first,
            });
        }
        out.sort_by_key(|r| r.start);
        out.dedup_by_key(|r| r.start);
        Ok(out)
    }

    /// Numbers the pages as the card says.
    fn apply_numbering(&mut self, cx: &mut Context<Self>) {
        let ranges = match self.numbering_ranges(cx) {
            Ok(ranges) => ranges,
            Err(message) => {
                self.notice = Some(message);
                cx.notify();
                return;
            }
        };
        // Plain 1, 2, 3 from the first page is no numbering at all.
        let plain = ranges.len() == 1
            && ranges[0]
                == LabelRange {
                    start: 0,
                    style: LabelStyle::Decimal,
                    prefix: String::new(),
                    first: 1,
                };
        let ranges = if plain { Vec::new() } else { ranges };
        self.numbering = None;
        self.label_ranges = ranges.clone();
        self.change_document(
            Vec::new(),
            "numbering-failed",
            move |doc| doc.set_page_labels(&ranges).map_err(|e| e.to_string()),
            cx,
        );
        cx.notify();
    }

    /// The page numbers dialog (R17): the ranges as a list, each
    /// with its pages, style and a way to take it out; the one picked
    /// is changed under the list; then Add a range, and Use these.
    pub(super) fn numbering_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let numbering = self.numbering.as_ref()?;
        let th = self.theme;
        let count = self.page_count();
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        // Where each row's pages end: before the next row's start.
        let starts: Vec<Option<usize>> = numbering
            .rows
            .iter()
            .map(|r| {
                r.from
                    .read(cx)
                    .text()
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|p| (1..=count).contains(p))
            })
            .collect();
        let mut list = div()
            .flex()
            .flex_col()
            .rounded(px(radius::SM))
            .border_1()
            .border_color(th.line)
            .overflow_hidden();
        for (i, row) in numbering.rows.iter().enumerate() {
            let from = starts[i].unwrap_or(1);
            let to = starts
                .iter()
                .filter_map(|s| *s)
                .filter(|s| *s > from)
                .min()
                .map_or(count, |next| next - 1);
            let pages = if from == to {
                t("numbering-page-one", &[("page", &from.to_string())])
            } else {
                t(
                    "numbering-pages",
                    &[("from", &from.to_string()), ("to", &to.to_string())],
                )
            };
            let style = STYLES
                .iter()
                .find(|(s, _)| *s == row.style)
                .map_or("numbering-none", |(_, id)| id);
            let range = LabelRange {
                start: 0,
                style: row.style,
                prefix: row.prefix.read(cx).text().to_owned(),
                first: row.first.read(cx).text().trim().parse().unwrap_or(1),
            };
            let shown = if row.style == LabelStyle::None && range.prefix.is_empty() {
                t("numbering-none", &[])
            } else if from == to {
                range.label(0)
            } else {
                format!(
                    "{}–{}",
                    range.label(0),
                    range.label(to.saturating_sub(from))
                )
            };
            let picked = numbering.editing == i;
            list = list.child(
                div()
                    .id(("numbering-row", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .when(i > 0, |d| d.border_t_1().border_color(th.line))
                    .cursor_pointer()
                    .map(|d| {
                        if picked {
                            d.bg(th.accent_soft)
                        } else {
                            d.hover(move |s| s.bg(th.hover))
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(n) = this.numbering.as_mut() {
                            n.editing = i;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(96.0))
                            .flex_none()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(pages),
                    )
                    .child(float::chip(("numbering-chip", i), t(style, &[]), true, &th))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(shown),
                    )
                    .when(numbering.rows.len() > 1, |d| {
                        d.child(
                            float::sized_ib(
                                ("numbering-remove", i),
                                "trash",
                                26.0,
                                14.0,
                                t("numbering-remove", &[]),
                                false,
                                &th,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if let Some(n) = this.numbering.as_mut()
                                        && i < n.rows.len()
                                    {
                                        n.rows.remove(i);
                                        n.editing = n.editing.min(n.rows.len() - 1);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                    }),
            );
        }
        // The row picked, to change.
        let editor = numbering.rows.get(numbering.editing).map(|row| {
            let i = numbering.editing;
            let mut styles = float::seg(&th);
            for (j, (style, id)) in STYLES.into_iter().enumerate() {
                styles = styles.child(
                    float::seg_item(
                        ("numbering-style", j),
                        None,
                        Some(t(&format!("{id}-short"), &[]).into()),
                        row.style == style,
                        &th,
                    )
                    .tip(t(id, &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(row) = this.numbering.as_mut().and_then(|n| n.rows.get_mut(i)) {
                            row.style = style;
                        }
                        cx.notify();
                    })),
                );
            }
            let column = |id: &str, el: AnyElement, width: Option<f32>| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .map(|d| match width {
                        Some(w) => d.w(px(w)).flex_none(),
                        None => d.flex_1().min_w_0(),
                    })
                    .child(label(id))
                    .child(el)
            };
            div()
                .flex()
                .items_end()
                .gap(px(space::S3))
                .child(column(
                    "numbering-from-label",
                    float::input_box(&row.from, window, cx, &th).into_any_element(),
                    Some(72.0),
                ))
                .child(column("numbering-style", styles.into_any_element(), None))
                .child(column(
                    "numbering-prefix-label",
                    float::input_box(&row.prefix, window, cx, &th).into_any_element(),
                    Some(80.0),
                ))
                .child(column(
                    "numbering-first-label",
                    float::input_box(&row.first, window, cx, &th).into_any_element(),
                    Some(56.0),
                ))
        });
        let body = div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("numbering-hint", &[])),
            )
            .child(list)
            .children(editor)
            .child(
                div().flex().child(
                    float::button("numbering-add", t("numbering-add", &[]), false, &th)
                        .child(float::icon("plus", 14.0, th.text))
                        .flex_row_reverse()
                        .on_click(cx.listener(|this, _, _, cx| {
                            // At the page shown, or after the last range.
                            let last = this
                                .numbering_ranges(cx)
                                .ok()
                                .and_then(|r| r.last().map(|r| r.start + 1))
                                .unwrap_or(0);
                            let start = this
                                .current_page()
                                .max(last)
                                .min(this.page_count().saturating_sub(1));
                            if let Some(mut numbering) = this.numbering.take() {
                                let range = LabelRange {
                                    start,
                                    style: LabelStyle::Decimal,
                                    prefix: String::new(),
                                    first: 1,
                                };
                                this.add_row(&mut numbering, range, cx);
                                numbering.editing = numbering.rows.len() - 1;
                                this.numbering = Some(numbering);
                            }
                            cx.notify();
                        })),
                ),
            );
        let foot = div()
            .flex()
            .gap(px(space::S2))
            .child(
                float::button("numbering-cancel", t("numbering-cancel", &[]), false, &th).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.numbering = None;
                        cx.notify();
                    }),
                ),
            )
            .child(
                float::button("numbering-apply", t("numbering-apply", &[]), true, &th)
                    .on_click(cx.listener(|this, _, _, cx| this.apply_numbering(cx))),
            );
        Some(
            float::dialog_card(
                "numbering-card",
                t("numbering-title", &[]),
                None,
                520.0,
                body,
                foot,
                &th,
            )
            .into_any_element(),
        )
    }
}
