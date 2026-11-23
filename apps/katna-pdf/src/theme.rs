// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's colours: Float's (docs/design/float.md), its own look
//! apart from Katna Mail's. Every colour is mixed from five a colour
//! scheme gives (page, card, text, faint and accent; `look.rs`), so the
//! schemes Katna Mail offers fit too. Float's own scheme keeps the
//! hand-tuned values of the storyboard.

use gpui::{BoxShadow, Hsla, Window, hsla, point, rgba};
use katna_ui::px;
use katna_ui::schemes::Side;
use katna_ui::theme::{SideScheme, Theme as Katna, mix, on};

use crate::look::{FLOAT, Look};

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    /// Behind everything: the window's own background (`--bg`).
    pub page: Hsla,
    /// Floating cards, the dock and menus (`--sf`).
    pub card: Hsla,
    /// Behind the pages: the window's background too.
    pub backdrop: Hsla,
    pub text: Hsla,
    /// Icons and text a step softer than `text` (`--tx2`).
    pub text2: Hsla,
    /// Secondary text (`--mu`).
    pub dim: Hsla,
    /// Lines between parts of a card (`--ln`).
    pub line: Hsla,
    /// Fainter lines, between rows (`--ln2`).
    pub line_soft: Hsla,
    /// Under the pointer (`--hov`).
    pub hover: Hsla,
    /// Fields and quiet buttons (`--sf2`).
    pub field: Hsla,
    /// Sunk below the card: segmented controls (`--sf3`).
    pub sunk: Hsla,
    pub accent: Hsla,
    /// A wash of the accent behind what is chosen (`--acs`).
    pub accent_soft: Hsla,
    /// The accent as text on `accent_soft` (`--act`).
    pub accent_text: Hsla,
    /// Text on the accent.
    pub on_accent: Hsla,
    /// The mode chosen in the dock, chips that are on (`--inv`).
    pub inverse: Hsla,
    pub on_inverse: Hsla,
    pub error: Hsla,
    pub error_soft: Hsla,
    /// Behind a dialog.
    pub scrim: Hsla,
    /// A signature that holds.
    pub valid: u32,
    /// The palette for Katna's shared controls (`katna_ui::widgets`),
    /// from the same scheme.
    pub katna: Katna,
}

/// The five colours a scheme is made from, `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Five {
    pub page: u32,
    pub card: u32,
    pub text: u32,
    pub faint: u32,
    pub accent: u32,
}

impl Five {
    pub const fn new(page: u32, card: u32, text: u32, faint: u32, accent: u32) -> Self {
        Self {
            page,
            card,
            text,
            faint,
            accent,
        }
    }

    fn side(&self, dark: bool) -> Side {
        let o = |c: u32| (c << 8) | 0xff;
        Side {
            page: o(self.page),
            card: o(self.card),
            text: o(self.text),
            faint: o(self.faint),
            accent: o(self.accent),
            error: if dark { 0xf28b82ff } else { 0xc5221fff },
        }
    }
}

fn c(rgb: u32) -> Hsla {
    rgba((rgb << 8) | 0xff).into()
}

/// `a` mixed `t` of the way to `b`, both `0xRRGGBB`.
fn m(a: u32, b: u32, t: f32) -> u32 {
    mix((a << 8) | 0xff, (b << 8) | 0xff, t) >> 8
}

impl Theme {
    /// The colours the window shows now: the look chosen, light or dark
    /// as its mode and the desktop say.
    pub fn current(window: &Window, look: &Look) -> Self {
        let dark = look.dark(window.appearance());
        Self::of(look, dark)
    }

    pub fn of(look: &Look, dark: bool) -> Self {
        if look.scheme_id() == FLOAT {
            let mut th = Self::float(dark);
            if look.accent != katna_ui::theme::Accent::Scheme {
                // Float's hand-picked colours, with the accent's mixed.
                let mixed = Self::mixed(FLOAT, look.five(dark), dark);
                th.accent = mixed.accent;
                th.accent_soft = mixed.accent_soft;
                th.accent_text = mixed.accent_text;
                th.on_accent = mixed.on_accent;
                th.katna = mixed.katna;
            }
            return th;
        }
        Self::mixed(look.scheme_id(), look.five(dark), dark)
    }

    /// Float's own, as the storyboard has it (`core.css`).
    pub fn float(dark: bool) -> Self {
        let five = crate::look::float_five(dark);
        let mut th = Self::mixed(FLOAT, five, dark);
        if dark {
            th.field = c(0x22262d);
            th.sunk = c(0x16191e);
            th.line = c(0x2c3039);
            th.line_soft = c(0x252930);
            th.text2 = c(0xc4c9d2);
            th.accent_text = c(0xa9c1ff);
            th.on_accent = c(0x0b1020);
        } else {
            th.field = c(0xf5f6f8);
            th.sunk = c(0xeceef2);
            th.line = c(0xdfe2e8);
            th.line_soft = c(0xeef0f3);
            th.text2 = c(0x3d434d);
            th.accent_text = c(0x1d4fd8);
        }
        th
    }

    /// Every colour mixed from `five`, as the storyboard's schemes are
    /// (`Win.dc.html`).
    pub fn mixed(id: &str, five: Five, dark: bool) -> Self {
        let Five {
            page: pg,
            card: cd,
            text: tx,
            faint: fa,
            accent: ac,
        } = five;
        let katna = Katna::from_scheme(&five.side(dark).scheme(id));
        let on_accent = on((ac << 8) | 0xff) >> 8;
        Self {
            dark,
            page: c(pg),
            card: c(cd),
            backdrop: c(pg),
            text: c(tx),
            text2: c(m(tx, cd, 0.18)),
            dim: c(fa),
            line: c(m(cd, tx, 0.15)),
            line_soft: c(m(cd, tx, 0.09)),
            // The text colour over whatever is under it, so a hover
            // shows on the title bar, the cards and the page alike.
            hover: c(tx).opacity(if dark { 0.12 } else { 0.09 }),
            field: c(m(cd, tx, 0.06)),
            sunk: c(m(pg, tx, 0.03)),
            accent: c(ac),
            accent_soft: c(m(cd, ac, if dark { 0.3 } else { 0.2 })),
            accent_text: c(if dark {
                m(ac, 0xffffff, 0.25)
            } else {
                m(ac, 0x000000, 0.15)
            }),
            on_accent: c(on_accent),
            inverse: c(tx),
            on_inverse: c(cd),
            error: c(if dark { 0xfca5a5 } else { 0xb91c1c }),
            error_soft: c(if dark { 0x2a1515 } else { 0xfde8e8 }),
            scrim: if dark {
                hsla(0.0, 0.0, 0.0, 0.5)
            } else {
                hsla(0.62, 0.2, 0.08, 0.28)
            },
            valid: if dark { 0x81c995ff } else { 0x1e8e3eff },
            katna,
        }
    }

    /// The soft shadow under the side card (`--sh`).
    pub fn shadow(&self) -> Vec<BoxShadow> {
        let a = if self.dark { 0.3 } else { 0.06 };
        let b = if self.dark { 0.35 } else { 0.08 };
        vec![
            BoxShadow {
                color: hsla(0.6, 0.2, 0.1, a),
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
                inset: false,
            },
            BoxShadow {
                color: hsla(0.6, 0.2, 0.1, b),
                offset: point(px(0.0), px(6.0)),
                blur_radius: px(20.0),
                spread_radius: px(0.0),
                inset: false,
            },
        ]
    }

    /// The deeper shadow under the dock, trays, menus and cards (`--sh2`).
    pub fn lifted(&self) -> Vec<BoxShadow> {
        let (a, b) = if self.dark { (0.55, 0.0) } else { (0.18, 0.08) };
        vec![
            BoxShadow {
                color: hsla(0.6, 0.2, 0.08, a),
                offset: point(px(0.0), px(10.0)),
                blur_radius: px(40.0),
                spread_radius: px(0.0),
                inset: false,
            },
            BoxShadow {
                color: hsla(0.6, 0.2, 0.08, b),
                offset: point(px(0.0), px(2.0)),
                blur_radius: px(6.0),
                spread_radius: px(0.0),
                inset: false,
            },
        ]
    }
}

impl Theme {
    /// The shadow under a page.
    pub fn page_shadow(&self) -> Vec<BoxShadow> {
        let (a, b) = if self.dark { (0.3, 0.45) } else { (0.1, 0.08) };
        vec![
            BoxShadow {
                color: hsla(0.6, 0.2, 0.1, a),
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(3.0),
                spread_radius: px(0.0),
                inset: false,
            },
            BoxShadow {
                color: hsla(0.6, 0.2, 0.1, b),
                offset: point(px(0.0), px(8.0)),
                blur_radius: px(28.0),
                spread_radius: px(0.0),
                inset: false,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_halfway() {
        assert_eq!(m(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(m(0x112233, 0x112233, 0.3), 0x112233);
    }

    #[test]
    fn float_keeps_its_own_values() {
        let day = Theme::float(false);
        assert_eq!(day.accent, c(0x2f6bff));
        assert_eq!(day.line, c(0xdfe2e8));
        let night = Theme::float(true);
        assert_eq!(night.card, c(0x1b1e24));
    }
}
