// SPDX-License-Identifier: GPL-3.0-or-later

//! The window's tabs: one [`Viewer`] per open PDF. The tab bar is a single
//! line, shown only while more than one PDF is open (docs/PLAN.md §4.7).
//! Files from the open button, a drop or a later launch of the app
//! (`instance.rs`) open here: a file already open comes to the front
//! rather than opening twice, and an empty tab is used before a new one.

use std::path::{Path, PathBuf};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, MouseButton, PathPromptOptions, SharedString,
    Subscription, Window, div, prelude::*, svg,
};
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};

use crate::text::t;
use crate::theme::Theme;
use crate::viewer::{CloseTab, NextTab, Open, PreviousTab, Viewer, ViewerEvent};

/// A tab is never wider than this, so many still fit on one line.
const TAB_WIDTH: f32 = 220.0;

struct Tab {
    viewer: Entity<Viewer>,
    _subscriptions: [Subscription; 2],
}

pub struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    /// The window title last set, so it is set only when it changes.
    title: String,
    theme: Theme,
    _appearance: Subscription,
}

impl Tabs {
    pub fn new(files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance());
            cx.notify();
        });
        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            title: String::new(),
            theme: Theme::for_appearance(window.appearance()),
            _appearance: appearance,
        };
        this.push(None, window, cx);
        this.open_files(files, window, cx);
        this
    }

    /// Opens each of `files`, and shows the last.
    pub fn open_files(&mut self, files: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
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

    fn push(&mut self, file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let viewer = cx.new(|cx| Viewer::new(file, window, cx));
        let events = cx.subscribe_in(&viewer, window, |this, _, event, window, cx| match event {
            ViewerEvent::ChooseFiles => this.choose_files(window, cx),
            ViewerEvent::Dropped(paths) => this.open_files(paths.clone(), window, cx),
        });
        let changes = cx.observe(&viewer, |_, _, cx| cx.notify());
        self.tabs.push(Tab {
            viewer,
            _subscriptions: [events, changes],
        });
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

    fn viewer_entity(&self) -> &Entity<Viewer> {
        &self.tabs[self.active].viewer
    }

    fn viewer<'a>(&'a self, cx: &'a App) -> &'a Viewer {
        self.viewer_entity().read(cx)
    }

    fn focus_active(&self, window: &mut Window, cx: &mut App) {
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

    fn activate(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    /// Closes tab `i`. The last tab is emptied rather than closed, so the
    /// window always has somewhere to open a file.
    fn close(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
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
            .update(cx, |viewer, _| viewer.release(window));
        if self.tabs.len() == 1 {
            self.tabs.clear();
            self.push(None, window, cx);
        } else {
            self.tabs.remove(i);
            if self.active > i || self.active == self.tabs.len() {
                self.active -= 1;
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn tab_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        div()
            .id("tab-bar")
            .flex()
            .flex_shrink_0()
            .gap(px(space::S1))
            .pb(px(space::S2))
            .overflow_x_scroll()
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                let active = i == self.active;
                let title = tab
                    .viewer
                    .read(cx)
                    .title()
                    .unwrap_or_else(|| t("tab-empty", &[]));
                div()
                    .id(("tab", i))
                    .h(px(32.0))
                    .min_w(px(96.0))
                    .max_w(px(TAB_WIDTH))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(space::S1))
                    .pl(px(space::S3))
                    .pr(px(space::S1))
                    .rounded(px(radius::SM))
                    .text_size(px(text::SMALL))
                    .cursor_pointer()
                    .when(active, |d| {
                        d.bg(th.card)
                            .border_1()
                            .border_color(th.line)
                            .text_color(th.text)
                    })
                    .when(!active, |d| d.text_color(th.dim).hover(|s| s.bg(th.hover)))
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(i, window, cx)))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| this.close(i, window, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .child(
                        div()
                            .id(("tab-close", i))
                            .size(px(24.0))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::FULL))
                            .hover(move |s| s.bg(th.hover))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close(i, window, cx);
                            }))
                            .child(
                                svg()
                                    .path(SharedString::from("icons/close.svg"))
                                    .size(px(14.0))
                                    .text_color(th.dim),
                            ),
                    )
            }))
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
        div()
            .id("katna-pdf")
            .key_context("Tabs")
            .on_action(cx.listener(|this, _: &Open, window, cx| this.choose_files(window, cx)))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let i = this.active;
                this.close(i, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle(1, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, window, cx| this.cycle(-1, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(th.page)
            .p(px(space::S3))
            .font_family(".SystemUIFont")
            .when(self.tabs.len() > 1, |d| d.child(self.tab_bar(cx)))
            .child(self.viewer_entity().clone())
    }
}
