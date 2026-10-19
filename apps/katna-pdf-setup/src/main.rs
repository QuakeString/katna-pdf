// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF Setup: installs Katna PDF on Windows 10 and later, updates
//! it, and removes it.
//!
//! One small window in Katna's look instead of a wizard, as Katna Mail's
//! Setup: the logo, who to install for, the folder, the shortcuts and PDF
//! files, one button, a progress bar, then Open Katna PDF. It follows
//! Windows' light or dark setting, as Katna PDF does.
//!
//! ```text
//! katna-pdf-setup                install or update, with the window
//! katna-pdf-setup --quiet        the same without a window
//!   --all-users                  for everyone (needs an administrator)
//!   --dir <folder>               the programs folder
//!   --desktop                    add a desktop shortcut
//!   --no-start-menu              no Start menu shortcut
//!   --no-pdf                     leave PDF files' Open with alone
//!   --update                     keep the shortcuts as they are
//! katna-pdf-setup --uninstall    remove (Settings > Apps runs this)
//! katna-pdf-setup --uninstall --quiet [--all-users] [--delete-data]
//! ```

// No console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod install;
mod payload;
mod text;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, FontWeight, Image, ImageFormat, IntoElement, ParentElement,
    PathPromptOptions, Render, SharedString, Styled, TitlebarOptions, Window, WindowAppearance,
    WindowBounds, WindowControlArea, WindowOptions, div, img, prelude::*, rgba, size,
};
use katna_ui::px;
use katna_ui::theme::{Theme, fade};
use katna_ui::tokens::{radius, space, text as size_of};
use katna_ui::widgets::{self, ButtonStyle};

use install::{Choices, Layout, Scope, Step};
use payload::Payload;
use text::t;

/// The files Setup installs, packed by `build.rs`.
static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));

/// The logo on Setup's window (`packaging/windows/make-ico.py`).
static LOGO: &[u8] = include_bytes!("../../../packaging/windows/katna-pdf-128.png");

/// The version Setup installs, as the release workflow names it.
const VERSION: &str = match option_env!("KATNA_PDF_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

/// The window: Windows draws its border, shadow and, on Windows 11,
/// round corners.
const WINDOW: (f32, f32) = (580.0, 520.0);

/// What the command line asks for.
#[derive(Debug, Default)]
struct Args {
    quiet: bool,
    uninstall: bool,
    all_users: bool,
    update: bool,
    delete_data: bool,
    dir: Option<PathBuf>,
    choices: Choices,
    /// The process ID of the Setup waiting for this one, running as
    /// administrator, to say how far it got (`install::write_progress`).
    progress: Option<u32>,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let mut out = Self::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--quiet" => out.quiet = true,
                "--uninstall" => out.uninstall = true,
                "--all-users" => out.all_users = true,
                "--update" => out.update = true,
                "--delete-data" => out.delete_data = true,
                "--desktop" => out.choices.desktop = true,
                "--no-start-menu" => out.choices.start_menu = false,
                "--no-pdf" => out.choices.pdf = false,
                "--dir" => out.dir = args.next().map(PathBuf::from),
                "--progress" => out.progress = args.next().and_then(|id| id.parse().ok()),
                _ => {}
            }
        }
        out
    }

    fn scope(&self) -> Scope {
        if self.all_users {
            Scope::Machine
        } else {
            Scope::User
        }
    }
}

/// The arguments that make a Setup running as administrator do what this
/// one would: install `layout` with `choices`, or update it.
fn elevated_args(layout: &Layout, choices: Option<Choices>, progress: u32) -> Vec<String> {
    let mut args = vec![
        "--quiet".to_owned(),
        "--all-users".to_owned(),
        "--dir".to_owned(),
        layout.programs.display().to_string(),
        "--progress".to_owned(),
        progress.to_string(),
    ];
    match choices {
        None => args.push("--update".to_owned()),
        Some(c) => {
            if c.desktop {
                args.push("--desktop".to_owned());
            }
            if !c.start_menu {
                args.push("--no-start-menu".to_owned());
            }
            if !c.pdf {
                args.push("--no-pdf".to_owned());
            }
        }
    }
    args
}

/// A step as a Setup running as administrator writes it for this one.
fn step_line(step: Step) -> String {
    match step {
        Step::Stopping => "stopping".to_owned(),
        Step::Copying(share) => format!("copying {share}"),
        Step::Registering => "registering".to_owned(),
    }
}

fn parse_step(line: &str) -> Option<Step> {
    match line.trim().split_once(' ') {
        Some(("copying", share)) => share.parse().ok().map(Step::Copying),
        _ => match line.trim() {
            "stopping" => Some(Step::Stopping),
            "registering" => Some(Step::Registering),
            _ => None,
        },
    }
}

fn main() -> ExitCode {
    let args = Args::parse(std::env::args().skip(1));
    let payload = match Payload::read(PAYLOAD) {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!("katna-pdf-setup: {err}");
            return ExitCode::FAILURE;
        }
    };
    if args.quiet {
        return match quiet(&args, &payload) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("katna-pdf-setup: {err}");
                ExitCode::FAILURE
            }
        };
    }
    let existing = Layout::existing();
    let start = if args.uninstall {
        match &existing {
            Some(_) => Screen::ConfirmRemove { data: false },
            None => Screen::Removed { data: false },
        }
    } else if payload.entries.is_empty() {
        Screen::Failed(t("setup-empty", &[]))
    } else {
        Screen::Welcome
    };
    gpui_platform::application().run(move |cx: &mut App| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(WINDOW.0), px(WINDOW.1)),
                cx,
            ))),
            // Setup draws its own close button, the same on Windows 10
            // and 11.
            titlebar: Some(TitlebarOptions {
                title: Some(t("setup-window-title", &[]).into()),
                appears_transparent: true,
                ..Default::default()
            }),
            app_id: Some(format!("{}.Pdf.Setup", katna_core::ids::PREFIX)),
            is_resizable: false,
            ..Default::default()
        };
        let opened = cx.open_window(options, |_, cx| {
            cx.new(|_| Setup {
                existing,
                scope: Scope::User,
                folder: None,
                choices: Choices::default(),
                screen: start,
                progress: Arc::new(Mutex::new(None)),
                logo: Arc::new(Image::from_bytes(ImageFormat::Png, LOGO.to_vec())),
            })
        });
        if let Err(err) = opened {
            eprintln!("katna-pdf-setup: {err}");
            cx.quit();
        }
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
    ExitCode::SUCCESS
}

/// Installs or removes without a window.
fn quiet(args: &Args, payload: &Payload<'_>) -> std::io::Result<()> {
    let mut layout = Layout::from_env(args.scope())?;
    if let Some(dir) = &args.dir {
        layout = layout.in_folder(dir);
    } else if let Some(existing) = Layout::existing().filter(|l| l.scope == args.scope()) {
        layout = existing;
    }
    if args.uninstall {
        return install::uninstall(&layout, args.delete_data);
    }
    if payload.entries.is_empty() {
        return Err(std::io::Error::other(t("setup-empty", &[])));
    }
    let choices = (!args.update).then_some(args.choices);
    let installed = install::install(&layout, payload, VERSION, choices, &|step| {
        if let Some(id) = args.progress {
            install::write_progress(id, &step_line(step));
        }
    });
    if let Some(id) = args.progress {
        install::clear_progress(id);
    }
    installed
}

/// What the window shows.
#[derive(Debug, Clone, PartialEq)]
enum Screen {
    Welcome,
    Installing(Step),
    Done,
    Failed(String),
    ConfirmRemove { data: bool },
    Removing,
    Removed { data: bool },
}

struct Setup {
    /// Katna PDF as installed already: Setup updates it in place.
    existing: Option<Layout>,
    scope: Scope,
    /// The folder the user picked, instead of the default for the scope.
    folder: Option<PathBuf>,
    choices: Choices,
    screen: Screen,
    /// The work's result, written by its thread: `None` while it runs.
    progress: Arc<Mutex<Option<Result<(), String>>>>,
    logo: Arc<Image>,
}

impl Setup {
    /// Where the install goes: the installed Katna PDF for an update, else
    /// the scope's default folder or the one picked.
    fn target(&self) -> std::io::Result<Layout> {
        if let Some(existing) = &self.existing {
            return Ok(existing.clone());
        }
        let layout = Layout::from_env(self.scope)?;
        Ok(match &self.folder {
            Some(dir) => layout.in_folder(dir),
            None => layout,
        })
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let layout = match self.target().and_then(|l| l.check_folder().map(|()| l)) {
            Ok(layout) => layout,
            Err(err) => {
                self.screen = Screen::Failed(err.to_string());
                return;
            }
        };
        let choices = self.existing.is_none().then_some(self.choices);
        self.screen = Screen::Installing(Step::Stopping);
        let step = Arc::new(Mutex::new(Step::Stopping));
        *self.progress.lock().unwrap() = None;
        let (shared_step, result) = (step.clone(), self.progress.clone());
        if layout.scope == Scope::Machine && !install::elevated() {
            // A second Setup does the work as administrator and records
            // how far it got where this one reads it.
            let id = std::process::id();
            let args = elevated_args(&layout, choices, id);
            let stop = Arc::new(Mutex::new(false));
            let stopped = stop.clone();
            std::thread::spawn(move || {
                while !*stopped.lock().unwrap() {
                    if let Some(now) = install::read_progress(id).and_then(|l| parse_step(&l)) {
                        *shared_step.lock().unwrap() = now;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            });
            std::thread::spawn(move || {
                let done = match install::run_elevated(&args) {
                    Ok(0) => Ok(()),
                    Ok(_) => Err(t("setup-everyone-failed", &[])),
                    Err(_) => Err(t("setup-everyone-refused", &[])),
                };
                *stop.lock().unwrap() = true;
                *result.lock().unwrap() = Some(done);
            });
        } else {
            std::thread::spawn(move || {
                let payload = Payload::read(PAYLOAD).map_err(|e| e.to_string());
                let done = payload.and_then(|payload| {
                    install::install(&layout, &payload, VERSION, choices, &|s| {
                        *shared_step.lock().unwrap() = s;
                    })
                    .map_err(|e| e.to_string())
                });
                *result.lock().unwrap() = Some(done);
            });
        }
        self.follow(step, Screen::Done, cx);
    }

    fn remove(&mut self, data: bool, cx: &mut Context<Self>) {
        let Some(layout) = self.existing.clone() else {
            return;
        };
        self.screen = Screen::Removing;
        *self.progress.lock().unwrap() = None;
        let result = self.progress.clone();
        std::thread::spawn(move || {
            let done = if layout.scope == Scope::Machine && !install::elevated() {
                // The administrator removes the programs; the user's own
                // settings and library are deleted here, as that user.
                let args = ["--uninstall", "--quiet", "--all-users"].map(str::to_owned);
                match install::run_elevated(&args) {
                    Ok(0) if data => install::delete_data().map_err(|e| e.to_string()),
                    Ok(0) => Ok(()),
                    Ok(_) => Err(t("setup-everyone-failed", &[])),
                    Err(_) => Err(t("setup-everyone-refused", &[])),
                }
            } else {
                install::uninstall(&layout, data).map_err(|e| e.to_string())
            };
            *result.lock().unwrap() = Some(done);
        });
        self.follow(
            Arc::new(Mutex::new(Step::Stopping)),
            Screen::Removed { data },
            cx,
        );
    }

    /// Redraws while the work on the other thread runs, then shows `done`
    /// or what went wrong.
    fn follow(&mut self, step: Arc<Mutex<Step>>, done: Screen, cx: &mut Context<Self>) {
        let result = self.progress.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let ended = result.lock().unwrap().take();
                let now = *step.lock().unwrap();
                let updated = this.update(cx, |setup, cx| {
                    match &ended {
                        Some(Ok(())) => setup.screen = done.clone(),
                        Some(Err(err)) => setup.screen = Screen::Failed(err.clone()),
                        None => {
                            if matches!(setup.screen, Screen::Installing(_)) {
                                setup.screen = Screen::Installing(now);
                            }
                        }
                    }
                    cx.notify();
                });
                if ended.is_some() || updated.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn open_katna_pdf(&self, cx: &mut Context<Self>) {
        if let Ok(layout) = self.target() {
            let app = layout.programs.join(install::APP_EXE);
            if let Err(err) = std::process::Command::new(&app).spawn() {
                eprintln!("katna-pdf-setup: cannot open Katna PDF: {err}");
            }
        }
        cx.quit();
    }

    /// Asks for the folder to install into.
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("setup-folder-pick", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                let _ = this.update(cx, |setup, cx| {
                    setup.folder = Some(dir);
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

impl Render for Setup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        let th = Theme::new(dark);
        let (title, body, bar, buttons) = self.content(&th, cx);
        let busy = matches!(self.screen, Screen::Installing(_) | Screen::Removing);
        // The choices stay put when the note under "Install for" comes and
        // goes; the short screens sit in the middle.
        let choosing = self.screen == Screen::Welcome && self.existing.is_none();
        let card = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(th.surface))
            .text_color(rgba(th.text))
            .p(px(space::S7))
            .child(drag_area())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(20.0))
                    .child(img(self.logo.clone()).size(px(64.0)).flex_none())
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .text_size(px(size_of::DISPLAY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(px(size_of::BODY))
                                    .text_color(rgba(th.text_dim))
                                    .child(t("setup-tagline", &[])),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .when(choosing, |d| d.pt(px(28.0)))
                    .when(!choosing, |d| d.justify_center())
                    .gap(px(space::S5))
                    .text_size(px(size_of::BODY))
                    .line_height(px(21.0))
                    .children(body)
                    .children(bar),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(space::S3))
                    .children(buttons),
            );
        if busy {
            card
        } else {
            card.child(close_button(&th, cx))
        }
    }
}

/// The top of the window, which moves it.
fn drag_area() -> impl IntoElement {
    div()
        .id("drag")
        .absolute()
        .top_0()
        .left_0()
        .right(px(56.0))
        .h(px(56.0))
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            // Windows moves it through the control area above.
            if !cfg!(windows) {
                window.start_window_move();
            }
        })
}

/// The X at the top right.
fn close_button(th: &Theme, cx: &mut Context<Setup>) -> AnyElement {
    div()
        .id("close-window")
        .absolute()
        .top(px(space::S4))
        .right(px(space::S4))
        .size(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(rgba(th.text_dim))
        .text_size(px(18.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(th.hover)))
        .on_click(cx.listener(|_, _, _, cx| cx.quit()))
        .child("✕")
        .into_any_element()
}

impl Setup {
    /// The Welcome screen's choices: who for, where, and what to add.
    fn choices_view(&self, th: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let folder = self
            .target()
            .map(|l| l.programs.display().to_string())
            .unwrap_or_default();
        let scope = self.scope;
        let pill = |id: &'static str, label: String, this: Scope, cx: &mut Context<Self>| {
            let on = scope == this;
            let pill = div()
                .id(id)
                .h(px(36.0))
                .px(px(space::S5))
                .flex()
                .items_center()
                .rounded_full()
                .border_1()
                .cursor_pointer()
                .text_size(px(size_of::BODY))
                .on_click(cx.listener(move |setup, _, _, cx| {
                    if setup.scope != this {
                        setup.scope = this;
                        setup.folder = None;
                    }
                    cx.notify();
                }))
                .child(label);
            if on {
                pill.bg(rgba(fade(th.accent, 0.16)))
                    .border_color(rgba(th.accent))
                    .text_color(rgba(th.text))
            } else {
                pill.border_color(rgba(th.outline))
                    .text_color(rgba(th.text_dim))
                    .hover(|s| s.bg(rgba(th.hover)))
            }
            .into_any_element()
        };
        let label = |s: String| {
            div()
                .text_size(px(size_of::SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgba(th.text_dim))
                .child(s)
        };
        let mut who = div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(label(t("setup-for", &[])))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(space::S3))
                    .child(pill("for-me", t("setup-for-me", &[]), Scope::User, cx))
                    .child(pill(
                        "for-everyone",
                        t("setup-for-everyone", &[]),
                        Scope::Machine,
                        cx,
                    )),
            );
        if scope == Scope::Machine {
            who = who.child(
                div()
                    .text_size(px(size_of::SMALL))
                    .text_color(rgba(th.text_dim))
                    .child(t("setup-for-everyone-note", &[])),
            );
        }
        let place = div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(label(t("setup-folder", &[])))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(36.0))
                            .px(px(space::S4))
                            .flex()
                            .items_center()
                            .rounded(px(radius::SM))
                            .bg(rgba(th.search))
                            .text_size(px(size_of::SMALL))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(folder),
                    )
                    .child(
                        widgets::button("change-folder", ButtonStyle::Text, th)
                            .child(t("setup-folder-change", &[]))
                            .on_click(cx.listener(|setup, _, _, cx| setup.pick_folder(cx))),
                    ),
            );
        let ch = self.choices;
        let boxes = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(checkbox(
                "desktop",
                ch.desktop,
                t("setup-desktop-shortcut", &[]),
                th,
                cx,
                |s| s.choices.desktop = !s.choices.desktop,
            ))
            .child(checkbox(
                "start-menu",
                ch.start_menu,
                t("setup-start-menu", &[]),
                th,
                cx,
                |s| s.choices.start_menu = !s.choices.start_menu,
            ))
            .child(checkbox(
                "pdf",
                ch.pdf,
                t("setup-pdf-files", &[]),
                th,
                cx,
                |s| s.choices.pdf = !s.choices.pdf,
            ));
        vec![
            who.into_any_element(),
            place.into_any_element(),
            boxes.into_any_element(),
        ]
    }

    /// The title, the text, the progress bar and the buttons of the screen.
    fn content(
        &self,
        th: &Theme,
        cx: &mut Context<Self>,
    ) -> (
        SharedString,
        Vec<AnyElement>,
        Option<AnyElement>,
        Vec<AnyElement>,
    ) {
        let text = |s: String| div().child(s).into_any_element();
        let dim = |s: String| {
            div()
                .text_color(rgba(th.text_dim))
                .child(s)
                .into_any_element()
        };
        let close = |cx: &mut Context<Self>| {
            widgets::button("close", ButtonStyle::Text, th)
                .child(t("setup-close", &[]))
                .on_click(cx.listener(|_, _, _, cx| cx.quit()))
                .into_any_element()
        };
        match &self.screen {
            Screen::Welcome => {
                let (label, body) = match &self.existing {
                    Some(existing) => {
                        let path = existing.programs.display().to_string();
                        (
                            t("setup-update", &[]),
                            vec![
                                text(t("setup-update-where", &[("path", &path)])),
                                dim(t("setup-version", &[("version", VERSION)])),
                            ],
                        )
                    }
                    None => (t("setup-install", &[]), self.choices_view(th, cx)),
                };
                (
                    t("setup-app-name", &[]).into(),
                    body,
                    None,
                    vec![
                        widgets::button("cancel", ButtonStyle::Text, th)
                            .child(t("setup-cancel", &[]))
                            .on_click(cx.listener(|_, _, _, cx| cx.quit()))
                            .into_any_element(),
                        widgets::filled_button("install", label, th)
                            .on_click(cx.listener(|setup, _, _, cx| setup.install(cx)))
                            .into_any_element(),
                    ],
                )
            }
            Screen::Installing(step) => {
                let (share, what) = match step {
                    Step::Stopping => (0.02, t("setup-step-stopping", &[])),
                    Step::Copying(share) => (0.05 + share * 0.85, t("setup-step-copying", &[])),
                    Step::Registering => (0.95, t("setup-step-registering", &[])),
                };
                let title = if self.existing.is_some() {
                    t("setup-updating", &[])
                } else {
                    t("setup-installing", &[])
                };
                (
                    title.into(),
                    vec![dim(what)],
                    Some(progress_bar(share, th)),
                    Vec::new(),
                )
            }
            Screen::Done => {
                let mut buttons = vec![close(cx)];
                // Windows lets only the user make an app the default for
                // a kind of file, in Settings.
                let pdf = self
                    .existing
                    .as_ref()
                    .map_or(self.choices.pdf, |l| l.current_choices().pdf);
                if pdf {
                    let scope = self.target().map_or(self.scope, |l| l.scope);
                    buttons.push(
                        widgets::outlined_button("default", t("setup-make-default", &[]), th)
                            .on_click(move |_, _, _| install::open_default_apps(scope))
                            .into_any_element(),
                    );
                }
                buttons.push(
                    widgets::filled_button("open", t("setup-open", &[]), th)
                        .on_click(cx.listener(|setup, _, _, cx| setup.open_katna_pdf(cx)))
                        .into_any_element(),
                );
                (
                    t("setup-done-title", &[]).into(),
                    vec![
                        text(t("setup-done-body", &[])),
                        dim(t("setup-pin-hint", &[])),
                    ],
                    None,
                    buttons,
                )
            }
            Screen::Failed(err) => {
                let mut buttons = vec![close(cx)];
                if !PAYLOAD.is_empty() {
                    buttons.push(
                        widgets::filled_button("retry", t("setup-try-again", &[]), th)
                            .on_click(cx.listener(|setup, _, _, cx| {
                                setup.screen = Screen::Welcome;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                (
                    t("setup-failed-title", &[]).into(),
                    vec![dim(err.clone())],
                    None,
                    buttons,
                )
            }
            Screen::ConfirmRemove { data } => {
                let data = *data;
                (
                    t("setup-remove-title", &[]).into(),
                    vec![
                        text(t("setup-remove-body", &[])),
                        checkbox(
                            "remove-data",
                            data,
                            t("setup-remove-data", &[]),
                            th,
                            cx,
                            |s| {
                                if let Screen::ConfirmRemove { data } = &mut s.screen {
                                    *data = !*data;
                                }
                            },
                        ),
                    ],
                    None,
                    vec![
                        widgets::button("cancel", ButtonStyle::Text, th)
                            .child(t("setup-cancel", &[]))
                            .on_click(cx.listener(|_, _, _, cx| cx.quit()))
                            .into_any_element(),
                        widgets::filled_button("remove", t("setup-remove", &[]), th)
                            .bg(rgba(th.error))
                            .on_click(cx.listener(move |s, _, _, cx| s.remove(data, cx)))
                            .into_any_element(),
                    ],
                )
            }
            Screen::Removing => (
                t("setup-removing", &[]).into(),
                Vec::new(),
                Some(progress_bar(0.5, th)),
                Vec::new(),
            ),
            Screen::Removed { data } => (
                t("setup-removed-title", &[]).into(),
                vec![text(if *data {
                    t("setup-removed-deleted", &[])
                } else {
                    t("setup-removed-kept", &[])
                })],
                None,
                vec![
                    widgets::filled_button("close", t("setup-close", &[]), th)
                        .on_click(cx.listener(|_, _, _, cx| cx.quit()))
                        .into_any_element(),
                ],
            ),
        }
    }
}

/// The share done, as a bar.
fn progress_bar(share: f32, th: &Theme) -> AnyElement {
    div()
        .w_full()
        .h(px(6.0))
        .rounded_full()
        .bg(rgba(fade(th.accent, 0.2)))
        .child(
            div()
                .h_full()
                .w(gpui::relative(share.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(rgba(th.accent)),
        )
        .into_any_element()
}

/// A tick box with its label; clicking either calls `toggle`.
fn checkbox(
    id: &'static str,
    on: bool,
    label: String,
    th: &Theme,
    cx: &mut Context<Setup>,
    toggle: impl Fn(&mut Setup) + 'static,
) -> AnyElement {
    let mut tick = div()
        .size(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::XS))
        .border_2()
        .border_color(rgba(if on { th.accent } else { th.text_dim }));
    if on {
        tick = tick
            .bg(rgba(th.accent))
            .text_color(rgba(th.on_accent))
            .text_size(px(size_of::SMALL))
            .child("✓");
    }
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(space::S4))
        .cursor_pointer()
        .on_click(cx.listener(move |setup, _, _, cx| {
            toggle(setup);
            cx.notify();
        }))
        .child(tick)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setup_for_everyone_gets_the_same_choices() {
        let layout = Layout {
            scope: Scope::Machine,
            programs: PathBuf::from("/Program Files/Katna PDF"),
            start_menu: PathBuf::from("/Start"),
            desktop: PathBuf::from("/Desktop"),
        };
        let choices = Choices {
            desktop: true,
            start_menu: false,
            pdf: false,
        };
        let args = elevated_args(&layout, Some(choices), 42);
        let parsed = Args::parse(args.into_iter());
        assert!(parsed.quiet && parsed.all_users && !parsed.update);
        assert_eq!(parsed.choices, choices);
        assert_eq!(parsed.dir.as_deref(), Some(layout.programs.as_path()));
        assert_eq!(parsed.progress, Some(42));

        let update = elevated_args(&layout, None, 42);
        assert!(Args::parse(update.into_iter()).update);
    }

    #[test]
    fn steps_cross_to_the_waiting_setup() {
        for step in [Step::Stopping, Step::Copying(0.5), Step::Registering] {
            assert_eq!(parse_step(&step_line(step)), Some(step));
        }
        assert_eq!(parse_step(""), None);
    }
}
