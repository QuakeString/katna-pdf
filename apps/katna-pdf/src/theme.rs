// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's colours: Katna Mail's palette (docs/PLAN.md §2.6), as the
//! mockups use it. Light or dark follows the desktop.

use gpui::{Hsla, WindowAppearance, rgb, rgba};

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
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub error: Hsla,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
        }
    }

    pub fn light() -> Self {
        Self {
            page: rgb(0xf6f8fc).into(),
            card: rgb(0xffffff).into(),
            backdrop: rgb(0xeceff4).into(),
            text: rgb(0x1f1f1f).into(),
            dim: rgb(0x444746).into(),
            line: rgba(0x64798f1f).into(),
            hover: rgba(0x1f1f1f12).into(),
            field: rgb(0xe9eef6).into(),
            accent: rgb(0x0b57d0).into(),
            on_accent: rgb(0xffffff).into(),
            error: rgb(0xb3261e).into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            page: rgb(0x131416).into(),
            card: rgb(0x1f2124).into(),
            backdrop: rgb(0x0d0e10).into(),
            text: rgb(0xe3e3e3).into(),
            dim: rgb(0xc4c7c5).into(),
            line: rgba(0xffffff12).into(),
            hover: rgba(0xffffff14).into(),
            field: rgb(0x2a2d31).into(),
            accent: rgb(0xa8c7fa).into(),
            on_accent: rgb(0x062e6f).into(),
            error: rgb(0xf2b8b5).into(),
        }
    }
}
