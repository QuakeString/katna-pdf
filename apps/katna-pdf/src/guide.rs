// SPDX-License-Identifier: GPL-3.0-or-later

//! The user guide (Help > User guide): `docs/guide.md`, built in, made
//! into a PDF with its headings as bookmarks and the keyboard list from
//! the app's own menus at the end, and opened in a tab like any file.

use std::path::PathBuf;

use crate::menus::{self, MENUS, MORE_KEYS};
use crate::text::t;

const GUIDE: &str = include_str!("../../../docs/guide.md");

/// The guide's lines for [`katna_pdf_cos::text_pdf_outlined`]: headings
/// flush left, paragraphs and bullets indented.
fn lines() -> Vec<String> {
    let mut out = Vec::new();
    let mut comment = false;
    for line in GUIDE.lines() {
        let line = line.trim_end();
        if comment || line.starts_with("<!--") {
            comment = !line.ends_with("-->");
            continue;
        }
        if line.starts_with("# ") {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            out.push(heading.to_owned());
        } else if let Some(item) = line.strip_prefix("- ") {
            out.push(format!("    \u{2022} {item}"));
        } else if line.is_empty() {
            if out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
        } else {
            out.push(format!("  {line}"));
        }
    }
    out.push(t("guide-keys", &[]));
    for (menu, entries) in MENUS {
        out.push(format!("  {}", menus::plain(&t(menu, &[]))));
        for (id, keys) in entries.iter().flatten() {
            if !keys.is_empty() {
                let words = menus::plain(&t(id, &[]));
                out.push(format!("    {words}: {}", latin(&menus::keys_text(keys))));
            }
        }
    }
    out.push(format!("  {}", t("guide-more-keys", &[])));
    for (id, keys) in MORE_KEYS {
        out.push(format!("    {}: {}", t(id, &[]), latin(keys)));
    }
    out
}

/// Arrows the guide's standard fonts lack, as words.
fn latin(keys: &str) -> String {
    keys.replace('\u{2190}', "Left")
        .replace('\u{2192}', "Right")
        .replace('\u{2191}', "Up")
        .replace('\u{2193}', "Down")
}

/// Writes the guide to the cache folder and gives its path.
pub fn write() -> Result<PathBuf, String> {
    let dir = katna_core::Paths::from_env()
        .map_err(|e| e.to_string())?
        .cache_dir()
        .join("pdf")
        .join("guide");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let title = t("guide-title", &[]);
    let path = dir.join(format!("{title}.pdf"));
    let pdf = katna_pdf_cos::text_pdf_outlined(&title, &lines());
    std::fs::write(&path, pdf).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_has_its_headings_and_keys() {
        let lines = lines();
        assert!(lines.iter().any(|l| l == "Opening files"));
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("<!--") || l.contains("-->"))
        );
        assert!(lines.iter().any(|l| l.contains("Ctrl O")));
        // Every line can be drawn in the standard fonts.
        for line in &lines {
            assert!(
                line.chars().all(|c| (c as u32) < 0x100 || c == '\u{2022}'),
                "{line}"
            );
        }
    }
}
