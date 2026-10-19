// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading the watched folders into the catalog: new and changed PDFs
//! indexed, moved ones found again by their bytes, and those no longer
//! there marked missing (kept, with their tags and notes, until they come
//! back or are forgotten).

use std::collections::HashSet;
use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use katna_pdf_render::{Engine, OpenError};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::{Library, Result, now};

/// How wide covers are drawn, in pixels.
const COVER_WIDTH: f32 = 160.0;
/// Text read from at most this many pages of one file.
const MAX_TEXT_PAGES: usize = 5_000;
/// Fewer letters than this on its first pages, and pictures on them: a
/// scan.
const SCAN_LETTERS: usize = 16;
const SCAN_PAGES: usize = 3;

/// What [`Library::index_file`] did with a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indexed {
    /// New to the catalog.
    Added(i64),
    /// Changed since it was read, and read again.
    Updated(i64),
    /// A document missing from where it was, found here by its bytes.
    Moved(i64),
    /// As it was.
    Unchanged(i64),
}

impl Indexed {
    pub fn id(self) -> i64 {
        match self {
            Indexed::Added(id)
            | Indexed::Updated(id)
            | Indexed::Moved(id)
            | Indexed::Unchanged(id) => id,
        }
    }
}

/// What a [`Library::scan`] found.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub moved: usize,
    pub unchanged: usize,
    /// Documents no longer found.
    pub missing: usize,
    /// Files that could not be read, and why.
    pub failed: Vec<(PathBuf, String)>,
}

impl Library {
    /// Reads every PDF in the watched folders (and the folders in them),
    /// calling `progress` with how many are done, of how many, before each.
    pub fn scan(
        &self,
        engine: &dyn Engine,
        progress: &mut dyn FnMut(usize, usize, &Path),
    ) -> Result<ScanReport> {
        let mut report = ScanReport::default();
        let mut files = Vec::new();
        for folder in self.folders()? {
            walk(&folder, &mut files, &mut HashSet::new(), 0);
        }
        files.sort();
        files.dedup();
        // Gone files first, so that a moved one is found again below.
        report.missing = self.mark_missing()?;
        for (i, file) in files.iter().enumerate() {
            progress(i, files.len(), file);
            match self.index_file(engine, file) {
                Ok(Indexed::Added(_)) => report.added += 1,
                Ok(Indexed::Updated(_)) => report.updated += 1,
                Ok(Indexed::Moved(_)) => {
                    report.moved += 1;
                    report.missing = report.missing.saturating_sub(1);
                }
                Ok(Indexed::Unchanged(_)) => report.unchanged += 1,
                Err(e) => report.failed.push((file.clone(), e.to_string())),
            }
        }
        progress(files.len(), files.len(), Path::new(""));
        Ok(report)
    }

    /// Marks the documents whose files are gone as missing; how many newly.
    fn mark_missing(&self) -> Result<usize> {
        let mut gone = Vec::new();
        {
            let mut st = self
                .conn
                .prepare("SELECT id, path FROM docs WHERE missing = 0")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, path) = row?;
                if !Path::new(&path).is_file() {
                    gone.push(id);
                }
            }
        }
        for id in &gone {
            self.conn
                .execute("UPDATE docs SET missing = 1 WHERE id = ?1", [id])?;
        }
        Ok(gone.len())
    }

    /// Reads one PDF into the catalog, or again when it changed.
    pub fn index_file(&self, engine: &dyn Engine, path: &Path) -> Result<Indexed> {
        let path = std::fs::canonicalize(path)?;
        let key = path.to_string_lossy().into_owned();
        let meta = std::fs::metadata(&path)?;
        let size = meta.len() as i64;
        let modified = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let known: Option<(i64, i64, i64, String, bool)> = self
            .conn
            .query_row(
                "SELECT id, size, modified, hash, missing FROM docs WHERE path = ?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        if let Some((id, s, m, _, missing)) = &known
            && *s == size
            && *m == modified
        {
            if *missing {
                self.conn
                    .execute("UPDATE docs SET missing = 0 WHERE id = ?1", [id])?;
            }
            return Ok(Indexed::Unchanged(*id));
        }
        let bytes = std::fs::read(&path)?;
        let hash = hex(&Sha256::digest(&bytes));
        if let Some((id, _, _, old, _)) = &known
            && *old == hash
        {
            // Touched, not changed.
            self.conn.execute(
                "UPDATE docs SET size = ?2, modified = ?3, missing = 0 WHERE id = ?1",
                params![id, size, modified],
            )?;
            return Ok(Indexed::Unchanged(*id));
        }
        if known.is_none() {
            let moved: Option<i64> = self
                .conn
                .query_row(
                    "SELECT id FROM docs WHERE hash = ?1 AND missing = 1 LIMIT 1",
                    [&hash],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = moved {
                self.conn.execute(
                    "UPDATE docs SET path = ?2, size = ?3, modified = ?4, missing = 0 WHERE id = ?1",
                    params![id, key, size, modified],
                )?;
                return Ok(Indexed::Moved(id));
            }
        }

        let facts = read(engine, bytes);
        let tx = self.conn.unchecked_transaction()?;
        let id = match &known {
            Some((id, ..)) => {
                tx.execute(
                    "UPDATE docs SET size = ?2, modified = ?3, hash = ?4, text_hash = ?5,
                     title = ?6, author = ?7, subject = ?8, keywords = ?9, pages = ?10,
                     signed = ?11, forms = ?12, scanned = ?13, locked = ?14, missing = 0,
                     cover = ?15, cover_width = ?16, cover_height = ?17
                     WHERE id = ?1",
                    params![
                        id,
                        size,
                        modified,
                        hash,
                        facts.text_hash,
                        facts.title,
                        facts.author,
                        facts.subject,
                        facts.keywords,
                        facts.pages as i64,
                        facts.signed,
                        facts.forms,
                        facts.scanned,
                        facts.locked,
                        facts.cover.as_ref().map(|c| &c.0),
                        facts.cover.as_ref().map(|c| c.1),
                        facts.cover.as_ref().map(|c| c.2),
                    ],
                )?;
                *id
            }
            None => {
                tx.execute(
                    "INSERT INTO docs (path, size, modified, hash, text_hash, title, author,
                     subject, keywords, pages, signed, forms, scanned, locked, added,
                     cover, cover_width, cover_height)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                     ?15, ?16, ?17, ?18)",
                    params![
                        key,
                        size,
                        modified,
                        hash,
                        facts.text_hash,
                        facts.title,
                        facts.author,
                        facts.subject,
                        facts.keywords,
                        facts.pages as i64,
                        facts.signed,
                        facts.forms,
                        facts.scanned,
                        facts.locked,
                        now(),
                        facts.cover.as_ref().map(|c| &c.0),
                        facts.cover.as_ref().map(|c| c.1),
                        facts.cover.as_ref().map(|c| c.2),
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.execute("DELETE FROM pages WHERE doc = ?1", [id])?;
        {
            let mut st = tx.prepare("INSERT INTO pages (body, doc, page) VALUES (?1, ?2, ?3)")?;
            for (page, body) in facts.text.iter().enumerate() {
                if !body.trim().is_empty() {
                    st.execute(params![body, id, page as i64])?;
                }
            }
        }
        tx.commit()?;
        Ok(match known {
            Some(_) => Indexed::Updated(id),
            None => Indexed::Added(id),
        })
    }
}

/// What a file says about itself.
#[derive(Default)]
struct Facts {
    title: Option<String>,
    author: Option<String>,
    subject: Option<String>,
    keywords: Option<String>,
    pages: usize,
    signed: bool,
    forms: bool,
    scanned: bool,
    locked: bool,
    /// Each page's text.
    text: Vec<String>,
    text_hash: Option<String>,
    /// The cover, deflated, and its width and height.
    cover: Option<(Vec<u8>, u32, u32)>,
}

fn read(engine: &dyn Engine, bytes: Vec<u8>) -> Facts {
    let mut facts = Facts::default();
    let bytes = Arc::new(bytes);
    if let Ok(doc) = katna_pdf_cos::Document::load(bytes.to_vec()) {
        facts.locked = doc.is_locked();
        facts.pages = doc.pages().len();
        if !facts.locked {
            let meta = doc.metadata();
            facts.title = meta.title;
            facts.author = meta.author;
            facts.subject = meta.subject;
            facts.keywords = meta.keywords;
            facts.signed = doc.signatures().iter().any(|s| !s.contents.is_empty());
            facts.forms = !doc.form().is_empty();
        }
    }
    // A broken file must not take the scan down with it.
    let drawn = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let doc = match engine.open(bytes, "") {
            Ok(doc) => doc,
            Err(OpenError::Locked) => return (None, Vec::new(), false, true, None),
            Err(_) => return (None, Vec::new(), false, false, None),
        };
        let count = doc.page_count();
        let text: Vec<String> = (0..count.min(MAX_TEXT_PAGES))
            .map(|p| doc.text(p).text)
            .collect();
        let first: usize = text
            .iter()
            .take(SCAN_PAGES)
            .map(|t| t.chars().filter(|c| c.is_alphanumeric()).count())
            .sum();
        let scanned = count > 0
            && first < SCAN_LETTERS
            && (0..count.min(SCAN_PAGES)).any(|p| !doc.pictures(p, 0.25).is_empty());
        let cover = doc.page_size(0).and_then(|(w, _)| {
            let bitmap = doc.render(0, COVER_WIDTH / w.max(1.0))?;
            let mut packed =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            packed.write_all(&bitmap.rgba).ok()?;
            Some((packed.finish().ok()?, bitmap.width, bitmap.height))
        });
        (Some(count), text, scanned, false, cover)
    }));
    if let Ok((count, text, scanned, locked, cover)) = drawn {
        if let Some(count) = count {
            facts.pages = count;
        }
        facts.locked |= locked;
        facts.scanned = scanned;
        facts.text_hash = text_hash(&text);
        facts.text = text;
        facts.cover = cover;
    }
    facts
}

/// The hash of a document's words, lower case, without spacing or
/// punctuation: the same for a copy saved again by another program.
/// None for a document with too few words to tell.
fn text_hash(pages: &[String]) -> Option<String> {
    let mut hasher = Sha256::new();
    let mut words = 0;
    for page in pages {
        for word in page
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
        {
            hasher.update(word.to_lowercase().as_bytes());
            hasher.update(b" ");
            words += 1;
        }
    }
    (words >= 20).then(|| hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The PDFs in `dir` and the folders in it, hidden ones left out, links
/// to folders not followed.
fn walk(dir: &Path, out: &mut Vec<PathBuf>, seen: &mut HashSet<PathBuf>, depth: usize) {
    if depth > 32 || !seen.insert(dir.to_path_buf()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, out, seen, depth + 1);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
            && (kind.is_file() || path.is_file())
        {
            out.push(path);
        }
    }
}
