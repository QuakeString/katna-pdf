// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF: a fast, pretty PDF reader in the Katna family
//! (docs/PLAN.md). This is Phase 0's first window: open a PDF and scroll
//! and zoom it, in the desktop's light or dark mode.

mod assets;
mod ids;
mod text;
mod theme;
mod viewer;

use std::path::PathBuf;
use std::process::ExitCode;

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, size};
use katna_ui::scale::desktop_px;

use crate::text::t;

fn main() -> ExitCode {
    let mut file: Option<PathBuf> = None;
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{}", t("usage", &[]));
                return ExitCode::SUCCESS;
            }
            _ => file = Some(arg.into()),
        }
    }
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!(
            "katna-pdf: no screen to open a window on (WAYLAND_DISPLAY and DISPLAY are unset)"
        );
        return ExitCode::FAILURE;
    }
    gpui_platform::application()
        .with_assets(assets::Assets)
        .run(move |cx: &mut App| {
            viewer::bind_keys(cx);
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
            let file = file.clone();
            if let Err(err) = cx.open_window(options, move |window, cx| {
                cx.new(|cx| viewer::Viewer::new(file, window, cx))
            }) {
                eprintln!("katna-pdf: cannot open a window: {err}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
    ExitCode::SUCCESS
}
