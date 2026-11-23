// SPDX-License-Identifier: GPL-3.0-or-later

//! Float's dock (docs/design/float.md): the modes (Read, Comment, Edit,
//! Pages, Forms, Sign), the zoom, the page number and the pages'
//! colours, floating at the bottom of the pages; the tool tray of the
//! mode chosen floats above it. The zoom opens a card of fits, steps
//! and layouts (R05).

use gpui::{AnyElement, Context, SharedString, div, prelude::*};
use katna_ui::px;
use katna_ui::tokens::space;

use super::{Content, POINT, Viewer, Zoom};
use crate::float::{
    self, FloatTip, Pressed, bar_shell, chip, dk, dk_text, menu, menu_item, menu_sep, popover, seg,
    seg_item,
};
use crate::spread::Spread;
use crate::text::t;

/// What the person is doing: the dock's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Read,
    Comment,
    Edit,
    Pages,
    Forms,
    Sign,
}

impl Mode {
    pub const ALL: [Mode; 6] = [
        Mode::Read,
        Mode::Comment,
        Mode::Edit,
        Mode::Pages,
        Mode::Forms,
        Mode::Sign,
    ];

    fn icon(self) -> &'static str {
        match self {
            Mode::Read => "cursor",
            Mode::Comment => "mark",
            Mode::Edit => "edit",
            Mode::Pages => "grid",
            Mode::Forms => "form",
            Mode::Sign => "sign",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Mode::Read => "mode-read",
            Mode::Comment => "mode-comment",
            Mode::Edit => "mode-edit",
            Mode::Pages => "mode-pages",
            Mode::Forms => "mode-forms",
            Mode::Sign => "mode-sign",
        }
    }
}

/// The zooms the card offers, in percent.
const STEPS: [f32; 7] = [50.0, 100.0, 125.0, 200.0, 400.0, 1600.0, 10_000.0];

impl Viewer {
    /// The mode on now.
    pub(super) fn mode(&self) -> Mode {
        if self.signing.is_some() {
            Mode::Sign
        } else if self.designing.is_some() {
            Mode::Forms
        } else if self.organizer {
            Mode::Pages
        } else if self.text_editing {
            Mode::Edit
        } else if self.tools {
            Mode::Comment
        } else {
            Mode::Read
        }
    }

    /// Leaves the mode on now and takes up `mode`.
    pub(super) fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let now = self.mode();
        if now == mode {
            return;
        }
        match now {
            Mode::Read => {}
            Mode::Comment => self.toggle_tools(cx),
            Mode::Edit => self.toggle_text_edit(cx),
            Mode::Pages => self.toggle_organizer(cx),
            Mode::Forms => self.toggle_designing(cx),
            Mode::Sign => self.toggle_signing(cx),
        }
        match mode {
            Mode::Read => {}
            Mode::Comment => self.toggle_tools(cx),
            Mode::Edit => self.toggle_text_edit(cx),
            Mode::Pages => self.toggle_organizer(cx),
            Mode::Forms => self.toggle_designing(cx),
            Mode::Sign => self.toggle_signing(cx),
        }
        cx.notify();
    }

    /// The zoom shown, in percent.
    fn percent(&self) -> f32 {
        (self.shown / POINT * 100.0).round()
    }

    /// The dock, once a document is open.
    pub(super) fn dock(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.content, Content::Ready(_)) {
            return None;
        }
        let th = self.theme;
        let now = self.mode();
        let labels = cx.global::<crate::prefs::Prefs>().dock_labels;
        let mut dock = bar_shell(&th, 12.0).id("dock").occlude();
        for (i, mode) in Mode::ALL.into_iter().enumerate() {
            let on = mode == now;
            dock = dock.child(
                dk(
                    ("dock-mode", i),
                    mode.icon(),
                    (on && labels).then(|| SharedString::from(t(mode.words(), &[]))),
                    t(mode.words(), &[]),
                    if on { Pressed::Mode } else { Pressed::No },
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    // The mode chosen again goes back to reading.
                    let next = if this.mode() == mode {
                        Mode::Read
                    } else {
                        mode
                    };
                    this.set_mode(next, cx);
                })),
            );
        }
        let page = self.current_page();
        let count = self.page_count();
        let shown = if self.label_ranges.is_empty() {
            (page + 1).to_string()
        } else {
            self.page_label(page)
        };
        dock = dock
            .child(float::vsep(&th))
            .child(
                dk(
                    "dock-zoom-out",
                    "minus",
                    None,
                    t("zoom-out", &[]),
                    Pressed::No,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step_zoom(false, cx))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        dk_text("dock-zoom", &th)
                            .min_w(px(54.0))
                            .ftip(t("zoom-card-tip", &[]), &th)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let just_closed = this
                                    .zoom_card_closed
                                    .is_some_and(|t| t.elapsed().as_millis() < 300);
                                this.zoom_card = !this.zoom_card && !just_closed;
                                cx.notify();
                            }))
                            .child(format!("{}%", self.percent())),
                    )
                    .children(self.zoom_card.then(|| self.zoom_card(cx))),
            )
            .child(
                dk(
                    "dock-zoom-in",
                    "plus",
                    None,
                    t("zoom-in", &[]),
                    Pressed::No,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.step_zoom(true, cx))),
            )
            .child(
                dk(
                    "dock-fit",
                    "fit",
                    None,
                    t("zoom-fit-page", &[]),
                    Pressed::No,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitPage, cx))),
            )
            .child(float::vsep(&th))
            .child(
                dk_text("dock-page", &th)
                    .ftip(t("numbering-tip", &[]), &th)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_numbering(cx)))
                    .child(shown)
                    .child(div().text_color(th.dim).child(format!("/ {count}"))),
            )
            .child(
                div()
                    .relative()
                    .child(
                        dk(
                            "dock-colours",
                            "moon",
                            None,
                            t("page-colors", &[]),
                            if self.colors_menu {
                                Pressed::Tool
                            } else {
                                Pressed::No
                            },
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let just_closed = this
                                .colors_menu_closed
                                .is_some_and(|t| t.elapsed().as_millis() < 300);
                            this.colors_menu = !this.colors_menu && !just_closed;
                            cx.notify();
                        })),
                    )
                    .children(self.colors_menu(cx)),
            );
        Some(dock.into_any_element())
    }

    /// The zoom card (R05): fits, steps, the layout and the hand tool.
    fn zoom_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let percent = self.percent();
        let fit = |id: &'static str, words: &'static str, keys: &'static str, zoom: Zoom| {
            let on = match (self.zoom, zoom) {
                (Zoom::FitWidth, Zoom::FitWidth)
                | (Zoom::FitPage, Zoom::FitPage)
                | (Zoom::FitHeight, Zoom::FitHeight) => true,
                (Zoom::Fixed(a), Zoom::Fixed(b)) => (a - b).abs() < 0.001,
                _ => false,
            };
            menu_item(id, None, t(words, &[]), Some(keys.into()), &th)
                .when(on, |d| d.bg(th.accent_soft).text_color(th.accent_text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_zoom(zoom, cx);
                }))
        };
        let steps = STEPS.into_iter().enumerate().map(|(i, step)| {
            let words = if step >= 10_000.0 {
                "10 000%".to_owned()
            } else {
                format!("{step}%")
            };
            chip(("zoom-step", i), words, (percent - step).abs() < 0.5, &th).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.set_zoom(Zoom::Fixed(step / 100.0), cx);
                },
            ))
        });
        let layouts = [
            (Spread::Single, "single", "spread-single"),
            (Spread::Facing, "two", "spread-facing"),
            (Spread::Book, "book", "spread-book"),
        ]
        .map(|(spread, glyph, words)| {
            seg_item(
                ("zoom-layout", spread as usize),
                Some(glyph),
                None,
                self.spread == spread,
                &th,
            )
            .ftip(t(words, &[]), &th)
            .on_click(cx.listener(move |this, _, _, cx| {
                while this.spread != spread {
                    this.next_spread(cx);
                }
            }))
        });
        let hand = self.hand;
        let zooming = self
            .area
            .as_ref()
            .is_some_and(|a| a.kind == super::area::AreaKind::Zoom);
        let body = menu("zoom-card", 250.0, &th)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.zoom_card = false;
                this.zoom_card_closed = Some(std::time::Instant::now());
                cx.notify();
            }))
            .child(fit(
                "zoom-fit-width",
                "zoom-card-fit-width",
                "Ctrl 1",
                Zoom::FitWidth,
            ))
            .child(fit(
                "zoom-fit-page",
                "zoom-card-fit-page",
                "Ctrl 2",
                Zoom::FitPage,
            ))
            .child(fit(
                "zoom-fit-height",
                "zoom-card-fit-height",
                "Ctrl 4",
                Zoom::FitHeight,
            ))
            .child(fit(
                "zoom-actual",
                "zoom-actual-size",
                "Ctrl 0",
                Zoom::Fixed(1.0),
            ))
            .child(menu_sep(&th))
            .child(
                div()
                    .px(px(space::S2))
                    .py(px(space::S2))
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S2))
                    .children(steps),
            )
            .child(menu_sep(&th))
            .child(float::heading(t("zoom-layout", &[]), &th))
            .child(
                div()
                    .px(px(space::S2))
                    .pb(px(space::S2))
                    .child(seg(&th).children(layouts)),
            )
            .child(
                menu_item(
                    "zoom-hand",
                    Some("hand"),
                    t("zoom-card-hand", &[]),
                    None,
                    &th,
                )
                .when(hand, |d| d.bg(th.accent_soft).text_color(th.accent_text))
                .on_click(cx.listener(move |this, _, _, cx| this.set_hand(!hand, cx))),
            )
            .child(
                menu_item(
                    "zoom-area",
                    Some("search"),
                    t("zoom-card-area", &[]),
                    None,
                    &th,
                )
                .when(zooming, |d| d.bg(th.accent_soft).text_color(th.accent_text))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.zoom_card = false;
                    this.toggle_area_tool(super::area::AreaKind::Zoom, cx)
                })),
            )
            .child(
                div()
                    .flex()
                    .child(
                        menu_item(
                            "zoom-turn-left",
                            Some("rotate-ccw"),
                            t("zoom-card-rotate-left", &[]),
                            None,
                            &th,
                        )
                        .flex_1()
                        .on_click(cx.listener(|this, _, _, cx| this.turn_view(3, cx))),
                    )
                    .child(
                        menu_item(
                            "zoom-turn-right",
                            Some("rotate-cw"),
                            t("zoom-card-rotate-right", &[]),
                            None,
                            &th,
                        )
                        .flex_1()
                        .on_click(cx.listener(|this, _, _, cx| this.turn_view(1, cx))),
                    ),
            );
        popover(body, true, false)
    }
}
