// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings (docs/design/float.md, X row): a window of its own, as
//! Okular's is, whose changes apply at once. A list of pages on the
//! left, Close under it; Profile (X00), Appearance (X01), Viewing (X02),
//! Editing (X03), Signing (X04), Shortcuts (X07) and About (X08) on the
//! right.

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, Global, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, size,
};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use crate::float::{self, avatar, button, icon, seg, seg_item};
use crate::look::{self, Look, Mode};
use crate::menus::{MENUS, keys_text, plain};
use crate::prefs::{Prefs, StartZoom, Units};
use crate::profile::Profile;
use crate::spread::Spread;
use crate::text::t;
use crate::theme::{Five, Theme};
use crate::viewer::SigningDefaults;

/// A page of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Profile,
    Appearance,
    Viewing,
    Editing,
    Signing,
    Shortcuts,
    About,
}

const PAGES: [(Page, &str, &str); 7] = [
    (Page::Profile, "user", "settings-profile"),
    (Page::Appearance, "sun", "settings-appearance"),
    (Page::Viewing, "eye", "settings-viewing"),
    (Page::Editing, "edit", "settings-editing"),
    (Page::Signing, "sign", "settings-signing"),
    (Page::Shortcuts, "kbd", "settings-shortcuts"),
    (Page::About, "info", "settings-about"),
];

/// The Settings window while it is open, so a second ask brings it
/// forward rather than opening another.
struct Shown(Option<WindowHandle<Settings>>);

impl Global for Shown {}

/// Opens Settings at `page`, or turns the open window to it.
pub fn open(page: Page, cx: &mut App) {
    if let Some(handle) = cx.try_global::<Shown>().and_then(|s| s.0) {
        let turned = handle.update(cx, |view, window, cx| {
            view.page = page;
            window.activate_window();
            cx.notify();
        });
        if turned.is_ok() {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(desktop_px(780.0), desktop_px(580.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        app_id: Some(crate::ids::app_id()),
        titlebar: Some(TitlebarOptions {
            title: Some(t("settings-title", &[]).into()),
            ..Default::default()
        }),
        window_min_size: Some(size(desktop_px(560.0), desktop_px(400.0))),
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| Settings::new(page, window, cx))
    }) {
        Ok(handle) => cx.set_global(Shown(Some(handle))),
        Err(err) => eprintln!("katna-pdf: cannot open Settings: {err}"),
    }
}

pub struct Settings {
    page: Page,
    theme: Theme,
    name: Entity<TextInput>,
    short: Entity<TextInput>,
    title: Entity<TextInput>,
    organisation: Entity<TextInput>,
    find: Entity<TextInput>,
    /// The timestamp server's address (Signing).
    server: Entity<TextInput>,
    signing: SigningDefaults,
    _subs: Vec<Subscription>,
}

impl Settings {
    fn new(page: Page, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profile = cx.global::<Profile>().clone();
        let accent = Theme::current(window, cx.global()).accent;
        let input = |placeholder: &str, value: &str, cx: &mut Context<Self>| {
            let placeholder = t(placeholder, &[]);
            let value = value.to_owned();
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input.set_text(value, cx);
                input
            })
        };
        let name = input("settings-name-hint", &profile.name, cx);
        let short = input("settings-short-hint", &profile.short, cx);
        let title = input("settings-job-hint", &profile.title, cx);
        let organisation = input("settings-organisation-hint", &profile.organisation, cx);
        let find = input("settings-find-shortcut", "", cx);
        let signing = crate::viewer::signing_defaults();
        let server = input("settings-server-hint", &signing.server, cx);
        let mut subs = vec![
            cx.observe_window_appearance(window, |this, window, cx| this.restyle(window, cx)),
            cx.observe_global_in::<Look>(window, |this, window, cx| this.restyle(window, cx)),
            cx.subscribe(&find, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&server, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Changed) {
                    this.signing.server = this.server.read(cx).text().to_string();
                    crate::viewer::set_signing_defaults(&this.signing);
                }
            }),
        ];
        for field in [&name, &short, &title, &organisation] {
            subs.push(cx.subscribe(field, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Changed) {
                    this.keep_profile(cx);
                }
            }));
        }
        window.on_window_should_close(cx, |_, cx| {
            cx.set_global(Shown(None));
            true
        });
        Self {
            page,
            theme: Theme::current(window, cx.global()),
            name,
            short,
            title,
            organisation,
            find,
            server,
            signing,
            _subs: subs,
        }
    }

    fn restyle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.theme = Theme::current(window, cx.global());
        let accent = self.theme.accent;
        for field in [
            &self.name,
            &self.short,
            &self.title,
            &self.organisation,
            &self.find,
            &self.server,
        ] {
            field.update(cx, |input, _| input.set_accent(accent));
        }
        cx.notify();
    }

    /// Keeps what the profile's boxes hold, and tells the other windows.
    fn keep_profile(&mut self, cx: &mut Context<Self>) {
        let profile = Profile {
            name: self.name.read(cx).text().to_string(),
            short: self.short.read(cx).text().to_string(),
            title: self.title.read(cx).text().to_string(),
            organisation: self.organisation.read(cx).text().to_string(),
        };
        if &profile != cx.global::<Profile>() {
            profile.save();
            cx.set_global(profile);
        }
        cx.notify();
    }

    fn side(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let rows = PAGES.iter().enumerate().map(|(i, &(page, glyph, words))| {
            let on = self.page == page;
            let fg = if on { th.accent_text } else { th.text };
            let hover = th.hover;
            div()
                .id(("settings-page", i))
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .text_size(px(text::BODY))
                .text_color(fg)
                .map(|d| {
                    if on {
                        d.bg(th.accent_soft).font_weight(FontWeight::SEMIBOLD)
                    } else {
                        d.hover(move |s| s.bg(hover))
                    }
                })
                .child(icon(glyph, 16.0, if on { fg } else { th.dim }))
                .child(t(words, &[]))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }))
        });
        div()
            .w(px(200.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .p(px(space::S3))
            .gap(px(2.0))
            .bg(th.page)
            .border_r_1()
            .border_color(th.line_soft)
            .child(
                div()
                    .px(px(10.0))
                    .pt(px(space::S3))
                    .pb(px(space::S4))
                    .text_size(px(text::SUBTITLE))
                    .font_weight(FontWeight::BOLD)
                    .text_color(th.text)
                    .child(t("settings-title", &[])),
            )
            .children(rows)
            .child(div().flex_1())
            .child(
                button("settings-close", t("settings-close", &[]), false, &th)
                    .w_full()
                    .on_click(|_, window, cx| {
                        cx.set_global(Shown(None));
                        window.remove_window();
                    }),
            )
    }

    /// A labelled box for one of the profile's words.
    fn boxed(&self, label: &str, input: &Entity<TextInput>) -> impl IntoElement {
        let th = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .min_w_0()
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.dim)
                    .child(t(label, &[])),
            )
            .child(
                div()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(th.line)
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(input.clone()),
            )
    }

    fn profile(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let name = cx.global::<Profile>().display_name();
        let initials = crate::viewer::kept_initials();
        let stamps = crate::viewer::kept_stamps();
        let picture = crate::viewer::signature_picture();
        let none = t("settings-none-yet", &[]);
        let joined = |list: &[String]| {
            if list.is_empty() {
                none.clone()
            } else {
                list.join(" · ")
            }
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .gap(px(space::S5))
                    .child(avatar(&float::initials(&name), 76.0, &th))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(space::S3))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S3))
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(self.boxed("settings-name", &self.name)),
                                    )
                                    .child(
                                        div()
                                            .w(px(150.0))
                                            .child(self.boxed("settings-short", &self.short)),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S3))
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(self.boxed("settings-job", &self.title)),
                                    )
                                    .child(div().flex_1().child(
                                        self.boxed("settings-organisation", &self.organisation),
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(px(space::S4))
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(t("settings-profile-detail", &[])),
            )
            .child(self.row(
                t("settings-initials", &[]),
                Some(joined(&initials)),
                Some(t("settings-initials-where", &[])),
                None,
            ))
            .child(self.row(
                t("settings-signature-picture", &[]),
                Some(picture.unwrap_or_else(|| none.clone())),
                Some(t("settings-signature-picture-where", &[])),
                None,
            ))
            .child(self.row(
                t("settings-stamps", &[]),
                Some(joined(&stamps)),
                Some(t("settings-stamps-where", &[])),
                None,
            ))
            .into_any_element()
    }

    /// One row of a page: its words, what it holds now, a hint, and a
    /// control on the right, over a line.
    fn row(
        &self,
        words: String,
        value: Option<String>,
        hint: Option<String>,
        control: Option<AnyElement>,
    ) -> impl IntoElement {
        let th = self.theme;
        div()
            .mt(px(space::S3))
            .pt(px(space::S4))
            .border_t_1()
            .border_color(th.line_soft)
            .flex()
            .items_center()
            .gap(px(space::S4))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(words),
                    )
                    .children(value.map(|v| {
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .truncate()
                            .child(v)
                    }))
                    .children(hint.map(|h| {
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .opacity(0.8)
                            .child(h)
                    })),
            )
            .children(control)
    }

    fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let current = cx.global::<Look>().clone();
        let modes = [
            (Mode::System, None, "mode-system"),
            (Mode::Day, Some("sun"), "mode-day"),
            (Mode::Night, Some("moon"), "mode-night"),
        ]
        .map(|(mode, glyph, words)| {
            seg_item(
                ("settings-mode", mode as usize),
                glyph,
                Some(t(words, &[]).into()),
                current.mode == mode,
                &th,
            )
            .on_click(cx.listener(move |_, _, _, cx| Look::set(cx, |look| look.mode = mode)))
        });
        let tile = |i: usize,
                    id: String,
                    name: SharedString,
                    day: Five,
                    night: Five,
                    cx: &mut Context<Self>| {
            let on = current.scheme == id;
            let rgb = |c: u32| gpui::rgb(c);
            let half = |five: Five| {
                div()
                    .flex_1()
                    .h_full()
                    .p(px(5.0))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .bg(rgb(five.page))
                    .child(
                        div()
                            .p(px(3.0))
                            .rounded(px(2.0))
                            .bg(rgb(five.card))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(div().h(px(2.0)).w(px(12.0)).bg(rgb(five.text)))
                            .child(div().h(px(2.0)).w(px(18.0)).bg(rgb(five.faint))),
                    )
                    .child(
                        div()
                            .h(px(4.0))
                            .w(px(14.0))
                            .rounded(px(2.0))
                            .bg(rgb(five.accent)),
                    )
            };
            div()
                .id(("settings-scheme", i))
                .w(px(70.0))
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| {
                    let id = id.clone();
                    Look::set(cx, move |look| look.scheme = id);
                }))
                .child(
                    div()
                        .h(px(46.0))
                        .rounded(px(7.0))
                        .p(px(if on { 2.0 } else { 0.0 }))
                        .when(on, |d| d.border_2().border_color(th.accent))
                        .child(
                            div()
                                .size_full()
                                .rounded(px(5.0))
                                .overflow_hidden()
                                .border_1()
                                .border_color(th.line)
                                .flex()
                                .child(half(day))
                                .child(half(night)),
                        ),
                )
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(if on { th.accent_text } else { th.text })
                        .when(on, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .truncate()
                        .child(name),
                )
        };
        let built_in: Vec<_> = look::schemes()
            .into_iter()
            .enumerate()
            .map(|(i, s)| {
                tile(
                    i,
                    s.id.to_owned(),
                    t(s.name, &[]).into(),
                    s.day,
                    s.night,
                    cx,
                )
            })
            .collect();
        let system = tile(
            1000,
            look::SYSTEM.to_owned(),
            t("scheme-system", &[]).into(),
            current.five_for(look::SYSTEM, false),
            current.five_for(look::SYSTEM, true),
            cx,
        );
        let heading = |words: &str| {
            div()
                .mt(px(space::S4))
                .mb(px(space::S2))
                .text_size(px(text::MICRO))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(th.dim)
                .child(t(words, &[]).to_uppercase())
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("profile-mode", &[])),
                    )
                    .child(div().w(px(260.0)).child(seg(&th).children(modes))),
            )
            .child(
                div()
                    .mt(px(space::S5))
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(t("profile-colours", &[])),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(t("settings-colours-detail", &[])),
            )
            .child(heading("settings-built-in"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S3))
                    .children(built_in),
            )
            .child(heading("settings-from-desktop"))
            .child(div().flex().child(system))
            .into_any_element()
    }

    /// A choice of a few, as a segmented control that changes `Prefs`.
    fn choice<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        current: T,
        options: &[(T, &'static str)],
        set: fn(&mut Prefs, T),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let items = options.iter().enumerate().map(|(i, &(value, words))| {
            seg_item(
                (id, i),
                None,
                Some(t(words, &[]).into()),
                current == value,
                &th,
            )
            .flex_none()
            .on_click(cx.listener(move |_, _, _, cx| Prefs::set(cx, |p| set(p, value))))
        });
        seg(&th).children(items).into_any_element()
    }

    /// An on and off switch that changes `Prefs`.
    fn toggle(
        &self,
        id: &'static str,
        on: bool,
        set: fn(&mut Prefs, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        float::switch(id, on, &self.theme)
            .on_click(cx.listener(move |_, _, _, cx| Prefs::set(cx, |p| set(p, !on))))
            .into_any_element()
    }

    fn viewing(&self, cx: &mut Context<Self>) -> AnyElement {
        let prefs = cx.global::<Prefs>().clone();
        let open_at = self.choice(
            "settings-open-at",
            prefs.open_at_last,
            &[(true, "settings-open-last"), (false, "settings-open-first")],
            |p, v| p.open_at_last = v,
            cx,
        );
        let zoom = self.choice(
            "settings-zoom",
            prefs.zoom,
            &[
                (StartZoom::FitWidth, "settings-zoom-width"),
                (StartZoom::FitPage, "settings-zoom-page"),
                (StartZoom::Actual, "settings-zoom-actual"),
            ],
            |p, v| p.zoom = v,
            cx,
        );
        let layout = self.choice(
            "settings-layout",
            prefs.layout,
            &[
                (Spread::Single, "settings-layout-one"),
                (Spread::Facing, "settings-layout-two"),
                (Spread::Book, "settings-layout-book"),
            ],
            |p, v| p.layout = v,
            cx,
        );
        let reopen = self.toggle(
            "settings-reopen",
            prefs.reopen_session,
            |p, v| p.reopen_session = v,
            cx,
        );
        let workers = self.toggle("settings-workers", prefs.workers, |p, v| p.workers = v, cx);
        div()
            .flex()
            .flex_col()
            .child(self.row(t("settings-open-at", &[]), None, None, Some(open_at)))
            .child(self.row(
                t("settings-zoom", &[]),
                None,
                Some(t("settings-zoom-detail", &[])),
                Some(zoom),
            ))
            .child(self.row(
                t("settings-layout", &[]),
                None,
                Some(t("settings-zoom-detail", &[])),
                Some(layout),
            ))
            .child(self.row(
                t("settings-reopen", &[]),
                None,
                Some(t("settings-reopen-detail", &[])),
                Some(reopen),
            ))
            .child(self.row(
                t("settings-workers", &[]),
                None,
                Some(t("settings-workers-detail", &[])),
                Some(workers),
            ))
            .into_any_element()
    }

    fn editing(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let prefs = cx.global::<Prefs>().clone();
        let units = self.choice(
            "settings-units",
            prefs.units,
            &[
                (Units::Millimetres, "settings-units-mm"),
                (Units::Inches, "settings-units-in"),
                (Units::Points, "settings-units-pt"),
            ],
            |p, v| p.units = v,
            cx,
        );
        let grid = self.toggle("settings-grid", prefs.grid, |p, v| p.grid = v, cx);
        let folder = katna_core::Paths::from_env()
            .ok()
            .map(|p| p.state_dir().join("pdf"))
            .map(|dir| open_folder("settings-own-folder", dir, &th, cx));
        div()
            .flex()
            .flex_col()
            .child(self.row(
                t("settings-units", &[]),
                None,
                Some(t("settings-units-detail", &[])),
                Some(units),
            ))
            .child(self.row(
                t("settings-grid", &[]),
                None,
                Some(t("settings-grid-detail", &[])),
                Some(grid),
            ))
            .child(self.row(t("settings-own-files", &[]), None, None, folder))
            .into_any_element()
    }

    /// An on and off switch for what signing starts with.
    fn signing_switch(
        &self,
        id: &'static str,
        on: bool,
        change: fn(&mut SigningDefaults, bool),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        float::switch(id, on, &self.theme)
            .on_click(cx.listener(move |this, _, _, cx| {
                change(&mut this.signing, !on);
                crate::viewer::set_signing_defaults(&this.signing);
                cx.notify();
            }))
            .into_any_element()
    }

    fn signing(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let drivers: Vec<String> = katna_pdf_sign::pkcs11::known_modules()
            .iter()
            .filter_map(|m| m.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect();
        let drivers = if drivers.is_empty() {
            t("settings-drivers-none", &[])
        } else {
            drivers.join(" · ")
        };
        let defaults = self.signing.clone();
        let timestamp = self.signing_switch(
            "settings-timestamp",
            defaults.timestamp,
            |d, v| d.timestamp = v,
            cx,
        );
        let long_term = self.signing_switch(
            "settings-long-term",
            defaults.long_term,
            |d, v| d.long_term = v,
            cx,
        );
        let folder = crate::viewer::trusted_folder();
        let yours = folder
            .as_ref()
            .and_then(|f| std::fs::read_dir(f).ok())
            .map_or(0, |entries| entries.flatten().count());
        let open = folder.map(|dir| open_folder("settings-trusted-folder", dir, &th, cx));
        let server = div()
            .w(px(240.0))
            .h(px(32.0))
            .flex()
            .items_center()
            .px(px(space::S3))
            .rounded(px(6.0))
            .border_1()
            .border_color(th.line)
            .bg(th.field)
            .text_size(px(text::BODY))
            .child(self.server.clone())
            .into_any_element();
        div()
            .flex()
            .flex_col()
            .child(self.row(
                t("settings-drivers", &[]),
                Some(drivers),
                Some(t("settings-drivers-detail", &[])),
                None,
            ))
            .child(self.row(
                t("settings-timestamp", &[]),
                None,
                Some(t("settings-timestamp-detail", &[])),
                Some(timestamp),
            ))
            .child(self.row(t("settings-server", &[]), None, None, Some(server)))
            .child(self.row(
                t("settings-long-term", &[]),
                None,
                Some(t("settings-long-term-detail", &[])),
                Some(long_term),
            ))
            .child(self.row(
                t("settings-trusted", &[]),
                Some(t(
                    "settings-trusted-detail",
                    &[("count", &yours.to_string())],
                )),
                None,
                open,
            ))
            .into_any_element()
    }

    fn shortcuts(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let find = self.find.read(cx).text().to_lowercase();
        let rows = MENUS
            .iter()
            .flat_map(|(_, entries)| entries.iter().flatten())
            .filter(|(_, keys)| !keys.is_empty())
            .map(|(id, keys)| (plain(&t(id, &[])), keys_text(keys)))
            .filter(|(words, keys)| {
                find.is_empty()
                    || words.to_lowercase().contains(&find)
                    || keys.to_lowercase().contains(&find)
            })
            .map(|(words, keys)| {
                div()
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(th.line_soft)
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(words),
                    )
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .border_1()
                            .border_color(th.line)
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(keys),
                    )
            });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(space::S3))
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S3))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(th.line)
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(icon("search", 14.0, th.dim))
                    .child(div().flex_1().child(self.find.clone())),
            )
            .children(rows)
            .into_any_element()
    }

    fn about(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let version = match crate::update::VERSION {
            Some(v) => t("settings-version", &[("version", v)]),
            None => t("settings-version-dev", &[]),
        };
        let crash = katna_core::Paths::from_env().ok().map(|p| p.crash_dir());
        let open = crash.map(|dir| open_folder("settings-crash-folder", dir, &th, cx));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(space::S4))
                    .h(px(40.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .rounded(px(7.0))
                    .bg(th.accent_soft)
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.accent_text)
                    .child(icon("info", 15.0, th.accent_text))
                    .child(version),
            )
            .child(self.row(
                t("settings-updates", &[]),
                Some(t(
                    if crate::update::VERSION.is_some() {
                        "settings-updates-detail"
                    } else {
                        "settings-updates-off"
                    },
                    &[],
                )),
                None,
                None,
            ))
            .child(self.row(
                t("settings-crash-reports", &[]),
                Some(t("settings-crash-reports-detail", &[])),
                None,
                open,
            ))
            .into_any_element()
    }
}

/// A button that shows `dir` in the desktop's file manager, made first
/// when it is not there yet.
fn open_folder(
    id: &'static str,
    dir: std::path::PathBuf,
    th: &Theme,
    cx: &mut Context<Settings>,
) -> AnyElement {
    button(id, t("settings-open-folder", &[]), false, th)
        .on_click(cx.listener(move |_, _, _, cx| {
            std::fs::create_dir_all(&dir).ok();
            cx.open_url(&format!("file://{}", dir.to_string_lossy()));
        }))
        .into_any_element()
}

impl Render for Settings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let (title, body) = match self.page {
            Page::Profile => ("settings-profile", self.profile(cx)),
            Page::Appearance => ("settings-appearance", self.appearance(cx)),
            Page::Viewing => ("settings-viewing", self.viewing(cx)),
            Page::Editing => ("settings-editing", self.editing(cx)),
            Page::Signing => ("settings-signing", self.signing(cx)),
            Page::Shortcuts => ("settings-shortcuts", self.shortcuts(cx)),
            Page::About => ("settings-about", self.about(cx)),
        };
        div()
            .id("settings")
            .size_full()
            .flex()
            .bg(th.card)
            .text_color(th.text)
            .font_family(".SystemUIFont")
            .child(self.side(cx))
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .px(px(28.0))
                    .py(px(space::S5))
                    .child(
                        div()
                            .mb(px(space::S5))
                            .text_size(px(text::TITLE))
                            .font_weight(FontWeight::BOLD)
                            .child(t(title, &[])),
                    )
                    .child(body),
            )
    }
}
