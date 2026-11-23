// SPDX-License-Identifier: GPL-3.0-or-later

//! Float's title bar (docs/design/float.md): the library and side card
//! on the left, then the file's name (or the tabs, with two files or
//! more), and on the right find, the share menu, the app menu and the
//! person's picture, whose menu holds the mode, the colour schemes and
//! the rest. Under the desktop's own frame this is the window's
//! toolbar; where the window draws its own frame it also moves the
//! window and holds its buttons.

use gpui::{
    AnyElement, Context, Decorations, FontWeight, MouseButton, SharedString, Window, div,
    prelude::*,
};
use katna_ui::px;
use katna_ui::tokens::{space, text};

use crate::float::{
    self, BAR, FloatTip, avatar, ib, menu, menu_item, menu_sep, popover, seg, seg_item,
};
use crate::look::{self, Look, Mode};
use crate::menus::{MENUS, action, keys_text, plain};
use crate::tabs::{Tabs, TopMenu};
use crate::text::t;
use crate::viewer::{
    Compare, Export, Find, Present, Print, Properties, Quit, ReadAloud, Redo, Save, ShowLibrary,
    ToggleContents, ToggleNight, Undo,
};

/// The person's name, as the desktop knows it: the full name in their
/// account, else their user name.
pub fn person() -> String {
    #[cfg(unix)]
    {
        let user = std::env::var("USER").unwrap_or_default();
        if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
            for line in passwd.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() > 4 && fields[0] == user {
                    let full = fields[4].split(',').next().unwrap_or("").trim();
                    if !full.is_empty() {
                        return full.to_owned();
                    }
                }
            }
        }
        if !user.is_empty() {
            return user;
        }
    }
    std::env::var("USERNAME").unwrap_or_else(|_| "?".into())
}

impl Tabs {
    /// Runs `action` where the keys go, as its shortcut would.
    fn run(action: Box<dyn gpui::Action>, window: &mut Window, cx: &mut gpui::App) {
        window.dispatch_action(action, cx);
    }

    /// Opens `which`, or closes it when it is open (or just closed by
    /// the click that reached its button).
    fn toggle_menu(&mut self, which: TopMenu, cx: &mut Context<Self>) {
        let just_closed = self
            .menu_closed
            .is_some_and(|(m, at)| m == which && at.elapsed().as_millis() < 300);
        self.menu = if self.menu == Some(which) || just_closed {
            None
        } else {
            Some(which)
        };
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(which) = self.menu.take() {
            self.menu_closed = Some((which, std::time::Instant::now()));
            cx.notify();
        }
    }

    pub(crate) fn title_bar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let chrome = self.viewer(cx).chrome();
        let in_library = self.in_library;
        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let mut bar = div()
            .id("title-bar")
            .h(px(BAR))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(10.0))
            .text_size(px(float::UI_TEXT))
            .text_color(th.text);
        if client {
            bar = bar
                .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                .on_click(|e, window, _| {
                    if e.click_count() == 2 {
                        window.zoom_window();
                    }
                });
        }
        bar = bar
            .child(
                ib("bar-library", "lib", t("library-tip", &[]), in_library, &th)
                    .on_click(|_, window, cx| Self::run(Box::new(ShowLibrary), window, cx)),
            )
            .when(chrome.ready && !in_library, |bar| {
                bar.child(
                    ib(
                        "bar-pane",
                        "side",
                        t("side-pane-toggle", &[]),
                        chrome.pane,
                        &th,
                    )
                    .on_click(|_, window, cx| Self::run(Box::new(ToggleContents), window, cx)),
                )
            })
            .when(
                !in_library && (chrome.dirty || chrome.undo || chrome.redo),
                |bar| {
                    bar.child(float::vsep(&th))
                        .child(
                            ib("bar-undo", "undo", t("undo-tip", &[]), false, &th)
                                .when(!chrome.undo, |b| b.opacity(0.4))
                                .on_click(|_, window, cx| Self::run(Box::new(Undo), window, cx)),
                        )
                        .child(
                            ib("bar-redo", "redo", t("redo-tip", &[]), false, &th)
                                .when(!chrome.redo, |b| b.opacity(0.4))
                                .on_click(|_, window, cx| Self::run(Box::new(Redo), window, cx)),
                        )
                },
            );
        if in_library {
            bar = bar.child(
                div()
                    .ml(px(6.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(t("library-title", &[])),
            );
        } else if self.tabs.len() > 1 {
            bar = bar.child(self.title_tabs(cx));
        } else if chrome.ready {
            let name = self.viewer(cx).title().unwrap_or_default();
            bar = bar.child(
                div()
                    .id("bar-name")
                    .ml(px(6.0))
                    .px(px(space::S3))
                    .py(px(6.0))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(6.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .ftip(t("properties-tip", &[]), &th)
                    .on_click(|_, window, cx| Self::run(Box::new(Properties), window, cx))
                    .child(div().min_w_0().truncate().child(name))
                    .when(chrome.dirty, |d| {
                        d.child(div().size(px(6.0)).rounded(px(3.0)).bg(th.accent))
                    })
                    .child(float::icon("chev", 13.0, th.dim)),
            );
        }
        bar = bar
            .child(div().flex_1().h_full())
            .when(chrome.dirty && !in_library, |bar| {
                bar.child(
                    float::button("bar-save", t("save", &[]), true, &th)
                        .h(px(28.0))
                        .mr(px(space::S2))
                        .on_mouse_move(|_, _, cx| cx.stop_propagation())
                        .on_click(|_, window, cx| Self::run(Box::new(Save), window, cx)),
                )
            })
            .when(chrome.ready && !in_library, |bar| {
                bar.child(
                    ib(
                        "bar-find",
                        "search",
                        t("search-open", &[]),
                        chrome.finding,
                        &th,
                    )
                    .on_click(|_, window, cx| Self::run(Box::new(Find), window, cx)),
                )
                .child(
                    div()
                        .relative()
                        .child(
                            ib(
                                "bar-share",
                                "share",
                                t("share-tip", &[]),
                                self.menu == Some(TopMenu::Share),
                                &th,
                            )
                            .on_click(
                                cx.listener(|this, _, _, cx| this.toggle_menu(TopMenu::Share, cx)),
                            ),
                        )
                        .children((self.menu == Some(TopMenu::Share)).then(|| self.share_menu(cx))),
                )
            })
            .child(
                div()
                    .relative()
                    .child(
                        ib(
                            "bar-more",
                            "more",
                            t("menu-tip", &[]),
                            self.menu == Some(TopMenu::App),
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(TopMenu::App, cx))),
                    )
                    .children((self.menu == Some(TopMenu::App)).then(|| self.app_menu(cx))),
            )
            .child(
                div()
                    .relative()
                    .mx(px(6.0))
                    .child(
                        div()
                            .id("bar-profile")
                            .cursor_pointer()
                            .on_mouse_move(|_, _, cx| cx.stop_propagation())
                            .ftip(t("profile-tip", &[]), &th)
                            .on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_menu(TopMenu::Profile, cx)
                                }),
                            )
                            .child(avatar(&float::initials(&self.person), 28.0, &th)),
                    )
                    .children(
                        (self.menu == Some(TopMenu::Profile))
                            .then(|| self.profile_menu(chrome.colors, cx)),
                    ),
            );
        if client {
            bar = bar
                .child(
                    ib("bar-min", "min", t("window-minimize", &[]), false, &th)
                        .on_click(|_, window, _| window.minimize_window()),
                )
                .child(
                    ib("bar-max", "max", t("window-maximize", &[]), false, &th)
                        .on_click(|_, window, _| window.zoom_window()),
                )
                .child(
                    ib("bar-close", "x", t("window-close", &[]), false, &th)
                        .on_click(|_, window, _| window.remove_window()),
                );
        }
        bar.into_any_element()
    }

    /// The tabs, in the title bar, when two files or more are open.
    fn title_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        div()
            .id("bar-tabs")
            .ml(px(space::S3))
            .min_w_0()
            .flex()
            .gap(px(space::S2))
            .overflow_x_scroll()
            .children(self.tabs.iter().enumerate().map(|(i, tab)| {
                let on = i == self.active;
                let viewer = tab.viewer.read(cx);
                let title = viewer.title().unwrap_or_else(|| t("tab-empty", &[]));
                let dirty = viewer.dirty();
                div()
                    .id(("tab", i))
                    .h(px(30.0))
                    .max_w(px(220.0))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(10.0))
                    .rounded(px(6.0))
                    .text_size(px(12.5))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .map(|d| {
                        if on {
                            d.bg(if th.dark { th.field } else { th.card })
                                .text_color(th.text)
                                .shadow_sm()
                        } else {
                            d.text_color(th.dim).hover(move |s| s.bg(th.hover))
                        }
                    })
                    .on_click(cx.listener(move |this, _, window, cx| this.activate(i, window, cx)))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| this.close(i, window, cx)),
                    )
                    .child(div().min_w_0().truncate().child(title))
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .size(px(6.0))
                                .flex_none()
                                .rounded(px(3.0))
                                .bg(th.accent),
                        )
                    })
                    .when(on, |d| {
                        d.child(
                            div()
                                .id(("tab-close", i))
                                .size(px(18.0))
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .hover(move |s| s.bg(th.hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close(i, window, cx);
                                }))
                                .child(float::icon("x", 13.0, th.dim)),
                        )
                    })
            }))
    }

    /// Export, print, compare, present, read aloud, and the file's own
    /// settings (R13).
    fn share_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        type Run = fn() -> Box<dyn gpui::Action>;
        let rows: [Option<(&str, &str, &str, Run)>; 11] = [
            Some(("print", "share-print", "Ctrl P", || Box::new(Print))),
            Some(("upload", "share-export", "Ctrl Shift E", || {
                Box::new(Export)
            })),
            None,
            Some(("diff", "share-compare", "Ctrl Shift D", || {
                Box::new(Compare)
            })),
            Some(("play", "share-present", "F5", || Box::new(Present))),
            Some(("speak", "share-read-aloud", "Ctrl Shift Y", || {
                Box::new(ReadAloud)
            })),
            None,
            Some(("zip", "share-smaller", "", || Box::new(Properties))),
            Some(("book", "share-pdfa", "", || Box::new(Properties))),
            Some(("lock", "share-password", "", || Box::new(Properties))),
            Some(("info", "share-properties", "Ctrl D", || {
                Box::new(Properties)
            })),
        ];
        let items = rows.into_iter().enumerate().map(|(i, row)| match row {
            None => menu_sep(&th).into_any_element(),
            Some((glyph, words, keys, run)) => menu_item(
                ("share-item", i),
                Some(glyph),
                t(words, &[]),
                (!keys.is_empty()).then(|| SharedString::from(keys)),
                &th,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_menu(cx);
                Self::run(run(), window, cx);
            }))
            .into_any_element(),
        });
        popover(
            menu("share-menu", 280.0, &th)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
                .children(items),
            false,
            true,
        )
    }

    /// The application menu: File, Edit, View, Go, each opening beside
    /// it (S09).
    fn app_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let open = self.app_open;
        let titles = MENUS.iter().enumerate().map(|(i, (title, _))| {
            menu_item(("app-menu", i), None, plain(&t(title, &[])), None, &th)
                .when(i == open, |d| {
                    d.bg(th.accent_soft).text_color(th.accent_text)
                })
                .child(float::icon("chevr", 13.0, th.dim))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered && this.app_open != i {
                        this.app_open = i;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.app_open = i;
                    cx.notify();
                }))
        });
        let (_, entries) = MENUS[open.min(MENUS.len() - 1)];
        let items = entries.iter().enumerate().map(|(i, entry)| match entry {
            None => menu_sep(&th).into_any_element(),
            Some((id, keys)) => {
                let id = *id;
                menu_item(
                    ("app-item", i),
                    None,
                    plain(&t(id, &[])),
                    (!keys.is_empty()).then(|| SharedString::from(keys_text(keys))),
                    &th,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_menu(cx);
                    if let Some(action) = action(id) {
                        Self::run(action, window, cx);
                    }
                }))
                .into_any_element()
            }
        });
        popover(
            div()
                .id("app-menu")
                .occlude()
                .flex()
                .items_start()
                .gap(px(space::S2))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
                .child(menu("app-menu-titles", 190.0, &th).children(titles))
                .child(menu("app-menu-items", 290.0, &th).children(items)),
            false,
            true,
        )
    }

    /// The person's menu (S08): who they are, day or night, the colour
    /// schemes, the pages' colours, the app menu, updates and Quit.
    fn profile_menu(&self, colors: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let current = cx.global::<Look>().clone();
        let modes = [
            (Mode::System, None, "mode-system"),
            (Mode::Day, Some("sun"), "mode-day"),
            (Mode::Night, Some("moon"), "mode-night"),
        ]
        .map(|(mode, glyph, words)| {
            seg_item(
                ("mode", mode as usize),
                glyph,
                Some(t(words, &[]).into()),
                current.mode == mode,
                &th,
            )
            .on_click(cx.listener(move |_, _, _, cx| Look::set(cx, |look| look.mode = mode)))
        });
        let mut schemes: Vec<(String, &'static str, crate::theme::Five, crate::theme::Five)> =
            look::schemes()
                .into_iter()
                .map(|s| (s.id.to_owned(), s.name, s.day, s.night))
                .collect();
        schemes.push((
            look::SYSTEM.to_owned(),
            "scheme-system",
            current.five_for(look::SYSTEM, false),
            current.five_for(look::SYSTEM, true),
        ));
        let dots = schemes
            .into_iter()
            .enumerate()
            .map(|(i, (id, name, day, night))| {
                let on = current.scheme == id;
                let rgb = |c: u32| gpui::rgb(c);
                div()
                    .id(("scheme", i))
                    .size(px(26.0))
                    .p(px(2.0))
                    .rounded(px(13.0))
                    .cursor_pointer()
                    .when(on, |d| d.border_2().border_color(th.accent))
                    .ftip(t(name, &[]), &th)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let id = id.clone();
                        Look::set(cx, move |look| look.scheme = id);
                    }))
                    .child(
                        div()
                            .size_full()
                            .rounded(px(11.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(th.line)
                            .flex()
                            .relative()
                            .child(div().flex_1().h_full().bg(rgb(day.card)))
                            .child(div().flex_1().h_full().bg(rgb(night.page)))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(7.0))
                                    .left(px(7.0))
                                    .size(px(7.0))
                                    .rounded(px(3.5))
                                    .bg(rgb(if th.dark { night.accent } else { day.accent })),
                            ),
                    )
            });
        let update = self.update.as_ref().map(|_| {
            menu_item(
                "profile-update",
                Some("download"),
                t("profile-updates", &[]),
                Some(t("profile-update-ready", &[]).into()),
                &th,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_menu(cx);
                this.install_update(cx);
            }))
        });
        let name = self.person.clone();
        let body = menu("profile-menu", 320.0, &th)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S4))
                    .p(px(space::S4))
                    .child(avatar(&float::initials(&name), 40.0, &th))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(t(
                                        "profile-signs-as",
                                        &[("name", &crate::viewer::comment_author())],
                                    )),
                            ),
                    ),
            )
            .child(float::heading(t("profile-mode", &[]), &th))
            .child(div().px(px(space::S3)).child(seg(&th).children(modes)))
            .child(float::heading(t("profile-colours", &[]), &th))
            .child(
                div()
                    .px(px(space::S3))
                    .pb(px(space::S2))
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(dots),
            )
            .child(
                menu_item(
                    "profile-page-colours",
                    Some("moon"),
                    t("profile-page-colours", &[]),
                    Some(t(colors, &[]).into()),
                    &th,
                )
                .on_click(cx.listener(|_, _, window, cx| {
                    Self::run(Box::new(ToggleNight), window, cx);
                })),
            )
            .child(menu_sep(&th))
            .child(
                menu_item(
                    "profile-menu-item",
                    Some("menu"),
                    t("profile-menu", &[]),
                    Some(t("profile-menu-hint", &[]).into()),
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.menu = Some(TopMenu::App);
                    cx.notify();
                })),
            )
            .children(update.map(|u| u.into_any_element()))
            .child(menu_sep(&th))
            .child(
                menu_item(
                    "profile-quit",
                    None,
                    t("profile-quit", &[]),
                    Some("Ctrl Q".into()),
                    &th,
                )
                .on_click(|_, window, cx| Self::run(Box::new(Quit), window, cx)),
            );
        popover(body, false, true)
    }
}
