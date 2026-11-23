// SPDX-License-Identifier: GPL-3.0-or-later

//! The chosen comment's look, in its card: line width, opacity, solid,
//! dashed or cloudy edges, and what a line ends in at each end (`/LE`:
//! arrows, circles, squares, diamonds, butts and slashes).

use gpui::{AnyElement, Context, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, LineEnd};
use katna_ui::px;
use katna_ui::tokens::{space, text};

use super::Viewer;
use crate::float::{self, FloatTip};
use crate::text::t;

/// The opacities offered, in hundredths.
const OPACITIES: [u32; 4] = [100, 75, 50, 25];
/// The thinnest and thickest lines, and the step between.
const WIDTHS: (f32, f32, f32) = (0.5, 12.0, 0.5);

/// Whether `a` is drawn with a line whose width and dashes matter.
fn has_line(a: &Annotation) -> bool {
    matches!(
        a.kind,
        AnnotKind::Ink(_)
            | AnnotKind::Square { .. }
            | AnnotKind::Circle { .. }
            | AnnotKind::Line { .. }
            | AnnotKind::Polygon(_)
            | AnnotKind::PolyLine(_)
    )
}

/// The words of `end`'s name.
fn end_words(end: LineEnd) -> String {
    t(&format!("line-end-{}", end.name().to_lowercase()), &[])
}

/// The end after `end` in the list, round to the first.
fn next_end(end: LineEnd) -> LineEnd {
    let i = LineEnd::ALL.iter().position(|e| *e == end).unwrap_or(0);
    LineEnd::ALL[(i + 1) % LineEnd::ALL.len()]
}

impl Viewer {
    /// Changes the chosen comment's look, unless it is locked.
    fn restyle_chosen(&mut self, change: impl FnOnce(&mut Annotation), cx: &mut Context<Self>) {
        let Some(mut a) = self.chosen_annotation().cloned() else {
            return;
        };
        if a.flags.locked {
            return;
        }
        change(&mut a);
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// The row of `a`'s look in its card; none for comments that have
    /// none to change (notes, attached files, replies).
    pub(super) fn comment_look_row(
        &self,
        a: &Annotation,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let th = self.theme;
        if a.reply_to.is_some()
            || matches!(
                a.kind,
                AnnotKind::Note | AnnotKind::FileAttachment { .. } | AnnotKind::Other(_)
            )
        {
            return None;
        }
        let label = |id: &str| {
            div()
                .w(px(64.0))
                .flex_none()
                .text_size(px(text::CAPTION))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let row = || div().flex().items_center().flex_wrap().gap(px(space::S1));
        let mut rows = div().flex().flex_col().gap(px(space::S1)).pt(px(space::S1));
        if has_line(a) {
            let width = a.width;
            let step = |id: &'static str, icon: &'static str, by: f32| {
                float::sized_ib(id, icon, 24.0, 14.0, t(id, &[]), false, &th).on_click(cx.listener(
                    move |this, _, _, cx| {
                        let (low, high, _) = WIDTHS;
                        let to = ((width + by) / WIDTHS.2).round() * WIDTHS.2;
                        this.restyle_chosen(|a| a.width = to.clamp(low, high), cx);
                    },
                ))
            };
            rows = rows.child(
                row()
                    .child(label("comment-look-width"))
                    .child(step("comment-thinner", "minus", -WIDTHS.2))
                    .child(
                        div()
                            .min_w(px(40.0))
                            .text_size(px(text::CAPTION))
                            .text_color(th.text)
                            .child(t("comment-look-points", &[("n", &format!("{width:.1}"))])),
                    )
                    .child(step("comment-thicker", "plus", WIDTHS.2)),
            );
            let cloudable = matches!(a.kind, AnnotKind::Square { .. } | AnnotKind::Polygon(_));
            let styles = [
                ("comment-look-solid", !a.dashed && !a.cloudy, false, false),
                ("comment-look-dashed", a.dashed, true, false),
            ]
            .into_iter()
            .chain(cloudable.then_some(("comment-look-cloudy", a.cloudy, false, true)));
            let mut chips = row().child(label("comment-look-line"));
            for (i, (id, on, dashed, cloudy)) in styles.enumerate() {
                chips = chips.child(
                    float::chip(("comment-line", i), t(id, &[]), on, &th).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.restyle_chosen(
                                |a| {
                                    a.dashed = dashed;
                                    a.cloudy = cloudy;
                                },
                                cx,
                            )
                        },
                    )),
                );
            }
            rows = rows.child(chips);
        }
        if matches!(a.kind, AnnotKind::Line { .. } | AnnotKind::PolyLine(_)) {
            let ends = a.line_ends();
            let mut chips = row().child(label("comment-look-ends"));
            for (i, (id, end)) in [
                ("comment-look-start", ends[0]),
                ("comment-look-end", ends[1]),
            ]
            .into_iter()
            .enumerate()
            {
                let words = t(id, &[("end", &end_words(end))]);
                chips = chips.child(
                    float::chip(("comment-end", i), words, end != LineEnd::None, &th)
                        .ftip(t("comment-look-end-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.restyle_chosen(
                                |a| {
                                    let mut ends = a.line_ends();
                                    ends[i] = next_end(ends[i]);
                                    a.ends = ends;
                                    if let AnnotKind::Line { arrow, .. } = &mut a.kind {
                                        *arrow = ends[1] != LineEnd::None;
                                    }
                                },
                                cx,
                            )
                        })),
                );
            }
            rows = rows.child(chips);
        }
        let percent = (a.opacity * 100.0).round() as u32;
        let mut chips = row().child(label("comment-look-opacity"));
        for (i, o) in OPACITIES.into_iter().enumerate() {
            chips = chips.child(
                float::chip(("comment-opacity", i), format!("{o}%"), percent == o, &th).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.restyle_chosen(|a| a.opacity = o as f32 / 100.0, cx)
                    }),
                ),
            );
        }
        rows = rows.child(chips);
        Some(rows.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_go_round() {
        let mut end = LineEnd::None;
        for _ in 0..LineEnd::ALL.len() {
            end = next_end(end);
        }
        assert_eq!(end, LineEnd::None);
        assert_eq!(next_end(LineEnd::None), LineEnd::OpenArrow);
    }
}
