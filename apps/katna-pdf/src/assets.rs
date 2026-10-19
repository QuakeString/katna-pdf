// SPDX-License-Identifier: GPL-3.0-or-later

//! Icons built into the binary, served to GPUI's `svg()` element. Most are
//! Katna Mail's (`apps/katna-mail/icons/` in the Katna repository), so
//! both apps look alike; the annotation shapes, ink and stamp are drawn
//! here in the same style.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

const ICONS: &[(&str, &[u8])] = &[
    (
        "icons/arrow-down.svg",
        include_bytes!("../icons/arrow-down.svg"),
    ),
    (
        "icons/arrow-up.svg",
        include_bytes!("../icons/arrow-up.svg"),
    ),
    (
        "icons/download.svg",
        include_bytes!("../icons/download.svg"),
    ),
    (
        "icons/insert-below.svg",
        include_bytes!("../icons/insert-below.svg"),
    ),
    (
        "icons/rotate-ccw.svg",
        include_bytes!("../icons/rotate-ccw.svg"),
    ),
    (
        "icons/to-front.svg",
        include_bytes!("../icons/to-front.svg"),
    ),
    ("icons/to-back.svg", include_bytes!("../icons/to-back.svg")),
    (
        "icons/rotate-cw.svg",
        include_bytes!("../icons/rotate-cw.svg"),
    ),
    ("icons/trash.svg", include_bytes!("../icons/trash.svg")),
    (
        "icons/signature.svg",
        include_bytes!("../icons/signature.svg"),
    ),
    (
        "icons/shield-check.svg",
        include_bytes!("../icons/shield-check.svg"),
    ),
    (
        "icons/shield-alert.svg",
        include_bytes!("../icons/shield-alert.svg"),
    ),
    ("icons/upload.svg", include_bytes!("../icons/upload.svg")),
    (
        "icons/format-squiggle.svg",
        include_bytes!("../icons/format-squiggle.svg"),
    ),
    (
        "icons/format-strike.svg",
        include_bytes!("../icons/format-strike.svg"),
    ),
    (
        "icons/text-edit.svg",
        include_bytes!("../icons/text-edit.svg"),
    ),
    ("icons/image.svg", include_bytes!("../icons/image.svg")),
    ("icons/redact.svg", include_bytes!("../icons/redact.svg")),
    ("icons/form.svg", include_bytes!("../icons/form.svg")),
    (
        "icons/water-drop.svg",
        include_bytes!("../icons/water-drop.svg"),
    ),
    (
        "icons/format-text.svg",
        include_bytes!("../icons/format-text.svg"),
    ),
    (
        "icons/format-underline.svg",
        include_bytes!("../icons/format-underline.svg"),
    ),
    (
        "icons/highlight.svg",
        include_bytes!("../icons/highlight.svg"),
    ),
    ("icons/ink.svg", include_bytes!("../icons/ink.svg")),
    ("icons/notes.svg", include_bytes!("../icons/notes.svg")),
    ("icons/pen.svg", include_bytes!("../icons/pen.svg")),
    (
        "icons/shape-arrow.svg",
        include_bytes!("../icons/shape-arrow.svg"),
    ),
    (
        "icons/shape-circle.svg",
        include_bytes!("../icons/shape-circle.svg"),
    ),
    (
        "icons/shape-line.svg",
        include_bytes!("../icons/shape-line.svg"),
    ),
    (
        "icons/shape-square.svg",
        include_bytes!("../icons/shape-square.svg"),
    ),
    ("icons/stamp.svg", include_bytes!("../icons/stamp.svg")),
    (
        "icons/chevron-down.svg",
        include_bytes!("../icons/chevron-down.svg"),
    ),
    (
        "icons/chevron-up.svg",
        include_bytes!("../icons/chevron-up.svg"),
    ),
    (
        "icons/chevron-left.svg",
        include_bytes!("../icons/chevron-left.svg"),
    ),
    (
        "icons/chevron-right.svg",
        include_bytes!("../icons/chevron-right.svg"),
    ),
    ("icons/check.svg", include_bytes!("../icons/check.svg")),
    ("icons/close.svg", include_bytes!("../icons/close.svg")),
    (
        "icons/contrast.svg",
        include_bytes!("../icons/contrast.svg"),
    ),
    (
        "icons/document.svg",
        include_bytes!("../icons/document.svg"),
    ),
    (
        "icons/fit-page.svg",
        include_bytes!("../icons/fit-page.svg"),
    ),
    (
        "icons/fit-width.svg",
        include_bytes!("../icons/fit-width.svg"),
    ),
    ("icons/folder.svg", include_bytes!("../icons/folder.svg")),
    (
        "icons/folders-pane.svg",
        include_bytes!("../icons/folders-pane.svg"),
    ),
    ("icons/lock.svg", include_bytes!("../icons/lock.svg")),
    ("icons/print.svg", include_bytes!("../icons/print.svg")),
    ("icons/search.svg", include_bytes!("../icons/search.svg")),
    ("icons/spread.svg", include_bytes!("../icons/spread.svg")),
    (
        "icons/view-grid.svg",
        include_bytes!("../icons/view-grid.svg"),
    ),
    ("icons/crop.svg", include_bytes!("../icons/crop.svg")),
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
