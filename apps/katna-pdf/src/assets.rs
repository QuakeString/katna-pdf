// SPDX-License-Identifier: GPL-3.0-or-later

//! Icons built into the binary, served to GPUI's `svg()` element. They are
//! Katna Mail's (`apps/katna-mail/icons/` in the Katna repository), so
//! both apps look alike.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    ("icons/close.svg", include_bytes!("../icons/close.svg")),
    (
        "icons/document.svg",
        include_bytes!("../icons/document.svg"),
    ),
    (
        "icons/fit-width.svg",
        include_bytes!("../icons/fit-width.svg"),
    ),
    ("icons/folder.svg", include_bytes!("../icons/folder.svg")),
    ("icons/zoom-in.svg", include_bytes!("../icons/zoom-in.svg")),
    (
        "icons/zoom-out.svg",
        include_bytes!("../icons/zoom-out.svg"),
    ),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
