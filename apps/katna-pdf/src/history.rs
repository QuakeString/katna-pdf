// SPDX-License-Identifier: GPL-3.0-or-later

//! Recent files and reading progress (docs/PLAN.md, Phase 1): the files
//! opened lately, newest first, each with the page last read, so the
//! start screen lists them and a file opens again where it was left.
//!
//! Kept in `$XDG_STATE_HOME/katna/pdf/recent.tsv`, one file a line: the
//! page, a tab, the path. The files open at the end of the last session
//! are kept beside it in `session.txt`, a path a line, to open again.

use std::path::{Path, PathBuf};

use gpui::{App, Global};

/// Files remembered; older ones are forgotten.
const KEEP: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// The page last read, from 0.
    pub page: usize,
}

#[derive(Debug, Default)]
pub struct History {
    entries: Vec<Entry>,
    /// Where it is kept; none when there is no home folder.
    file: Option<PathBuf>,
    /// The files open when the app last ran, as read at the start.
    last_session: Vec<PathBuf>,
    /// The files open now, as last written.
    session: Vec<PathBuf>,
}

impl Global for History {}

impl History {
    /// Reads the history kept, leaving out files no longer there.
    pub fn load() -> Self {
        let file = katna_core::paths::Paths::from_env()
            .ok()
            .map(|paths| paths.state_dir().join("pdf").join("recent.tsv"));
        let entries = file
            .as_ref()
            .and_then(|file| std::fs::read_to_string(file).ok())
            .map(|text| parse(&text))
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.path.is_file())
            .collect();
        let last_session = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(session_file(f)).ok())
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.is_file())
                    .collect()
            })
            .unwrap_or_default();
        Self {
            entries,
            file,
            last_session,
            session: Vec::new(),
        }
    }

    /// The files open when the app last ran.
    pub fn last_session(&self) -> &[PathBuf] {
        &self.last_session
    }

    /// The files open now: kept, so they can be opened again next time.
    pub fn session_is(files: Vec<PathBuf>, cx: &mut App) {
        let history = cx.default_global::<Self>();
        if history.session == files {
            return;
        }
        history.session = files;
        let Some(file) = history.file.as_ref().map(|f| session_file(f)) else {
            return;
        };
        let text: String = history
            .session
            .iter()
            .filter_map(|p| p.to_str().filter(|p| !p.contains(['\n', '\r'])))
            .map(|p| format!("{p}\n"))
            .collect();
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = save(&file, &text) {
                    eprintln!("katna-pdf: cannot save {}: {err}", file.display());
                }
            })
            .detach();
    }

    /// The files opened lately, newest first.
    pub fn recent(&self) -> &[Entry] {
        &self.entries
    }

    /// The page `path` was left at.
    pub fn page(&self, path: &Path) -> Option<usize> {
        self.entries.iter().find(|e| e.path == path).map(|e| e.page)
    }

    /// `path` is open at `page`: it moves to the top. Returns whether
    /// anything changed.
    fn note(&mut self, path: &Path, page: usize) -> bool {
        if self
            .entries
            .first()
            .is_some_and(|e| e.path == path && e.page == page)
        {
            return false;
        }
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            Entry {
                path: path.to_owned(),
                page,
            },
        );
        self.entries.truncate(KEEP);
        true
    }

    /// `path` is open at `page`; the history is saved on a background
    /// thread.
    pub fn opened(path: &Path, page: usize, cx: &mut App) {
        let history = cx.default_global::<Self>();
        if !history.note(path, page) {
            return;
        }
        let Some(file) = history.file.clone() else {
            return;
        };
        let text = write(&history.entries);
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = save(&file, &text) {
                    eprintln!("katna-pdf: cannot save {}: {err}", file.display());
                }
            })
            .detach();
    }
}

fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let (page, path) = line.split_once('\t')?;
            Some(Entry {
                path: PathBuf::from(path),
                page: page.parse().ok()?,
            })
        })
        .collect()
}

fn write(entries: &[Entry]) -> String {
    entries
        .iter()
        .filter_map(|e| {
            // A path that cannot be written on one line is not kept.
            let path = e.path.to_str().filter(|p| !p.contains(['\n', '\r']))?;
            Some(format!("{}\t{path}\n", e.page))
        })
        .collect()
}

/// Where the session is kept, beside the history.
fn session_file(history: &Path) -> PathBuf {
    history.with_file_name("session.txt")
}

/// Writes `text` to `file` whole: to a file beside it, then moved over.
fn save(file: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let new = file.with_extension("new");
    std::fs::write(&new, text)?;
    std::fs::rename(&new, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_opened_comes_first_with_its_page() {
        let mut history = History::default();
        assert!(history.note(Path::new("/a.pdf"), 0));
        assert!(history.note(Path::new("/b b.pdf"), 3));
        assert!(history.note(Path::new("/a.pdf"), 7));
        assert!(!history.note(Path::new("/a.pdf"), 7));
        assert_eq!(history.page(Path::new("/b b.pdf")), Some(3));
        let text = write(history.recent());
        assert_eq!(text, "7\t/a.pdf\n3\t/b b.pdf\n");
        assert_eq!(parse(&text), history.recent());
        assert_eq!(parse("x\t/c.pdf\nno tab\n"), []);
    }

    #[test]
    fn saving_writes_the_whole_file() {
        let dir = std::env::temp_dir().join(format!("katna-pdf-history-{}", std::process::id()));
        let file = dir.join("pdf").join("recent.tsv");
        save(&file, "1\t/a.pdf\n").unwrap();
        save(&file, "2\t/b.pdf\n").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "2\t/b.pdf\n");
        std::fs::remove_dir_all(dir).ok();
    }
}
