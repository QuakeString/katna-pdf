// SPDX-License-Identifier: GPL-3.0-or-later

//! Work on many documents at once: merged into one file, renamed from
//! their properties, copied to another folder.

use std::path::{Path, PathBuf};

use katna_pdf_cos::{Document, PageSource};

use crate::{Doc, Library, Result};

/// What a pattern's `{name}`s are read from, as the rename card lists
/// them.
pub const PATTERN_KEYS: [&str; 8] = [
    "title", "author", "subject", "name", "year", "date", "pages", "n",
];

/// The file name `pattern` gives `doc`, the `n`th of those renamed
/// (from 1), without `.pdf`: `{title}`, `{author}`, `{subject}`, `{name}`
/// (the file's name now), `{year}` and `{date}` (when it last changed),
/// `{pages}` and `{n}`. A property it lacks reads as nothing, and what
/// a file name cannot hold is left out.
pub fn name_from(doc: &Doc, pattern: &str, n: usize) -> String {
    let stem = doc
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (year, month, day) = civil(doc.modified);
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            rest = &rest[open..];
            break;
        };
        let key = &rest[open + 1..open + close];
        let text = |v: &Option<String>| v.as_deref().unwrap_or("").trim().to_owned();
        let value = match key.to_ascii_lowercase().as_str() {
            "title" => text(&doc.title),
            "author" => text(&doc.author),
            "subject" => text(&doc.subject),
            "name" => stem.clone(),
            "year" => year.to_string(),
            "date" => format!("{year:04}-{month:02}-{day:02}"),
            "pages" => doc.pages.to_string(),
            "n" => n.to_string(),
            _ => rest[open..=open + close].to_owned(),
        };
        out.push_str(&value);
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    let clean = clean(&out);
    if clean.is_empty() { stem } else { clean }
}

/// `name` as a file name on every system: no separators or characters
/// Windows refuses, spaces run together, no dots or dashes left dangling.
fn clean(name: &str) -> String {
    let kept: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let words: Vec<&str> = kept.split_whitespace().collect();
    let joined = words.join(" ");
    let trimmed = joined.trim_matches(|c: char| c == '.' || c == '-' || c == '_' || c == ' ');
    trimmed
        .chars()
        .take(180)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The year, month and day of `secs` after 1970, in UTC.
fn civil(secs: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil.
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `folder/stem.pdf`, or `stem (2).pdf` and so on when that is taken.
/// `except` is a file that may keep its own name.
pub fn free_path(folder: &Path, stem: &str, except: Option<&Path>) -> PathBuf {
    let mut n = 1;
    loop {
        let name = if n == 1 {
            format!("{stem}.pdf")
        } else {
            format!("{stem} ({n}).pdf")
        };
        let path = folder.join(name);
        if except == Some(path.as_path()) || !path.exists() {
            return path;
        }
        n += 1;
    }
}

/// What happened to each document of a bulk change.
#[derive(Debug, Default)]
pub struct BulkReport {
    /// The documents changed, and where each is now.
    pub done: Vec<(i64, PathBuf)>,
    /// The documents left as they were, and why.
    pub failed: Vec<(PathBuf, String)>,
}

impl Library {
    /// Renames each of `ids` by `pattern` ([`name_from`]), in its own
    /// folder, and keeps the catalog with it. A name already taken gets
    /// a number.
    pub fn rename(&self, ids: &[i64], pattern: &str) -> Result<BulkReport> {
        let mut report = BulkReport::default();
        for (i, &id) in ids.iter().enumerate() {
            let Some(doc) = self.doc(id)? else { continue };
            let folder = doc.path.parent().unwrap_or(Path::new("")).to_path_buf();
            let stem = name_from(&doc, pattern, i + 1);
            let to = free_path(&folder, &stem, Some(&doc.path));
            if to == doc.path {
                report.done.push((id, to));
                continue;
            }
            match std::fs::rename(&doc.path, &to) {
                Ok(()) => {
                    self.conn.execute(
                        "UPDATE docs SET path = ?1 WHERE id = ?2",
                        rusqlite::params![to.to_string_lossy(), id],
                    )?;
                    report.done.push((id, to));
                }
                Err(e) => report.failed.push((doc.path, e.to_string())),
            }
        }
        Ok(report)
    }

    /// Copies each of `ids` into `folder`, a name already there getting
    /// a number. The copies come into the library when `folder` is
    /// watched.
    pub fn copy_to(&self, ids: &[i64], folder: &Path) -> Result<BulkReport> {
        std::fs::create_dir_all(folder)?;
        let mut report = BulkReport::default();
        for &id in ids {
            let Some(doc) = self.doc(id)? else { continue };
            let stem = doc
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "document".into());
            let to = free_path(folder, &stem, None);
            match std::fs::copy(&doc.path, &to) {
                Ok(_) => report.done.push((id, to)),
                Err(e) => report.failed.push((doc.path, e.to_string())),
            }
        }
        Ok(report)
    }

    /// Gives each of `ids` the tag `tag` too.
    pub fn add_tag(&self, ids: &[i64], tag: &str) -> Result<()> {
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        for &id in ids {
            let Some(doc) = self.doc(id)? else { continue };
            if doc.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                continue;
            }
            let mut tags = doc.tags;
            tags.push(tag.to_owned());
            self.set_tags(id, &tags)?;
        }
        Ok(())
    }
}

/// One file of `paths`' pages in order, written to `out`: their pages
/// counted. The first file's outline and properties are kept.
pub fn merge(paths: &[PathBuf], out: &Path) -> std::result::Result<usize, String> {
    let mut docs = Vec::with_capacity(paths.len());
    for path in paths {
        let name = || {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", name()))?;
        let doc = Document::load(bytes).map_err(|e| format!("{}: {e}", name()))?;
        docs.push(doc);
    }
    let sources: Vec<PageSource> = docs
        .iter()
        .flat_map(|doc| {
            (0..doc.pages().len()).map(move |page| PageSource {
                doc,
                page,
                turns: 0,
                crop: None,
            })
        })
        .collect();
    let bytes = katna_pdf_cos::compose(docs.first(), &sources).map_err(|e| e.to_string())?;
    let partial = out.with_extension("pdf.part");
    std::fs::write(&partial, &bytes)
        .and_then(|()| std::fs::rename(&partial, out))
        .map_err(|e| {
            let _ = std::fs::remove_file(&partial);
            e.to_string()
        })?;
    Ok(sources.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Doc {
        Doc {
            id: 1,
            path: PathBuf::from("/x/scan 0012.pdf"),
            size: 0,
            // 2024-03-05 10:00 UTC.
            modified: 1_709_632_800,
            hash: String::new(),
            title: Some("  Rent: March / April ".into()),
            author: Some("A. Rao".into()),
            subject: None,
            keywords: None,
            pages: 3,
            signed: false,
            forms: false,
            scanned: false,
            locked: false,
            added: 0,
            opened: None,
            missing: false,
            rating: 0,
            note: String::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn patterns_fill_in_properties() {
        let d = doc();
        assert_eq!(
            name_from(&d, "{author} - {title}", 1),
            "A. Rao - Rent March April"
        );
        assert_eq!(name_from(&d, "{date} {name}", 1), "2024-03-05 scan 0012");
        assert_eq!(name_from(&d, "{year}-{n} ({pages}p)", 7), "2024-7 (3p)");
        // Nothing to go on: the name it had.
        assert_eq!(name_from(&d, "{subject}", 1), "scan 0012");
        // Unknown keys are kept as written; a lone brace too.
        assert_eq!(name_from(&d, "{what} {", 1), "{what} {");
    }

    #[test]
    fn days_become_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(951_782_400), (2000, 2, 29));
        assert_eq!(civil(-86_400), (1969, 12, 31));
    }
}
