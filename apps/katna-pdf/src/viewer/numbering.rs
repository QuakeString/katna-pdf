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
use katna_ui::widgets::{self, Tip};

use super::{Content, Viewer};
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

    /// The page numbers card, while open.
    pub(super) fn numbering_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let numbering = self.numbering.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let field = |input: &Entity<TextInput>, width: Option<f32>| {
            div()
                .h(px(32.0))
                .flex()
                .items_center()
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::SMALL))
                .text_color(th.text)
                .map(|d| match width {
                    Some(w) => d.w(px(w)).flex_none(),
                    None => d.flex_1().min_w_0(),
                })
                .child(input.clone())
        };
        let mut list = div().flex().flex_col().gap(px(space::S2)).child(
            div()
                .flex()
                .gap(px(space::S2))
                .child(div().w(px(48.0)).child(label("numbering-from-label")))
                .child(div().w(px(84.0)).child(label("numbering-style")))
                .child(div().flex_1().child(label("numbering-prefix-label")))
                .child(div().w(px(48.0)).child(label("numbering-first-label")))
                .child(div().w(px(28.0))),
        );
        for (i, row) in numbering.rows.iter().enumerate() {
            let style = STYLES
                .iter()
                .find(|(s, _)| *s == row.style)
                .map_or("numbering-none", |(_, id)| id);
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(field(&row.from, Some(48.0)))
                    .child(
                        div()
                            .id(("numbering-style", i))
                            .w(px(84.0))
                            .h(px(32.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::FULL))
                            .border_1()
                            .border_color(th.line)
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .hover(move |s| s.bg(th.hover))
                            .cursor_pointer()
                            .tip(t("numbering-style-tip", &[]), &th.katna)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(row) =
                                    this.numbering.as_mut().and_then(|n| n.rows.get_mut(i))
                                {
                                    let at = STYLES
                                        .iter()
                                        .position(|(s, _)| *s == row.style)
                                        .unwrap_or(0);
                                    row.style = STYLES[(at + 1) % STYLES.len()].0;
                                }
                                cx.notify();
                            }))
                            .child(t(style, &[])),
                    )
                    .child(field(&row.prefix, None))
                    .child(field(&row.first, Some(48.0)))
                    .child(
                        widgets::icon_button(("numbering-remove", i), "close", 16.0, &th.katna)
                            .size(px(28.0))
                            .tip(t("numbering-remove", &[]), &th.katna)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(n) = this.numbering.as_mut()
                                    && i < n.rows.len()
                                {
                                    n.rows.remove(i);
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        // What the first pages will be called, as typed now.
        let preview = self.numbering_ranges(cx).ok().map(|ranges| {
            let count = self.page_count();
            let shown: Vec<String> = katna_pdf_cos::labels_of(&ranges, count)
                .into_iter()
                .take(8)
                .collect();
            let more = if count > 8 { ", …" } else { "" };
            t(
                "numbering-preview",
                &[("labels", &format!("{}{more}", shown.join(", ")))],
            )
        });
        Some(
            div()
                .id("numbering-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(400.0))
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
                        .child(t("numbering-title", &[])),
                )
                .child(label("numbering-hint"))
                .child(list)
                .children(preview.map(|p| {
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(p)
                }))
                .child(
                    div()
                        .pt(px(space::S2))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            widgets::outlined_button(
                                "numbering-add",
                                t("numbering-add", &[]),
                                &th.katna,
                            )
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
                                    this.numbering = Some(numbering);
                                }
                                cx.notify();
                            })),
                        )
                        .child(div().flex_1())
                        .child(
                            widgets::outlined_button(
                                "numbering-cancel",
                                t("numbering-cancel", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.numbering = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            widgets::filled_button(
                                "numbering-apply",
                                t("numbering-apply", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.apply_numbering(cx))),
                        ),
                )
                .into_any_element(),
        )
    }
}
