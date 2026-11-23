// SPDX-License-Identifier: GPL-3.0-or-later

//! Copies of changes not yet saved, kept every minute or few (Settings > Editing) in the state
//! folder (`pdf/recovery/`), so a crash or a power cut loses at most a
//! minute or two. Each file changed has its copy (`NAME.pdf`, the file as
//! changed) and the path it belongs to (`NAME.txt`). A copy goes when its
//! file is saved, reverted or closed on purpose; one left when the app
//! starts is offered back on the start screen.
//!
//! The copy is the file's own bytes with its changes, so a file with a
//! password stays locked in its copy too.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// How often the app looks whether changes are due a copy, in seconds;
/// Settings > Editing says every how many of these minutes one is kept.
pub const EVERY: u64 = 60;

/// The folder the copies are kept in.
fn folder() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("recovery"))
}

/// The name a file's copy goes by: the path's FNV-1a hash, so the same
/// file always has the same copy.
fn name(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A copy left from before.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    /// The file it belongs to.
    pub path: PathBuf,
    /// The copy itself.
    pub copy: PathBuf,
    /// When it was made.
    pub when: SystemTime,
}

/// The copies left when the app last stopped, offered on the start
/// screen until recovered or thrown away.
#[derive(Default)]
pub struct Left(pub Vec<Kept>);

impl gpui::Global for Left {}

/// Keeps `bytes` as the copy of `path`'s changes.
pub fn keep(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(dir) = folder() else {
        return Ok(());
    };
    keep_in(&dir, path, bytes)
}

fn keep_in(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let name = name(path);
    // Written beside, then put in place, so a crash while writing
    // leaves the copy before.
    let new = dir.join(format!("{name}.pdf.new"));
    std::fs::write(&new, bytes)?;
    std::fs::rename(&new, dir.join(format!("{name}.pdf")))?;
    std::fs::write(
        dir.join(format!("{name}.txt")),
        path.to_string_lossy().as_bytes(),
    )
}

/// Drops the copy of `path`'s changes, if there is one.
pub fn forget(path: &Path) {
    if let Some(dir) = folder() {
        forget_in(&dir, path);
    }
}

fn forget_in(dir: &Path, path: &Path) {
    let name = name(path);
    std::fs::remove_file(dir.join(format!("{name}.pdf"))).ok();
    std::fs::remove_file(dir.join(format!("{name}.txt"))).ok();
}

/// The copies there are, newest first.
pub fn copies() -> Vec<Kept> {
    folder().map(|dir| copies_in(&dir)).unwrap_or_default()
}

fn copies_in(dir: &Path) -> Vec<Kept> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut copies: Vec<Kept> = entries
        .flatten()
        .filter_map(|entry| {
            let copy = entry.path();
            if copy.extension()? != "pdf" {
                return None;
            }
            let path = std::fs::read_to_string(copy.with_extension("txt")).ok()?;
            let when = entry.metadata().ok()?.modified().ok()?;
            Some(Kept {
                path: PathBuf::from(path.trim_end()),
                copy,
                when,
            })
        })
        .collect();
    copies.sort_by_key(|c| std::cmp::Reverse(c.when));
    copies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_are_kept_listed_and_forgotten() {
        let dir = std::env::temp_dir().join(format!("katna-pdf-recovery-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let file = Path::new("/home/someone/Taxes 2026.pdf");
        keep_in(&dir, file, b"%PDF-1.7 one").unwrap();
        keep_in(&dir, file, b"%PDF-1.7 two").unwrap();
        let copies = copies_in(&dir);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].path, file);
        assert_eq!(std::fs::read(&copies[0].copy).unwrap(), b"%PDF-1.7 two");
        forget_in(&dir, file);
        assert!(copies_in(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn each_file_has_its_own_name() {
        assert_eq!(name(Path::new("/a.pdf")), name(Path::new("/a.pdf")));
        assert_ne!(name(Path::new("/a.pdf")), name(Path::new("/b.pdf")));
    }
}
