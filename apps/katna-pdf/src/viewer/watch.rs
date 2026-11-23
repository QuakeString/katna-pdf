// SPDX-License-Identifier: GPL-3.0-or-later

//! The open file changed on disk, by another program: shown again as it
//! now is when nothing here is unsaved, else a notice says so and Revert
//! shows the new one, the changes here kept until then.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gpui::{Context, Task};

use super::{Viewer, said};

/// How often the file is looked at.
const EVERY: Duration = Duration::from_millis(1500);

/// What tells a file changed: when it was last written, and its length.
pub(super) type Stamp = (SystemTime, u64);

/// The file's stamp now; `None` when it cannot be read.
pub(super) fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Watching the open file.
pub(super) struct Watch {
    path: PathBuf,
    /// The file as last seen here: opened, saved, or shown again.
    seen: Option<Stamp>,
    /// Whether the notice about a change was given, for the change seen.
    told: bool,
    _task: Task<()>,
}

impl Viewer {
    /// Starts watching the file just opened.
    pub(super) fn watch_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            self.watch = None;
            return;
        };
        let task = cx.spawn(async move |this, cx| {
            // A new stamp is acted on once it holds for a look, so a
            // file still being written is not read half done.
            let mut settling: Option<Stamp> = None;
            loop {
                cx.background_executor().timer(EVERY).await;
                let going = this.update(cx, |this, cx| {
                    let Some(watch) = this.watch.as_ref() else {
                        return false;
                    };
                    if this.path.as_deref() != Some(watch.path.as_path()) {
                        return false;
                    }
                    let now = stamp(&watch.path);
                    if now.is_none() || now == watch.seen {
                        settling = None;
                    } else if settling == now {
                        settling = None;
                        this.changed_on_disk(now, cx);
                    } else {
                        settling = now;
                    }
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        });
        self.watch = Some(Watch {
            seen: stamp(&path),
            path,
            told: false,
            _task: task,
        });
    }

    /// Notes the file as it is now as seen here, after writing it.
    pub(super) fn file_written(&mut self) {
        if let Some(watch) = self.watch.as_mut() {
            watch.seen = stamp(&watch.path);
            watch.told = false;
        }
    }

    /// The file is now `now` on disk, written by something else.
    fn changed_on_disk(&mut self, now: Option<Stamp>, cx: &mut Context<Self>) {
        let dirty = self.dirty();
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        if dirty {
            // Kept as seen until Revert, so the notice is given once.
            if !watch.told {
                watch.told = true;
                self.notice = Some(said("file-changed-kept", &[]));
                cx.notify();
            }
            return;
        }
        watch.seen = now;
        watch.told = false;
        self.reload_from_disk(cx);
    }

    /// Shows the file as it now is on disk, where it was; the undo
    /// steps were for the file as it was, so they go.
    fn reload_from_disk(&mut self, cx: &mut Context<Self>) {
        self.undo.clear();
        self.redo.clear();
        self.restore(None, cx);
        self.notice = Some(said("file-changed-reloaded", &[]));
        cx.notify();
    }
}
