// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF: a fast, pretty PDF reader in the Katna family
//! (docs/PLAN.md). Phase 0: open PDFs in tabs and scroll and zoom them,
//! in the desktop's light or dark mode. One app runs per person; opening
//! a file again hands it to the running window (`instance.rs`).

// No console window on Windows (the render workers it starts talk over
// pipes, which a windowed program still has).
#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod annotate;
mod assets;
mod contents;
mod drives;
mod engine;
mod form;
mod history;
mod ids;
mod instance;
mod library;
mod mail;
#[cfg(target_os = "linux")]
mod menu;
mod organize;
mod print;
mod search;
mod select;
mod speech;
mod spread;
mod tabs;
mod text;
mod theme;
mod thumbnails;
mod viewer;

use std::path::PathBuf;
use std::process::ExitCode;

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, size,
};
use katna_ui::scale::desktop_px;

use crate::text::t;

/// Keeps crash reports with the Katna family's, in Katna's crash folder
/// (`katna_core::crash`): a panic leaves a scrubbed report there, which
/// Katna Mail lists, and its "Save crash reports on this computer" switch
/// covers Katna PDF too. Native crashes are collected from
/// `systemd-coredump` by Katna Mail, which keeps the one record of what
/// it has looked at. Nothing is sent anywhere.
fn crash_reports() {
    if let Ok(paths) = katna_core::Paths::from_env() {
        katna_core::crash::install("katna-pdf", &paths);
    }
}

fn main() -> ExitCode {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == engine::WORKER_ARG)
    {
        return match katna_pdf_render::worker::serve() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    crash_reports();
    let mut files: Vec<PathBuf> = Vec::new();
    // `--compare OLDER NEWER`: NEWER opened and compared with OLDER.
    let mut compare: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{}\n{}", t("usage", &[]), t("usage-compare", &[]));
                return ExitCode::SUCCESS;
            }
            Some("--compare") => {
                let (Some(older), Some(newer)) = (args.next(), args.next()) else {
                    eprintln!("{}\n{}", t("usage", &[]), t("usage-compare", &[]));
                    return ExitCode::FAILURE;
                };
                compare = Some(std::path::absolute(&older).unwrap_or_else(|_| older.into()));
                files.push(std::path::absolute(&newer).unwrap_or_else(|_| newer.into()));
            }
            // The running app has another working folder.
            _ => files.push(std::path::absolute(&arg).unwrap_or_else(|_| arg.into())),
        }
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!(
            "katna-pdf: no screen to open a window on (WAYLAND_DISPLAY and DISPLAY are unset)"
        );
        return ExitCode::FAILURE;
    }
    // A comparison is not handed to a running app: it opens its own.
    let requests = match compare {
        Some(_) => None,
        None => match instance::start(&files) {
            instance::Started::HandedOff => return ExitCode::SUCCESS,
            instance::Started::Running(requests) => requests,
        },
    };
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            cx.set_global(history::History::load());
            katna_ui::text_input::bind_keys(cx);
            katna_ui::text_area::bind_keys(cx);
            viewer::bind_keys(cx);
            // Before the window opens, so it carries the menu's address.
            #[cfg(target_os = "linux")]
            let clicks = menu::serve(cx);
            let Some(window) = open_window(std::mem::take(&mut files), cx) else {
                cx.quit();
                return;
            };
            if let Some(older) = compare.take() {
                window
                    .update(cx, |tabs, _, cx| tabs.compare_with(older, cx))
                    .ok();
            }
            #[cfg(target_os = "linux")]
            if let Some(clicks) = clicks {
                cx.spawn(async move |cx| menu::run(window, clicks, cx).await)
                    .detach();
            }
            cx.activate(true);
            if let Some(requests) = requests.clone() {
                cx.spawn(async move |cx| serve(window, requests, cx).await)
                    .detach();
            }
        });
    ExitCode::SUCCESS
}

fn open_window(files: Vec<PathBuf>, cx: &mut App) -> Option<WindowHandle<tabs::Tabs>> {
    let bounds = Bounds::centered(None, size(desktop_px(1100.0), desktop_px(820.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some(ids::app_id()),
        titlebar: Some(TitlebarOptions {
            title: Some(t("app-name", &[]).into()),
            ..Default::default()
        }),
        window_min_size: Some(size(desktop_px(360.0), desktop_px(280.0))),
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| tabs::Tabs::new(files, window, cx))
    }) {
        Ok(window) => Some(window),
        Err(err) => {
            eprintln!("katna-pdf: cannot open a window: {err}");
            None
        }
    }
}

/// Opens what later launches of the app ask for, as tabs, and brings the
/// window to the front. A window closed meanwhile is opened again.
async fn serve(
    mut window: WindowHandle<tabs::Tabs>,
    requests: async_channel::Receiver<instance::Request>,
    cx: &mut AsyncApp,
) {
    while let Ok(request) = requests.recv().await {
        if let Some(token) = request.activation_token {
            katna_ui::native::set_activation_token(token);
        }
        let mut files = Some(request.files);
        let shown = window.update(cx, |tabs, window, cx| {
            tabs.open_files(files.take().unwrap_or_default(), window, cx);
            window.activate_window();
        });
        if shown.is_err() {
            let reopened = cx.update(|cx| open_window(files.take().unwrap_or_default(), cx));
            match reopened {
                Some(new) => window = new,
                None => return,
            }
        }
    }
}
