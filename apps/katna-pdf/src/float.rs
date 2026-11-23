// SPDX-License-Identifier: GPL-3.0-or-later

//! Float's controls (docs/design/float.md): the title bar's icon buttons,
//! the dock's and trays' buttons, menus and their rows, chips, the
//! segmented control and the avatar. Colours come from [`Theme`]; sizes
//! are the storyboard's (`core.css`).

use gpui::{
    AnyElement, Div, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, Stateful, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, svg,
};
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};
use katna_ui::tooltip::Tooltip;

use crate::theme::Theme;

/// The title bar's height.
pub const BAR: f32 = 48.0;
/// The side card's gap to the window's edges.
pub const GAP: f32 = 12.0;
/// From the window's bottom edge to the dock's.
pub const DOCK_BOTTOM: f32 = 18.0;
/// Text inside the title bar, the dock and menus.
pub const UI_TEXT: f32 = text::SMALL;

/// A Float icon (`icons/f/NAME.svg`) at `size` in `color`.
pub fn icon(name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg()
        .path(SharedString::from(format!("icons/f/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// A tooltip in Float's inverse colours, also read out by screen
/// readers.
pub trait FloatTip: StatefulInteractiveElement + Sized {
    fn ftip(self, words: impl Into<SharedString>, th: &Theme) -> Self {
        let words = words.into();
        let (bg, fg) = (th.inverse, th.on_inverse);
        self.aria_label(words.clone())
            .tooltip(Tooltip::text(words, bg, fg))
    }
}

impl<E: StatefulInteractiveElement> FloatTip for E {}

/// A 32 px icon button (`.ib`): the title bar's and the side card's.
/// `on` shows it pressed in.
pub fn ib(
    id: impl Into<ElementId>,
    name: &str,
    tip: impl Into<SharedString>,
    on: bool,
    th: &Theme,
) -> Stateful<Div> {
    sized_ib(id, name, 32.0, 18.0, tip, on, th)
}

pub fn sized_ib(
    id: impl Into<ElementId>,
    name: &str,
    side: f32,
    glyph: f32,
    tip: impl Into<SharedString>,
    on: bool,
    th: &Theme,
) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .when(on, |d| d.bg(th.hover))
        .hover(move |s| s.bg(hover))
        // Keep the title bar from starting a window move.
        .on_mouse_move(|_, _, cx| cx.stop_propagation())
        .child(icon(
            name,
            glyph,
            if on { th.accent_text } else { th.text2 },
        ))
        .ftip(tip, th)
}

/// How a dock or tray button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressed {
    No,
    /// The mode chosen: inverse colours, with its name.
    Mode,
    /// A tool in hand: the accent's wash.
    Tool,
}

/// A dock or tray button (`.dk`), 36 px high; `label` shows beside the
/// icon.
pub fn dk(
    id: impl Into<ElementId>,
    name: &str,
    label: Option<SharedString>,
    tip: impl Into<SharedString>,
    pressed: Pressed,
    th: &Theme,
) -> Stateful<Div> {
    let (bg, fg) = match pressed {
        Pressed::No => (None, th.text2),
        Pressed::Mode => (Some(th.inverse), th.on_inverse),
        Pressed::Tool => (Some(th.accent_soft), th.accent_text),
    };
    let hover = th.hover;
    div()
        .id(id)
        .h(px(36.0))
        .min_w(px(36.0))
        .px(px(space::S3))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(radius::SM))
        .text_size(px(UI_TEXT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .cursor_pointer()
        .whitespace_nowrap()
        .map(|d| match bg {
            Some(bg) => d.bg(bg),
            None => d.hover(move |s| s.bg(hover)),
        })
        .child(icon(name, 18.0, fg))
        .children(label)
        .ftip(tip, th)
}

/// A dock button with words only, such as the zoom or the page number.
pub fn dk_text(id: impl Into<ElementId>, th: &Theme) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .h(px(36.0))
        .min_w(px(36.0))
        .px(px(space::S3))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(space::S2))
        .rounded(px(radius::SM))
        .text_size(px(UI_TEXT))
        .font_weight(FontWeight::MEDIUM)
        .text_color(th.text2)
        .cursor_pointer()
        .whitespace_nowrap()
        .hover(move |s| s.bg(hover))
}

/// The thin line between groups (`.vsep`).
pub fn vsep(th: &Theme) -> Div {
    div()
        .w(px(1.0))
        .h(px(20.0))
        .mx(px(space::S2))
        .flex_none()
        .bg(th.line)
}

/// A floating bar: the dock and the trays above it.
pub fn bar_shell(th: &Theme, radius_px: f32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(space::S1))
        .p(px(5.0))
        .bg(th.card)
        .border_1()
        .border_color(th.line)
        .rounded(px(radius_px))
        .shadow(th.lifted())
        .whitespace_nowrap()
}

/// A mode's tool tray (C01, E01): a bar over the dock that scrolls
/// sideways when the window is too narrow for it.
pub fn tray(id: impl Into<ElementId>, th: &Theme) -> Stateful<Div> {
    bar_shell(th, 12.0)
        .id(id)
        .occlude()
        .max_w_full()
        .flex_none()
        .overflow_x_scroll()
}

/// A colour to pick in a tray: a dot, ringed when it is the one picked.
pub fn swatch(id: impl Into<ElementId>, fill: Hsla, on: bool, th: &Theme) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .size(px(26.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .when(on, |d| d.border_2().border_color(th.text))
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
        .child(
            div()
                .size(px(16.0))
                .rounded_full()
                .border_1()
                .border_color(th.line)
                .bg(fill),
        )
}

/// An on and off switch (`.sw`).
pub fn switch(id: impl Into<ElementId>, on: bool, th: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.0))
        .h(px(20.0))
        .flex_none()
        .p(px(2.0))
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .rounded_full()
        .cursor_pointer()
        .bg(if on { th.accent } else { th.line })
        .child(div().size(px(16.0)).rounded_full().bg(th.card).shadow_sm())
}

/// A card's row: its name on the left, what sets it on the right.
pub fn field_row(words: impl Into<SharedString>, th: &Theme) -> Div {
    div()
        .min_h(px(32.0))
        .flex()
        .items_center()
        .gap(px(space::S2))
        .child(
            div()
                .w(px(64.0))
                .flex_none()
                .text_size(px(UI_TEXT))
                .text_color(th.text2)
                .child(words.into()),
        )
}

/// A popover menu's box (`.pop`).
pub fn menu(id: impl Into<ElementId>, width: f32, th: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .occlude()
        .w(px(width))
        .p(px(space::S2))
        .flex()
        .flex_col()
        .bg(th.card)
        .rounded(px(radius::SM))
        .shadow(th.lifted())
        .when(th.dark, |d| d.border_1().border_color(th.line))
}

/// A menu row (`.mi`): an icon, words, and keys or a value at the end.
pub fn menu_item(
    id: impl Into<ElementId>,
    glyph: Option<&str>,
    words: impl Into<SharedString>,
    end: Option<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .h(px(32.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .rounded(px(5.0))
        .text_size(px(UI_TEXT))
        .text_color(th.text)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .children(glyph.map(|g| icon(g, 16.0, th.text2)))
        .child(div().flex_1().min_w_0().truncate().child(words.into()))
        .children(end.map(|end| {
            div()
                .pl(px(20.0))
                .text_size(px(11.5))
                .text_color(th.dim)
                .child(end)
        }))
}

/// The line between groups of a menu (`.msep`).
pub fn menu_sep(th: &Theme) -> Div {
    div().h(px(1.0)).my(px(space::S2)).bg(th.line_soft)
}

/// A small heading in capitals (`.mhd`, `.phead`, `.sec`).
pub fn heading(words: impl Into<SharedString>, th: &Theme) -> Div {
    div()
        .px(px(10.0))
        .pt(px(space::S3))
        .pb(px(space::S2))
        .text_size(px(text::MICRO))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(th.dim)
        .child(words.into().to_uppercase())
}

/// A round chip (`.chip`); `on` in inverse colours.
pub fn chip(
    id: impl Into<ElementId>,
    words: impl Into<SharedString>,
    on: bool,
    th: &Theme,
) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .h(px(26.0))
        .px(px(10.0))
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(13.0))
        .border_1()
        .text_size(px(text::CAPTION))
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|d| {
            if on {
                d.bg(th.inverse)
                    .border_color(th.inverse)
                    .text_color(th.on_inverse)
            } else {
                d.border_color(th.line)
                    .text_color(th.text)
                    .hover(move |s| s.bg(hover))
            }
        })
        .child(words.into())
}

/// One choice of a segmented control (`.seg button`).
pub fn seg_item(
    id: impl Into<ElementId>,
    glyph: Option<&str>,
    words: Option<SharedString>,
    on: bool,
    th: &Theme,
) -> Stateful<Div> {
    let fg = if on { th.text } else { th.dim };
    div()
        .id(id)
        .flex_1()
        .h(px(28.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(6.0))
        .text_size(px(UI_TEXT))
        .text_color(fg)
        .cursor_pointer()
        .when(on, |d| {
            d.bg(if th.dark { th.line } else { th.card })
                .font_weight(FontWeight::SEMIBOLD)
                .shadow_sm()
        })
        .children(glyph.map(|g| icon(g, 14.0, fg)))
        .children(words)
}

/// The box around a segmented control's choices (`.seg`).
pub fn seg(th: &Theme) -> Div {
    div()
        .flex()
        .gap(px(space::S1))
        .p(px(space::S1))
        .rounded(px(radius::SM))
        .bg(if th.dark { th.field } else { th.sunk })
}

/// The person's round picture: their initials on the accent.
pub fn avatar(initials: &str, side: f32, th: &Theme) -> Div {
    div()
        .size(px(side))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded(px(side / 2.0))
        .bg(th.accent)
        .text_color(th.on_accent)
        .text_size(px((side * 0.43).round()))
        .font_weight(FontWeight::BOLD)
        .child(initials.to_owned())
}

/// A button (`.btn`); `primary` in the accent.
pub fn button(
    id: impl Into<ElementId>,
    words: impl Into<SharedString>,
    primary: bool,
    th: &Theme,
) -> Stateful<Div> {
    let hover = th.hover;
    div()
        .id(id)
        .h(px(32.0))
        .px(px(space::S4))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .rounded(px(7.0))
        .border_1()
        .text_size(px(UI_TEXT))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .cursor_pointer()
        .map(|d| {
            if primary {
                d.bg(th.accent)
                    .border_color(th.accent)
                    .text_color(th.on_accent)
                    .hover(|s| s.opacity(0.92))
            } else {
                d.bg(th.field)
                    .border_color(th.line)
                    .text_color(th.text)
                    .hover(move |s| s.bg(hover))
            }
        })
        .child(words.into())
}

/// Wraps `menu` to open under (or above) the element it belongs to,
/// lined up with its left (or right) edge, kept inside the window.
pub fn popover(menu: impl IntoElement, above: bool, right: bool) -> AnyElement {
    let anchor = match (above, right) {
        (false, false) => gpui::Anchor::TopLeft,
        (false, true) => gpui::Anchor::TopRight,
        (true, false) => gpui::Anchor::BottomLeft,
        (true, true) => gpui::Anchor::BottomRight,
    };
    div()
        .absolute()
        .map(|d| if above { d.top_0() } else { d.top_full() })
        .map(|d| if right { d.right_0() } else { d.left_0() })
        .child(
            gpui::deferred(
                katna_ui::anchored().anchor(anchor).snap_to_window().child(
                    div()
                        .map(|d| if above { d.mb(px(8.0)) } else { d.mt(px(6.0)) })
                        .child(menu),
                ),
            )
            .with_priority(2),
        )
        .into_any_element()
}

/// The person's initials, from their name.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace().filter_map(|w| w.chars().next());
    match (words.next(), words.next_back()) {
        (Some(a), Some(b)) => [a, b].iter().flat_map(|c| c.to_uppercase()).collect(),
        (Some(a), None) => a.to_uppercase().collect(),
        _ => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Mozammel Hossain"), "MH");
        assert_eq!(initials("anita"), "A");
        assert_eq!(initials("Rao Anita Kumari"), "RK");
        assert_eq!(initials(""), "?");
    }
}
