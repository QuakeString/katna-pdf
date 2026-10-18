// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF: a fast, pretty PDF reader in the Katna family
//! (docs/PLAN.md). Phase 0: open PDFs in tabs and scroll and zoom them,
//! in the desktop's light or dark mode. One app runs per person; opening
//! a file again hands it to the running window (`instance.rs`).

mod assets;
mod contents;
mod form;
mod history;
mod ids;
mod instance;
mod print;
mod search;
mod select;
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

fn main() -> ExitCode {
    let mut files: Vec<PathBuf> = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{}", t("usage", &[]));
                return ExitCode::SUCCESS;
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
    let requests = match instance::start(&files) {
        instance::Started::HandedOff => return ExitCode::SUCCESS,
        instance::Started::Running(requests) => requests,
    };
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            cx.set_global(history::History::load());
            katna_ui::text_input::bind_keys(cx);
            viewer::bind_keys(cx);
            let Some(window) = open_window(std::mem::take(&mut files), cx) else {
                cx.quit();
                return;
            };
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
