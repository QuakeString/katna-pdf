// SPDX-License-Identifier: GPL-3.0-or-later

//! Cursor coordinates (View > Cursor Coordinates): where the pointer is
//! on the page under it, from the page's top left, in the units picked
//! in Settings, shown in a small card at the bottom left.

use gpui::{AnyElement, App, Context, div, prelude::*};
use katna_ui::px;
use katna_ui::tokens::{space, text};

use super::Viewer;
use crate::text::t;

impl Viewer {
    /// Shows the card, or hides it.
    pub(super) fn toggle_coordinates(&mut self, cx: &mut Context<Self>) {
        self.coords_shown = !self.coords_shown;
        self.cursor_at = None;
        cx.notify();
    }

    /// Notes where the pointer is, while the card shows.
    pub(super) fn coords_move(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !self.coords_shown {
            return;
        }
        let at = self.page_point(position).map(|(page, [x, y])| {
            let (w, h) = self.page_size(page);
            (page, [x.clamp(0.0, w), y.clamp(0.0, h)])
        });
        if at != self.cursor_at {
            self.cursor_at = at;
            cx.notify();
        }
    }

    /// The card, while shown.
    pub(super) fn coords_card(&self, cx: &App) -> Option<AnyElement> {
        if !self.coords_shown {
            return None;
        }
        let th = self.theme;
        let units = cx.global::<crate::prefs::Prefs>().units;
        let words = match self.cursor_at {
            Some((page, [x, y])) => {
                let (w, h) = self.page_size(page);
                let n = |v: f32| format_length(v / units.points(), units);
                t(
                    "coords-at",
                    &[
                        ("page", &self.page_label(page)),
                        ("x", &n(x)),
                        ("y", &n(y)),
                        ("width", &n(w)),
                        ("height", &n(h)),
                        ("unit", units.suffix()),
                    ],
                )
            }
            None => t("coords-off-page", &[]),
        };
        Some(
            div()
                .h(px(30.0))
                .px(px(space::S3))
                .flex()
                .items_center()
                .text_size(px(text::SMALL))
                .text_color(th.text2)
                .child(words)
                .into_any_element(),
        )
    }
}

/// A length in `units`: whole points, millimetres to one place, inches
/// to two.
fn format_length(v: f32, units: crate::prefs::Units) -> String {
    match units {
        crate::prefs::Units::Points => format!("{v:.0}"),
        crate::prefs::Units::Millimetres => format!("{v:.1}"),
        crate::prefs::Units::Inches => format!("{v:.2}"),
    }
}

#[cfg(test)]
mod tests {
    use super::format_length;
    use crate::prefs::Units;

    #[test]
    fn lengths_are_written_to_their_units_places() {
        assert_eq!(format_length(595.3, Units::Points), "595");
        assert_eq!(format_length(210.04, Units::Millimetres), "210.0");
        assert_eq!(format_length(8.5, Units::Inches), "8.50");
    }
}
