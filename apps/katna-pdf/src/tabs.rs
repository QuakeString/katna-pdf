// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's tabs: one [`Viewer`] per open PDF. The tab bar is a single
//! line, shown only while more than one PDF is open (docs/PLAN.md §4.7).
//! Files from the open button, a drop or a later launch of the app
//! (`instance.rs`) open here: a file already open comes to the front
//! rather than opening twice, and an empty tab is used before a new one.

use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, PathPromptOptions, Subscription, Window, div,
    prelude::*,
};
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};

use crate::history::History;
use crate::library::{LibraryEvent, LibraryView};
use crate::text::t;
use crate::theme::Theme;
use crate::update::{self, InstallError, Offer, Seen};
use crate::viewer::{
    CloseAll, CloseTab, NextTab, Open, PreviousTab, ReopenSession, ShowLibrary, Viewer, ViewerEvent,
};

pub(crate) struct Tab {
    pub(crate) viewer: Entity<Viewer>,
    _subscriptions: [Subscription; 2],
}

/// The title bar's menus (`chrome.rs`).
/// What the profile menu says of a signing token (S14), looked for each
/// time the menu opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenStatus {
    Looking,
    /// No token driver is installed.
    NoDriver,
    /// A driver, but no token in.
    NonePlugged,
    /// The first token in: its label.
    Found(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopMenu {
    Share,
    App,
    Profile,
}

pub struct Tabs {
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    /// The title bar's menu open, and the one closed last with when.
    pub(crate) menu: Option<TopMenu>,
    pub(crate) menu_closed: Option<(TopMenu, std::time::Instant)>,
    /// The app menu's submenu shown.
    /// The application menu's title pointed at, its items shown.
    pub(crate) app_open: Option<usize>,
    /// The tab right-clicked, and where, while its menu is open.
    pub(crate) tab_menu: Option<(usize, gpui::Point<gpui::Pixels>)>,
    /// The person's name, for their picture.
    pub(crate) person: String,
    /// The window title last set, so it is set only when it changes.
    title: String,
    pub(crate) theme: Theme,
    /// The library, made the first time it is shown.
    library: Option<(Entity<LibraryView>, Subscription)>,
    /// The library is shown instead of the documents.
    pub(crate) in_library: bool,
    /// A newer Katna PDF, and how far installing it got (`update.rs`).
    pub(crate) update: Option<(Offer, UpdateStep)>,
    /// A look for a newer build asked for by hand, and what it found.
    pub(crate) update_check: Option<UpdateCheck>,
    _appearance: Subscription,
    _look: Subscription,
    _profile: Subscription,
    _active: Subscription,
    /// The signing token, for the profile menu.
    pub(crate) token: TokenStatus,
}

/// A look for updates asked for from the profile menu.
#[derive(Clone, PartialEq)]
pub(crate) enum UpdateCheck {
    Looking,
    Newest,
    Failed(String),
}

/// Where an update offered stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum UpdateStep {
    Offered,
    Downloading(f32),
    Installing,
    /// Files have changes not saved: they are saved or closed first.
    Unsaved,
    Failed(String),
}

impl Tabs {
    pub fn new(files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance =
            cx.observe_window_appearance(window, |this, window, cx| this.restyle(window, cx));
        let look = cx.observe_global_in::<crate::look::Look>(window, |this, window, cx| {
            this.restyle(window, cx)
        });
        let profile = cx.observe_global::<crate::profile::Profile>(|this, cx| {
            this.person = cx.global::<crate::profile::Profile>().display_name();
            cx.notify();
        });
        // Katna Mail's colours may have changed while this window was away.
        let active = cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                crate::look::Look::follow(cx);
            }
        });
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            menu: None,
            menu_closed: None,
            app_open: None,
            tab_menu: None,
            person: cx.global::<crate::profile::Profile>().display_name(),
            title: String::new(),
            theme: Theme::current(window, cx.global()),
            library: None,
            in_library: false,
            update: None,
            update_check: None,
            _appearance: appearance,
            _look: look,
            _profile: profile,
            _active: active,
            token: TokenStatus::Looking,
        };
        this.push(None, window, cx);
        this.open_files(files, window, cx);
        this.watch_updates(cx);
        this.keep_changes(cx);
        this
    }

    /// Every few minutes, as Settings > Editing says, keeps a copy of
    /// each tab's changes not saved.
    fn keep_changes(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let every = std::time::Duration::from_secs(crate::recovery::EVERY);
            let mut minutes = 0;
            loop {
                cx.background_executor().timer(every).await;
                minutes += 1;
                let kept = this.update(cx, |this, cx| {
                    if minutes < cx.global::<crate::prefs::Prefs>().recovery_minutes {
                        return;
                    }
                    minutes = 0;
                    for tab in &this.tabs {
                        tab.viewer.update(cx, |viewer, cx| viewer.keep_changes(cx));
                    }
                });
                if kept.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn restyle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = Theme::current(window, cx.global());
        if let Some((library, _)) = &self.library {
            let theme = self.theme;
            library.update(cx, |library, _| library.set_theme(theme));
        }
        cx.notify();
    }

    /// Looks for a newer Katna PDF a little after starting, then every so
    /// often, at most once in [`update::EVERY`] across runs.
    fn watch_updates(&mut self, cx: &mut Context<Self>) {
        if !update::enabled() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut wait = std::time::Duration::from_secs(20);
            loop {
                cx.background_executor().timer(wait).await;
                wait = std::time::Duration::from_secs(60 * 60);
                let mut seen = Seen::load();
                if !seen.due() {
                    continue;
                }
                let found = cx.background_spawn(async { update::check() }).await;
                seen.mark_checked();
                seen.save();
                let offer = match found {
                    Ok(Some(offer)) if seen.skipped.as_ref() != Some(&offer.version) => offer,
                    Ok(_) => continue,
                    Err(err) => {
                        eprintln!("katna-pdf: looking for updates: {err}");
                        continue;
                    }
                };
                let gone = this
                    .update(cx, |this, cx| {
                        if this
                            .update
                            .as_ref()
                            .is_none_or(|(_, step)| *step == UpdateStep::Offered)
                        {
                            this.update = Some((offer, UpdateStep::Offered));
                            cx.notify();
                        }
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        })
        .detach();
    }

    /// Looks for a newer build now, whenever the last look was.
    pub(crate) fn check_updates_now(&mut self, cx: &mut Context<Self>) {
        if !update::enabled() || self.update_check == Some(UpdateCheck::Looking) {
            return;
        }
        self.update_check = Some(UpdateCheck::Looking);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_spawn(async { update::check() }).await;
            let mut seen = Seen::load();
            seen.mark_checked();
            seen.save();
            this.update(cx, |this, cx| {
                this.update_check = Some(match found {
                    Ok(Some(offer)) => {
                        if this.update.is_none() {
                            this.update = Some((offer, UpdateStep::Offered));
                        }
                        UpdateCheck::Newest
                    }
                    Ok(None) => UpdateCheck::Newest,
                    Err(err) => UpdateCheck::Failed(err),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Sets the update offered aside until a newer one comes.
    fn skip_update(&mut self, cx: &mut Context<Self>) {
        if let Some((offer, _)) = self.update.take() {
            let mut seen = Seen::load();
            seen.skipped = Some(offer.version);
            seen.save();
        }
        cx.notify();
    }

    /// Downloads, checks and installs the update offered, then starts
    /// the new Katna PDF with the files open now.
    pub(crate) fn install_update(&mut self, cx: &mut Context<Self>) {
        let Some((offer, step)) = &mut self.update else {
            return;
        };
        if self.tabs.iter().any(|tab| tab.viewer.read(cx).dirty()) {
            *step = UpdateStep::Unsaved;
            cx.notify();
            return;
        }
        let Some(dir) = update::download_dir() else {
            return;
        };
        *step = UpdateStep::Downloading(0.0);
        let offer = offer.clone();
        let files: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.viewer.read(cx).path().map(Path::to_path_buf))
            .collect();
        // Each step as it comes; `None` once installed, to quit.
        let (sender, progress) = async_channel::unbounded::<Option<UpdateStep>>();
        cx.background_spawn(async move {
            let report = |share: f32| {
                let _ = sender.try_send(Some(UpdateStep::Downloading(share)));
            };
            let last = match update::download(&offer, &dir, &report) {
                Ok(file) => {
                    let _ = sender.try_send(Some(UpdateStep::Installing));
                    match update::install(&offer, &file).and_then(|()| {
                        update::restart(&offer.package, &files)
                            .map_err(|e| InstallError::Failed(e.to_string()))
                    }) {
                        Ok(()) => None,
                        Err(InstallError::Cancelled) => Some(UpdateStep::Offered),
                        Err(InstallError::Failed(err)) => Some(UpdateStep::Failed(err)),
                    }
                }
                Err(err) => Some(UpdateStep::Failed(err)),
            };
            let _ = sender.try_send(last);
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(step) = progress.recv().await {
                let Some(step) = step else {
                    cx.update(|cx| cx.quit());
                    break;
                };
                let gone = this
                    .update(cx, |this, cx| {
                        if let Some((_, current)) = &mut this.update {
                            *current = step;
                            cx.notify();
                        }
                    })
                    .is_err();
                if gone {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// The line at the top that offers a newer Katna PDF.
    fn update_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (offer, step) = self.update.as_ref()?;
        let th = self.theme;
        let installs = offer.file.is_some();
        let page = offer.package.page();
        // A pill at the top (S10): the offer, or why it waits; and while
        // it downloads, a card in the corner.
        let pill = |id: &'static str, bg, fg, glyph: &'static str, words: String| {
            div()
                .id(id)
                .occlude()
                .max_w(px(720.0))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .pl(px(space::S3))
                .pr(px(space::S1))
                .py(px(space::S1))
                .rounded(px(radius::MD))
                .bg(bg)
                .shadow(th.shadow())
                .text_size(px(text::SMALL))
                .text_color(fg)
                .child(crate::float::icon(glyph, 15.0, fg))
                .child(div().flex_1().min_w_0().child(words))
        };
        let button = |id: &'static str, words: String| crate::float::button(id, words, false, &th);
        let install = |words: String| {
            button("update-install", words)
                .on_click(cx.listener(|this, _, _, cx| this.install_update(cx)))
        };
        let later = || {
            button("update-later", t("update-later", &[]))
                .on_click(cx.listener(|this, _, _, cx| this.skip_update(cx)))
        };
        let mut top = Vec::new();
        let mut corner = None;
        match step {
            UpdateStep::Offered if installs => top.push(
                pill(
                    "update-offer",
                    th.accent_soft,
                    th.accent_text,
                    "info",
                    t("update-ready", &[]),
                )
                .child(install(t("update-install", &[])))
                .child(later()),
            ),
            UpdateStep::Offered => top.push(
                pill(
                    "update-offer",
                    th.accent_soft,
                    th.accent_text,
                    "info",
                    t("update-out", &[]),
                )
                .child(
                    button("update-download", t("update-download", &[]))
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&page))),
                )
                .child(later()),
            ),
            UpdateStep::Unsaved => {
                top.push(
                    pill(
                        "update-offer",
                        th.accent_soft,
                        th.accent_text,
                        "info",
                        t("update-ready", &[]),
                    )
                    .child(install(t("update-install", &[]))),
                );
                top.push(
                    pill(
                        "update-wait",
                        gpui::rgba(th.katna.warning & 0xffff_ff00 | 0x2e).into(),
                        gpui::rgba(th.katna.warning).into(),
                        "alert",
                        t("update-unsaved", &[]),
                    )
                    .child(later()),
                );
            }
            UpdateStep::Failed(err) => top.push(
                pill(
                    "update-failed",
                    th.error_soft,
                    th.error,
                    "alert",
                    t("update-failed", &[("error", err)]),
                )
                .child(install(t("update-retry", &[])))
                .child(later()),
            ),
            UpdateStep::Downloading(_) | UpdateStep::Installing => {
                let (title, share) = match step {
                    UpdateStep::Downloading(share) => (
                        t(
                            "update-downloading",
                            &[("percent", &format!("{:.0}", share * 100.0))],
                        ),
                        *share,
                    ),
                    _ => (t("update-installing", &[]), 1.0),
                };
                corner = Some(
                    div()
                        .id("update-progress")
                        .occlude()
                        .w(px(320.0))
                        .p(px(space::S4))
                        .flex()
                        .flex_col()
                        .gap(px(space::S2))
                        .rounded(px(radius::MD))
                        .bg(th.card)
                        .shadow(th.lifted())
                        .when(th.dark, |d| d.border_1().border_color(th.line))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .text_size(px(text::BODY))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(th.text)
                                .child(crate::float::icon("download", 15.0, th.accent))
                                .child(title),
                        )
                        .child(
                            div().h(px(6.0)).rounded(px(3.0)).bg(th.field).child(
                                div()
                                    .h_full()
                                    .rounded(px(3.0))
                                    .bg(th.accent)
                                    .w(gpui::relative(share.clamp(0.0, 1.0))),
                            ),
                        )
                        .child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(th.dim)
                                .child(t("update-progress-detail", &[])),
                        ),
                );
            }
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .absolute()
                        .top(px(52.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(space::S2))
                        .children(top),
                )
                .children(corner.map(|card| {
                    div()
                        .absolute()
                        .right(px(space::S5))
                        .bottom(px(space::S5))
                        .child(card)
                })),
        )
    }

    /// Opens each of `files`, and shows the last.
    pub fn open_files(&mut self, files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.in_library = false;
        for file in files {
            if let Some(i) = self.find(&file, cx) {
                self.active = i;
            } else if self.viewer(cx).path().is_none() {
                self.viewer_entity()
                    .update(cx, |viewer, cx| viewer.open(file, cx));
            } else {
                self.push(Some(file), window, cx);
                self.active = self.tabs.len() - 1;
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Compares the file shown with `older`.
    pub fn compare_with(&mut self, older: PathBuf, cx: &mut Context<Self>) {
        self.viewer_entity()
            .update(cx, |viewer, cx| viewer.compare_with(older, cx));
    }

    fn push(&mut self, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = cx.new(|cx| Viewer::new(file, window, cx));
        let events = cx.subscribe_in(
            &viewer,
            window,
            |this, viewer, event, window, cx| match event {
                ViewerEvent::ChooseFiles => this.choose_files(window, cx),
                ViewerEvent::ReopenSession => this.reopen_session(window, cx),
                ViewerEvent::Dropped(paths) => this.open_files(paths.clone(), window, cx),
                ViewerEvent::ShowLibrary => this.toggle_library(window, cx),
                ViewerEvent::Close => {
                    if let Some(i) = this.tabs.iter().position(|t| t.viewer == *viewer) {
                        this.close(i, window, cx);
                    }
                }
            },
        );
        let changes = cx.observe(&viewer, |this, _, cx| {
            this.note_session(cx);
            cx.notify();
        });
        self.tabs.push(Tab {
            viewer,
            _subscriptions: [events, changes],
        });
    }

    /// Opens the files that were open when the app last ran.
    /// Looks for a signing token off the UI thread, for the profile
    /// menu's token row.
    pub(crate) fn look_for_token(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async {
                    let modules = katna_pdf_sign::pkcs11::known_modules();
                    if modules.is_empty() {
                        return TokenStatus::NoDriver;
                    }
                    for path in modules {
                        if let Ok(module) = katna_pdf_sign::pkcs11::Module::load(&path)
                            && let Ok(tokens) = module.tokens()
                            && let Some(token) = tokens.into_iter().next()
                        {
                            let name = if token.label.is_empty() {
                                token.model
                            } else {
                                token.label
                            };
                            return TokenStatus::Found(name);
                        }
                    }
                    TokenStatus::NonePlugged
                })
                .await;
            this.update(cx, |this, cx| {
                this.token = status;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn reopen_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = cx.global::<History>().last_session().to_vec();
        if !files.is_empty() {
            self.open_files(files, window, cx);
        }
    }

    /// Keeps the files open now, for next time.
    fn note_session(&self, cx: &mut App) {
        let files: Vec<PathBuf> = self
            .tabs
            .iter()
            .filter_map(|tab| tab.viewer.read(cx).path().map(Path::to_path_buf))
            .collect();
        History::session_is(files, cx);
    }

    /// The tab showing `file`, if one does.
    fn find(&self, file: &Path, cx: &App) -> Option<usize> {
        let same = |a: &Path| {
            a == file
                || std::fs::canonicalize(a)
                    .ok()
                    .is_some_and(|a| std::fs::canonicalize(file).is_ok_and(|b| a == b))
        };
        self.tabs
            .iter()
            .position(|tab| tab.viewer.read(cx).path().is_some_and(same))
    }

    /// Shows the library, or the documents again.
    fn toggle_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_library {
            self.in_library = false;
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let library = match &self.library {
            Some((library, _)) => library.clone(),
            None => {
                let theme = self.theme;
                let library = cx.new(|cx| LibraryView::new(theme, cx));
                let events =
                    cx.subscribe_in(&library, window, |this, _, event, window, cx| match event {
                        LibraryEvent::Open(path, page) => {
                            this.open_files(vec![path.clone()], window, cx);
                            if let Some(page) = *page {
                                this.viewer_entity()
                                    .update(cx, |viewer, cx| viewer.show_page(page, cx));
                            }
                        }
                        LibraryEvent::OpenWindow(path) => {
                            crate::open_window(vec![path.clone()], cx);
                        }
                        LibraryEvent::Sign(paths) => {
                            let Some((first, others)) = paths.split_first() else {
                                return;
                            };
                            this.open_files(vec![first.clone()], window, cx);
                            let others = others.to_vec();
                            this.viewer_entity()
                                .update(cx, |viewer, cx| viewer.sign_with(others, cx));
                        }
                        LibraryEvent::Compare { older, newer } => {
                            this.open_files(vec![newer.clone()], window, cx);
                            this.compare_with(older.clone(), cx);
                        }
                        LibraryEvent::Close => {
                            this.in_library = false;
                            this.focus_active(window, cx);
                            cx.notify();
                        }
                    });
                self.library = Some((library.clone(), events));
                library
            }
        };
        self.in_library = true;
        library.update(cx, |library, cx| library.shown(window, cx));
        cx.notify();
    }

    pub(crate) fn viewer_entity(&self) -> &Entity<Viewer> {
        &self.tabs[self.active].viewer
    }

    pub(crate) fn viewer<'a>(&'a self, cx: &'a App) -> &'a Viewer {
        self.viewer_entity().read(cx)
    }

    pub(crate) fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let focus = self.viewer(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn choose_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                this.update_in(cx, |this, window, cx| this.open_files(paths, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub(crate) fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i < self.tabs.len() {
            self.active = i;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    /// Moves `by` tabs along, wrapping around.
    fn cycle(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        let next = (self.active as isize + by).rem_euclid(n) as usize;
        self.activate(next, window, cx);
    }

    /// Closes every tab, last first, stopping at one with changes not
    /// saved (which says so; Close all again closes it too).
    pub(crate) fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for i in (0..self.tabs.len()).rev() {
            let viewer = self.tabs[i].viewer.entity_id();
            self.close(i, window, cx);
            if self
                .tabs
                .get(i)
                .is_some_and(|tab| tab.viewer.entity_id() == viewer)
            {
                break;
            }
        }
    }

    /// Closes tab `i`. The last tab is emptied rather than closed, so the
    /// window always has somewhere to open a file.
    pub(crate) fn close(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs.len() == 1 && self.viewer(cx).path().is_none() {
            return;
        }
        // A filled-in form not saved: the tab says so the first time.
        if !self.tabs[i]
            .viewer
            .update(cx, |viewer, cx| viewer.may_close(cx))
        {
            self.activate(i, window, cx);
            return;
        }
        self.tabs[i]
            .viewer
            .update(cx, |viewer, cx| viewer.release(window, cx));
        if self.tabs.len() == 1 {
            self.tabs.clear();
            self.push(None, window, cx);
        } else {
            self.tabs.remove(i);
            if self.active > i || self.active == self.tabs.len() {
                self.active -= 1;
            }
        }
        self.note_session(cx);
        self.focus_active(window, cx);
        cx.notify();
    }
}

impl Tabs {
    /// Closes every tab but `keep`, each asking first if it has changes.
    pub(crate) fn close_others(
        &mut self,
        keep: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for i in (0..self.tabs.len()).rev() {
            if i != keep {
                self.close(i, window, cx);
            }
        }
    }

    /// Closes the tabs after `i`.
    pub(crate) fn close_right(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        for j in (i + 1..self.tabs.len()).rev() {
            self.close(j, window, cx);
        }
    }

    /// Opens tab `i`'s file in a window of its own and closes the tab.
    /// A file with changes not saved stays, and says why.
    pub(crate) fn move_to_window(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(i) else {
            return;
        };
        let viewer = tab.viewer.read(cx);
        let Some(path) = viewer.path().map(Path::to_path_buf) else {
            return;
        };
        if viewer.dirty() {
            tab.viewer.update(cx, |viewer, cx| {
                viewer.tell(crate::text::t("tab-move-unsaved", &[]), cx)
            });
            self.activate(i, window, cx);
            return;
        }
        self.close(i, window, cx);
        crate::open_window(vec![path], cx);
    }
}

impl Focusable for Tabs {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.viewer(cx).focus_handle(cx)
    }
}

impl Render for Tabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.viewer(cx).title() {
            Some(name) => format!("{name} — {}", t("app-name", &[])),
            None => t("app-name", &[]),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
        let th = self.theme;
        // A presentation fills the window, with nothing around it.
        if self.viewer(cx).presenting() {
            return div()
                .id("katna-pdf")
                .size_full()
                .bg(gpui::black())
                .child(self.viewer_entity().clone());
        }
        div()
            .id("katna-pdf")
            .key_context("Tabs")
            .on_action(cx.listener(|this, _: &Open, window, cx| this.choose_files(window, cx)))
            .on_action(cx.listener(|_, _: &crate::viewer::NewWindow, _, cx| {
                crate::open_window(Vec::new(), cx);
            }))
            .on_action(cx.listener(|_, _: &crate::viewer::OpenSettings, _, cx| {
                crate::settings::open(crate::settings::Page::Appearance, cx)
            }))
            .on_action(cx.listener(|_, _: &crate::viewer::Shortcuts, _, cx| {
                crate::settings::open(crate::settings::Page::Shortcuts, cx)
            }))
            .on_action(cx.listener(|_, _: &crate::viewer::ReportProblem, _, cx| {
                cx.open_url("https://github.com/QuakeString/katna-pdf/issues/new")
            }))
            .on_action(
                cx.listener(|this, _: &crate::viewer::ShowGuide, window, cx| {
                    match crate::guide::write() {
                        Ok(path) => this.open_files(vec![path], window, cx),
                        Err(err) => eprintln!("katna-pdf: writing the guide: {err}"),
                    }
                }),
            )
            .on_action(cx.listener(|_, _: &crate::viewer::ShowAbout, _, cx| {
                crate::settings::open(crate::settings::Page::About, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ReopenSession, window, cx| this.reopen_session(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let i = this.active;
                this.close(i, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseAll, window, cx| this.close_all(window, cx)))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .on_action(
                cx.listener(|this, _: &ShowLibrary, window, cx| this.toggle_library(window, cx)),
            )
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(th.page)
            .text_color(th.text)
            .font_family(".SystemUIFont")
            .child(self.title_bar(window, cx))
            .children(self.tab_menu(cx))
            .map(|d| match &self.library {
                Some((library, _)) if self.in_library => d.child(library.clone()),
                _ => d.child(self.viewer_entity().clone()),
            })
            .children(self.update_bar(cx))
    }
}
