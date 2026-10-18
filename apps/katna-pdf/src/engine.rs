// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine the app opens files with: hayro in worker processes, so a
//! file that crashes or hangs it costs a page, not the app
//! (`katna_pdf_render::worker`). `KATNA_PDF_IN_PROCESS=1` draws in the
//! app instead, for debugging.

use std::path::PathBuf;
use std::sync::OnceLock;

use katna_pdf_render::{Engine, Hayro, Workers};

/// The argument that starts the app as a render worker.
pub const WORKER_ARG: &str = "--render-worker";
/// Worker processes at most: the pages drawn at once, and one for
/// thumbnails and text.
const WORKERS: usize = 4;

pub fn engine() -> &'static dyn Engine {
    static ENGINE: OnceLock<Box<dyn Engine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            if std::env::var_os("KATNA_PDF_IN_PROCESS").is_some() {
                return Box::new(Hayro);
            }
            Box::new(Workers::new(program, vec![WORKER_ARG.to_owned()], WORKERS))
        })
        .as_ref()
}

/// The program running now, so a worker is never another build than the
/// app. When an update has replaced the file since the app started, on
/// Linux that is the old program, still reachable through `/proc`.
fn program() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    #[cfg(target_os = "linux")]
    if std::fs::read_link("/proc/self/exe")
        .is_ok_and(|link| link.to_string_lossy().ends_with(" (deleted)"))
    {
        return PathBuf::from("/proc/self/exe");
    }
    exe
}
