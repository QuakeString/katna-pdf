// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether the app closed unexpectedly last time (S11): a mark is left in
//! the state folder while it runs and taken away when it quits, so a
//! mark found at start means the last run never quit. The start screen
//! then offers the files that were open back. No GPUI here.

use std::path::{Path, PathBuf};

/// The last run closed unexpectedly; unset once the person answers.
pub struct Crashed(pub bool);

impl gpui::Global for Crashed {}

fn mark() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("running"))
}

/// Notes that the app runs, and says whether the last run closed
/// unexpectedly. A run ended from outside (a logout, `kill`) never quits
/// either, so a mark left behind counts only with signs of a crash: a
/// report of one since the mark, or changes the run kept and never
/// saved (`left_changes`).
pub fn started(left_changes: bool) -> bool {
    let Some(mark) = mark() else {
        return false;
    };
    let since = std::fs::metadata(&mark)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    let reported = since.is_some_and(|since| {
        reports().is_some_and(|dir| {
            katna_core::crash::reports(&dir)
                .iter()
                .any(|r| r.app == "katna-pdf" && r.unix_time().is_some_and(|t| t >= since))
        })
    });
    started_at(&mark) && (reported || left_changes)
}

fn started_at(mark: &Path) -> bool {
    let left = mark.is_file();
    if let Some(dir) = mark.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(mark, b"");
    left
}

/// Notes that the app quit as it should.
pub fn quit() {
    if let Some(mark) = mark() {
        let _ = std::fs::remove_file(mark);
    }
}

/// Katna's crash folder, where a report of the last crash would be.
pub fn reports() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.crash_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_left_behind_means_the_last_run_never_quit() {
        let dir = std::env::temp_dir().join(format!("katna-pdf-crashed-{}", std::process::id()));
        let mark = dir.join("pdf").join("running");
        assert!(!started_at(&mark));
        assert!(started_at(&mark));
        std::fs::remove_file(&mark).unwrap();
        assert!(!started_at(&mark));
        let _ = std::fs::remove_dir_all(dir);
    }
}
