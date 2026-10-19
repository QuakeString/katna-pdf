// SPDX-License-Identifier: GPL-3.0-or-later

//! A calendar under a date field being filled in (docs/PLAN.md §4.8):
//! the field's own date format says how the day picked is written.

use gpui::{AnyElement, Context, div, prelude::*};
use jiff::civil::Date;
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::Viewer;
use crate::text::t;

/// The month a date field's calendar opens on: its date's, or today's.
pub(super) fn month_of(value: &Option<String>, pattern: &str) -> (i16, i8) {
    if let Some((y, m, ..)) = value
        .as_deref()
        .and_then(|v| katna_pdf_cos::parse_date(v, pattern))
        && let (Ok(y), Ok(m)) = (i16::try_from(y), i8::try_from(m))
        && (1..=12).contains(&m)
    {
        return (y, m);
    }
    let today = today();
    (today.year(), today.month())
}

fn today() -> Date {
    jiff::Zoned::now().date()
}

/// The month after (`step` 1) or before (-1) `(year, month)`.
fn step_month((year, month): (i16, i8), step: i8) -> (i16, i8) {
    match month + step {
        0 => (year - 1, 12),
        13 => (year + 1, 1),
        m => (year, m),
    }
}

impl Viewer {
    /// The calendar for the date field being filled in, showing `month`.
    pub(super) fn date_picker(
        &self,
        pattern: &str,
        month: (i16, i8),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Ok(first) = Date::new(month.0, month.1, 1) else {
            return div().into_any_element();
        };
        let today = today();
        let chosen = self
            .form
            .as_ref()
            .and_then(|f| f.editing.as_ref())
            .and_then(|e| katna_pdf_cos::parse_date(e.input.read(cx).text(), pattern));
        let go = |step: i8| {
            cx.listener(move |this: &mut Self, _, _, cx| {
                if let Some(editing) = this.form.as_mut().and_then(|f| f.editing.as_mut())
                    && let Some(month) = editing.month.as_mut()
                {
                    *month = step_month(*month, step);
                }
                cx.notify();
            })
        };
        let header = div()
            .flex()
            .items_center()
            .child(
                widgets::icon_button("date-earlier", "chevron-left", 16.0, &th.katna)
                    .on_click(go(-1)),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .text_size(px(text::BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(t(
                        "date-month",
                        &[
                            ("month", &t(&format!("date-month-{}", month.1), &[])),
                            ("year", &month.0.to_string()),
                        ],
                    )),
            )
            .child(
                widgets::icon_button("date-later", "chevron-right", 16.0, &th.katna)
                    .on_click(go(1)),
            );
        let cell = 32.0;
        let mut grid = div().flex().flex_wrap().w(px(cell * 7.0));
        for day in 1..=7 {
            grid = grid.child(
                div()
                    .w(px(cell))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t(&format!("date-day-{day}"), &[])),
            );
        }
        // Weeks start on Monday, as in India and most of the world.
        let blank = first.weekday().to_monday_zero_offset();
        for _ in 0..blank {
            grid = grid.child(div().w(px(cell)).h(px(cell)));
        }
        let pattern = pattern.to_owned();
        for day in 1..=first.days_in_month() {
            let date = (i32::from(month.0), month.1 as u32, day as u32);
            let is_chosen = chosen.is_some_and(|c| (c.0, c.1, c.2) == date);
            let is_today = Date::new(month.0, month.1, day).is_ok_and(|d| d == today);
            let pattern = pattern.clone();
            grid = grid.child(
                div()
                    .id(("date-day", day as usize))
                    .w(px(cell))
                    .h(px(cell))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::FULL))
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .when(is_today, |d| d.border_1().border_color(th.line))
                    .when(is_chosen, |d| {
                        d.bg(gpui::rgba(th.katna.accent))
                            .text_color(gpui::rgb(0xffffff))
                    })
                    .hover(move |s| s.bg(th.hover))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pick_date(date, &pattern, cx);
                    }))
                    .child(day.to_string()),
            );
        }
        // Drawn after the page's other fields, so it lies over them.
        let picker = div()
            .id("date-picker")
            .mt(px(space::S1))
            .p(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow(widgets::elevation(&th.katna, 2.0))
            .text_size(px(text::BODY))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(grid);
        div()
            .absolute()
            .top_full()
            .left_0()
            .child(gpui::deferred(picker).with_priority(1))
            .into_any_element()
    }

    /// Writes day `(year, month, day)` into the date field being filled
    /// in, as `pattern` says, and ends typing there.
    fn pick_date(&mut self, (y, m, d): (i32, u32, u32), pattern: &str, cx: &mut Context<Self>) {
        let Some(editing) = self.form.as_ref().and_then(|f| f.editing.as_ref()) else {
            return;
        };
        let text = katna_pdf_cos::write_date((y, m, d, 0, 0), pattern);
        editing.input.update(cx, |i, cx| i.set_text(text, cx));
        self.finish_typing(true, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_step_over_years() {
        assert_eq!(step_month((2026, 12), 1), (2027, 1));
        assert_eq!(step_month((2026, 1), -1), (2025, 12));
        assert_eq!(step_month((2026, 5), 1), (2026, 6));
        assert_eq!(
            month_of(&Some("05/01/2026".into()), "dd/mm/yyyy"),
            (2026, 1)
        );
    }
}
