// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's library (docs/PLAN.md §4.6): a catalog of the PDFs in the
//! folders a person watches, with what each one is (title, author, pages,
//! signed, has a form, scanned), a small cover, tags, a rating and a
//! note, and every page's text for full-text search. Files that move are
//! found again by their contents; copies are found the same way.
//!
//! The catalog is one SQLite file with an FTS5 index of the pages' text,
//! kept in `$XDG_DATA_HOME/katna/pdf/library.sqlite`. Text and covers are
//! read through a renderer [`Engine`], so the app can read them in its
//! worker processes, out of harm's way of broken files.

mod filter;
mod scan;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

pub use filter::{Filter, Sort};
pub use katna_pdf_render::Engine;
pub use scan::{Indexed, ScanReport};

/// The catalog's layout. Bumped when it changes; an older catalog is
/// moved up in [`Library::open`].
const VERSION: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS folders (
    path TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS docs (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    hash TEXT NOT NULL,
    text_hash TEXT,
    title TEXT,
    author TEXT,
    subject TEXT,
    keywords TEXT,
    pages INTEGER NOT NULL,
    signed INTEGER NOT NULL DEFAULT 0,
    forms INTEGER NOT NULL DEFAULT 0,
    scanned INTEGER NOT NULL DEFAULT 0,
    locked INTEGER NOT NULL DEFAULT 0,
    added INTEGER NOT NULL,
    opened INTEGER,
    missing INTEGER NOT NULL DEFAULT 0,
    rating INTEGER NOT NULL DEFAULT 0,
    note TEXT NOT NULL DEFAULT '',
    cover BLOB,
    cover_width INTEGER,
    cover_height INTEGER
);
CREATE INDEX IF NOT EXISTS docs_hash ON docs(hash);
CREATE INDEX IF NOT EXISTS docs_text_hash ON docs(text_hash);
CREATE TABLE IF NOT EXISTS tags (
    doc INTEGER NOT NULL REFERENCES docs(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (doc, tag)
);
CREATE TABLE IF NOT EXISTS collections (
    name TEXT PRIMARY KEY,
    query TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS pages USING fts5(
    body,
    doc UNINDEXED,
    page UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
";

#[derive(Debug)]
pub enum LibraryError {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    /// There is no home folder to keep the catalog in.
    NoHome,
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::Sql(e) => write!(f, "the library's catalog: {e}"),
            LibraryError::Io(e) => e.fmt(f),
            LibraryError::NoHome => f.write_str("there is no home folder for the library"),
        }
    }
}

impl std::error::Error for LibraryError {}

impl From<rusqlite::Error> for LibraryError {
    fn from(e: rusqlite::Error) -> Self {
        LibraryError::Sql(e)
    }
}

impl From<std::io::Error> for LibraryError {
    fn from(e: std::io::Error) -> Self {
        LibraryError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, LibraryError>;

/// One PDF in the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub id: i64,
    pub path: PathBuf,
    pub size: u64,
    /// When the file last changed, in seconds since 1970.
    pub modified: i64,
    /// The SHA-256 of its bytes, in hex.
    pub hash: String,
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub pages: usize,
    pub signed: bool,
    pub forms: bool,
    /// Its pages are pictures with no text: a scan.
    pub scanned: bool,
    /// It needs a password to open.
    pub locked: bool,
    /// When it came into the library, and was last opened, in seconds
    /// since 1970.
    pub added: i64,
    pub opened: Option<i64>,
    /// The file is no longer where it was.
    pub missing: bool,
    /// From 0 (none) to 5 stars.
    pub rating: u8,
    pub note: String,
    pub tags: Vec<String>,
}

impl Doc {
    /// Its title, or else its file name.
    pub fn name(&self) -> String {
        match &self.title {
            Some(t) if !t.trim().is_empty() => t.trim().to_owned(),
            _ => self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// A small picture of a document's first page.
#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A page where a search's words are found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub doc: i64,
    /// The page, from 0.
    pub page: usize,
    /// The words around the match, the match itself between `[` and `]`.
    pub snippet: String,
}

/// A cover as kept: deflated pixels, width and height.
type CoverRow = (Option<Vec<u8>>, Option<u32>, Option<u32>);

/// The catalog.
pub struct Library {
    conn: Connection,
}

/// Seconds since 1970, now.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Library {
    /// The catalog at `path`, made when it is not there yet.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::from_connection(Connection::open(path)?)
    }

    /// The person's own catalog, in `$XDG_DATA_HOME/katna/pdf/`.
    pub fn open_default() -> Result<Self> {
        Self::open(&Self::default_path()?)
    }

    pub fn default_path() -> Result<PathBuf> {
        let paths = katna_core::paths::Paths::from_env().map_err(|_| LibraryError::NoHome)?;
        Ok(paths.data_dir().join("pdf").join("library.sqlite"))
    }

    /// A catalog kept only in memory, for tests.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.execute_batch(SCHEMA)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version < VERSION {
            conn.pragma_update(None, "user_version", VERSION)?;
        }
        Ok(Self { conn })
    }

    /// The folders watched, in order.
    pub fn folders(&self) -> Result<Vec<PathBuf>> {
        let mut st = self
            .conn
            .prepare("SELECT path FROM folders ORDER BY path")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// Watches `folder` from now on; [`Library::scan`] reads it.
    pub fn add_folder(&self, folder: &Path) -> Result<()> {
        let folder = std::fs::canonicalize(folder)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO folders (path) VALUES (?1)",
            [folder.to_string_lossy()],
        )?;
        Ok(())
    }

    /// Stops watching `folder`; its documents leave the catalog, with
    /// their tags and notes, unless another folder watched holds them.
    pub fn remove_folder(&self, folder: &Path) -> Result<()> {
        let folder = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        self.conn.execute(
            "DELETE FROM folders WHERE path = ?1",
            [folder.to_string_lossy()],
        )?;
        let others = self.folders()?;
        for doc in self.all_docs()? {
            if doc.path.starts_with(&folder) && !others.iter().any(|f| doc.path.starts_with(f)) {
                self.forget(doc.id)?;
            }
        }
        Ok(())
    }

    /// Takes document `id` out of the catalog.
    pub fn forget(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM pages WHERE doc = ?1", [id])?;
        self.conn.execute("DELETE FROM docs WHERE id = ?1", [id])?;
        Ok(())
    }

    fn all_docs(&self) -> Result<Vec<Doc>> {
        self.docs(&Filter::default())
    }

    /// The documents `filter` lets through, in its order.
    pub fn docs(&self, filter: &Filter) -> Result<Vec<Doc>> {
        let (sql, args) = filter.sql();
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter()), doc_row)?;
        let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        // By the name shown: SQLite has no file name of a path to sort by.
        if filter.sort == Sort::Title {
            out.sort_by_cached_key(|d| d.name().to_lowercase());
        }
        let mut tags = self
            .conn
            .prepare("SELECT tag FROM tags WHERE doc = ?1 ORDER BY tag")?;
        for doc in &mut out {
            doc.tags = tags
                .query_map([doc.id], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()?;
        }
        Ok(out)
    }

    /// Document `id`.
    pub fn doc(&self, id: i64) -> Result<Option<Doc>> {
        let filter = Filter {
            id: Some(id),
            ..Filter::default()
        };
        Ok(self.docs(&filter)?.into_iter().next())
    }

    /// The document at `path`, when it is in the catalog.
    pub fn doc_at(&self, path: &Path) -> Result<Option<i64>> {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM docs WHERE path = ?1",
                [path.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// The cover of document `id`, when one was drawn.
    pub fn cover(&self, id: i64) -> Result<Option<Cover>> {
        let row: Option<CoverRow> = self
            .conn
            .query_row(
                "SELECT cover, cover_width, cover_height FROM docs WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((Some(packed), Some(width), Some(height))) = row else {
            return Ok(None);
        };
        let mut rgba = Vec::new();
        use std::io::Read as _;
        flate2::read::DeflateDecoder::new(&packed[..]).read_to_end(&mut rgba)?;
        if rgba.len() != (width as usize) * (height as usize) * 4 {
            return Ok(None);
        }
        Ok(Some(Cover {
            width,
            height,
            rgba,
        }))
    }

    /// Notes that document `id` was opened now.
    pub fn mark_opened(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE docs SET opened = ?2 WHERE id = ?1",
            params![id, now()],
        )?;
        Ok(())
    }

    /// Gives document `id` exactly `tags` (trimmed, without repeats).
    pub fn set_tags(&self, id: i64, tags: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tags WHERE doc = ?1", [id])?;
        for tag in tags {
            let tag = tag.trim();
            if !tag.is_empty() {
                tx.execute(
                    "INSERT OR IGNORE INTO tags (doc, tag) VALUES (?1, ?2)",
                    params![id, tag],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every tag in use, with how many documents have it.
    pub fn tags(&self) -> Result<Vec<(String, usize)>> {
        let mut st = self
            .conn
            .prepare("SELECT tag, COUNT(*) FROM tags GROUP BY tag ORDER BY tag COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Gives document `id` a rating from 0 to 5 stars.
    pub fn set_rating(&self, id: i64, rating: u8) -> Result<()> {
        self.conn.execute(
            "UPDATE docs SET rating = ?2 WHERE id = ?1",
            params![id, rating.min(5)],
        )?;
        Ok(())
    }

    pub fn set_note(&self, id: i64, note: &str) -> Result<()> {
        self.conn
            .execute("UPDATE docs SET note = ?2 WHERE id = ?1", params![id, note])?;
        Ok(())
    }

    /// Saves `query` (as [`Filter::parse`] reads it) as smart collection
    /// `name`; an empty query takes the collection away.
    pub fn set_collection(&self, name: &str, query: &str) -> Result<()> {
        if query.trim().is_empty() {
            self.conn
                .execute("DELETE FROM collections WHERE name = ?1", [name])?;
        } else {
            self.conn.execute(
                "INSERT INTO collections (name, query) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET query = excluded.query",
                params![name.trim(), query.trim()],
            )?;
        }
        Ok(())
    }

    /// The smart collections, by name, with their queries.
    pub fn collections(&self) -> Result<Vec<(String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT name, query FROM collections ORDER BY name COLLATE NOCASE")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The pages whose text holds `query` (FTS5 words: `invoice`,
    /// `"exact words"`, `rent OR lease`, `tax*`), best first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        let query = fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT doc, page, snippet(pages, 0, '[', ']', '…', 12) FROM pages
             WHERE pages MATCH ?1 ORDER BY rank LIMIT ?2",
        )?;
        let rows = st.query_map(params![query, limit as i64], |r| {
            Ok(Hit {
                doc: r.get(0)?,
                page: r.get::<_, i64>(1)? as usize,
                snippet: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The documents whose text holds `query`, as [`Library::search`]
    /// reads it.
    pub fn search_docs(&self, query: &str) -> Result<Vec<i64>> {
        let query = fts_query(query);
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT doc FROM pages WHERE pages MATCH ?1")?;
        let rows = st.query_map([query], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Groups of documents that are copies of each other: the same bytes,
    /// or the same text (the same words, saved again by another program).
    pub fn duplicates(&self) -> Result<Vec<Vec<Doc>>> {
        let docs = self.all_docs()?;
        let text_hashes: Vec<(i64, Option<String>)> = {
            let mut st = self.conn.prepare("SELECT id, text_hash FROM docs")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let text_of = |id: i64| {
            text_hashes
                .iter()
                .find(|(d, _)| *d == id)
                .and_then(|(_, h)| h.clone())
        };
        // Joined into groups: two documents meet when either key matches.
        let mut group: Vec<usize> = (0..docs.len()).collect();
        fn root(group: &mut [usize], mut i: usize) -> usize {
            while group[i] != i {
                group[i] = group[group[i]];
                i = group[i];
            }
            i
        }
        let mut by_key: std::collections::HashMap<String, usize> = Default::default();
        for (i, doc) in docs.iter().enumerate() {
            let keys = [
                Some(format!("b{}", doc.hash)),
                text_of(doc.id).map(|h| format!("t{h}")),
            ];
            for key in keys.into_iter().flatten() {
                match by_key.get(&key) {
                    Some(&j) => {
                        let (a, b) = (root(&mut group, i), root(&mut group, j));
                        group[a] = b;
                    }
                    None => {
                        by_key.insert(key, i);
                    }
                }
            }
        }
        let mut groups: std::collections::BTreeMap<usize, Vec<Doc>> = Default::default();
        for (i, doc) in docs.iter().enumerate() {
            let r = root(&mut group, i);
            groups.entry(r).or_default().push(doc.clone());
        }
        Ok(groups.into_values().filter(|g| g.len() > 1).collect())
    }
}

/// What a person types, as an FTS5 query: words kept as they are when
/// the query is already FTS5 (quotes, OR, NOT, `*`), each word quoted
/// otherwise so that punctuation does not upset the parser.
fn fts_query(query: &str) -> String {
    let query = query.trim();
    let advanced = query.contains('"')
        || query.contains('*')
        || query
            .split_whitespace()
            .any(|w| w == "OR" || w == "NOT" || w == "AND");
    if advanced {
        return query.to_owned();
    }
    query
        .split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn doc_row(r: &rusqlite::Row) -> rusqlite::Result<Doc> {
    Ok(Doc {
        id: r.get("id")?,
        path: PathBuf::from(r.get::<_, String>("path")?),
        size: r.get::<_, i64>("size")? as u64,
        modified: r.get("modified")?,
        hash: r.get("hash")?,
        title: r.get("title")?,
        author: r.get("author")?,
        subject: r.get("subject")?,
        keywords: r.get("keywords")?,
        pages: r.get::<_, i64>("pages")? as usize,
        signed: r.get("signed")?,
        forms: r.get("forms")?,
        scanned: r.get("scanned")?,
        locked: r.get("locked")?,
        added: r.get("added")?,
        opened: r.get("opened")?,
        missing: r.get("missing")?,
        rating: r.get::<_, i64>("rating")?.clamp(0, 5) as u8,
        note: r.get("note")?,
        tags: Vec::new(),
    })
}
