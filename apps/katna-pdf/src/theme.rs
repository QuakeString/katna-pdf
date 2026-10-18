// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's colours: Katna's shared palette (`katna_ui::theme`, the
//! one Katna Mail uses; docs/PLAN.md §2.6), as the mockups use it. Light
//! or dark follows the desktop.

use gpui::{Hsla, WindowAppearance, rgba};
use katna_ui::theme::Theme as Katna;

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Behind everything: the window's own background.
    pub page: Hsla,
    /// The document card.
    pub card: Hsla,
    /// Behind the pages, inside the card.
    pub backdrop: Hsla,
    pub text: Hsla,
    /// Secondary text and icons.
    pub dim: Hsla,
    /// Lines between parts of the card.
    pub line: Hsla,
    /// Under the pointer.
    pub hover: Hsla,
    /// The page-number field.
    pub field: Hsla,
    pub error: Hsla,
    /// The palette itself, for Katna's shared controls
    /// (`katna_ui::widgets`).
    pub katna: Katna,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    pub fn light() -> Self {
        Self::from_katna(&Katna::new(false), 0xeceff4ff)
    }

    pub fn dark() -> Self {
        Self::from_katna(&Katna::new(true), 0x0d0e10ff)
    }

    /// Katna's palette; `backdrop`, behind the pages, is Katna PDF's own.
    fn from_katna(k: &Katna, backdrop: u32) -> Self {
        Self {
            page: rgba(k.page).into(),
            card: rgba(k.surface).into(),
            backdrop: rgba(backdrop).into(),
            text: rgba(k.text).into(),
            dim: rgba(k.text_dim).into(),
            line: rgba(k.outline).into(),
            hover: rgba(k.hover).into(),
            field: rgba(k.search).into(),
            error: rgba(k.error).into(),
            katna: *k,
        }
    }
}
