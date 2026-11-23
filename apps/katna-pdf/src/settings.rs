// SPDX-License-Identifier: GPL-3.0-or-later

//! Settings (docs/design/float.md, X row): a window of its own, as
//! Okular's is, whose changes apply at once. A list of pages on the
//! left, Close under it; Profile (X00), Appearance (X01), Viewing (X02),
//! Editing (X03), Signing (X04), Library (X05), Read aloud (X06),
//! Shortcuts (X07) and About (X08) on the right.

use gpui::{
    AnyElement, App, Bounds, Context, Entity, FontWeight, Global, SharedString, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, size,
};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};

use crate::float::{self, avatar, button, icon, radio, seg, seg_item};
use crate::look::{self, Look, Mine, Mode};
use crate::menus::{MENUS, keys_text, plain};
use crate::prefs::{self, Motion, Prefs, StartZoom, Units, WebLinks};
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
    Library,
    ReadAloud,
    Shortcuts,
    About,
}

const PAGES: [(Page, &str, &str); 9] = [
    (Page::Profile, "user", "settings-profile"),
    (Page::Appearance, "sun", "settings-appearance"),
    (Page::Viewing, "eye", "settings-viewing"),
    (Page::Editing, "edit", "settings-editing"),
    (Page::Signing, "sign", "settings-signing"),
    (Page::Library, "lib", "settings-library"),
    (Page::ReadAloud, "speak", "settings-read-aloud"),
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
    email: Entity<TextInput>,
    find: Entity<TextInput>,
    /// The timestamp server's address (Signing).
    server: Entity<TextInput>,
    signing: SigningDefaults,
    /// The library's watched folders and the files found in each.
    folders: Vec<(std::path::PathBuf, usize)>,
    /// A scheme being made or changed (X10).
    editor: Option<Editor>,
    /// Why the last scheme file could not be brought in (X11).
    notice: Option<String>,
    /// Whether the menu of pages at night is open (X09).
    night_menu: bool,
    /// The voices that can read aloud, once asked for (opening the
    /// voice is slow, so done off the UI thread).
    voices: Option<Vec<(String, String)>>,
    _subs: Vec<Subscription>,
}

/// The scheme editor's state: its name, which side is shown, and the
/// five colours of each side as typed.
struct Editor {
    /// The scheme changed, or a new id.
    id: String,
    name: Entity<TextInput>,
    night: bool,
    /// Day's five, then night's: page, card, text, faint, accent.
    fields: [[Entity<TextInput>; 5]; 2],
    _subs: Vec<Subscription>,
}

const FIVE_WORDS: [&str; 5] = [
    "settings-colour-page",
    "settings-colour-card",
    "settings-colour-text",
    "settings-colour-faint",
    "settings-colour-accent",
];

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
        let email = input("settings-email-hint", &profile.email, cx);
        let find = input("settings-find-shortcut", "", cx);
        let signing = crate::viewer::signing_defaults();
        let server = input("settings-server-hint", &signing.server, cx);
        let mut subs = vec![
            cx.observe_window_appearance(window, |this, window, cx| this.restyle(window, cx)),
            cx.observe_global_in::<Look>(window, |this, window, cx| this.restyle(window, cx)),
            cx.observe_global::<Prefs>(|_, cx| cx.notify()),
            cx.subscribe(&find, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&server, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Changed) {
                    this.signing.server = this.server.read(cx).text().to_string();
                    crate::viewer::set_signing_defaults(&this.signing);
                }
            }),
        ];
        for field in [&name, &short, &title, &organisation, &email] {
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
        cx.spawn(async move |this, cx| {
            let voices = cx
                .background_spawn(async move {
                    crate::speech::voice()
                        .map(|v| v.voices())
                        .unwrap_or_default()
                })
                .await;
            this.update(cx, |this, cx| {
                this.voices = Some(voices);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            page,
            theme: Theme::current(window, cx.global()),
            name,
            short,
            title,
            organisation,
            email,
            find,
            server,
            signing,
            folders: watched(),
            editor: None,
            notice: None,
            night_menu: false,
            voices: None,
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
            &self.email,
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
            email: self.email.read(cx).text().to_string(),
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
                            )
                            .child(self.boxed("settings-email", &self.email)),
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

    /// A heading over the rows that follow it on a page.
    fn group(&self, id: &str) -> impl IntoElement {
        div()
            .mt(px(space::S6))
            .text_size(px(text::CAPTION))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(self.theme.dim)
            .child(t(id, &[]))
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
        let mine: Vec<_> = current
            .mine
            .iter()
            .enumerate()
            .map(|(i, m)| {
                tile(
                    2000 + i,
                    m.id.clone(),
                    m.name.clone().into(),
                    m.day,
                    m.night,
                    cx,
                )
                .into_any_element()
            })
            .collect();
        let add_tile = |id: &'static str, glyph: &'static str, words: &'static str| {
            div()
                .id(id)
                .w(px(70.0))
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .cursor_pointer()
                .child(
                    div()
                        .h(px(46.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_dashed()
                        .border_color(th.line)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(glyph, 16.0, th.dim)),
                )
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .truncate()
                        .child(t(words, &[])),
                )
        };
        let make = add_tile("settings-scheme-make", "plus", "settings-scheme-make")
            .on_click(cx.listener(|this, _, window, cx| this.open_editor(None, window, cx)))
            .into_any_element();
        let import = add_tile("settings-scheme-import", "upload", "settings-scheme-import")
            .on_click(cx.listener(|this, _, window, cx| this.import_scheme(window, cx)))
            .into_any_element();
        let chosen_mine = current
            .mine
            .iter()
            .find(|m| m.id == current.scheme)
            .cloned();
        let own_actions = chosen_mine.map(|m| {
            let id = m.id.clone();
            div()
                .mt(px(space::S3))
                .flex()
                .gap(px(space::S2))
                .child(
                    button(
                        "settings-scheme-change",
                        t("settings-scheme-change", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_editor(Some(m.clone()), window, cx)
                    })),
                )
                .child(
                    button(
                        "settings-scheme-delete",
                        t("settings-scheme-delete", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let id = id.clone();
                        Look::set(cx, move |look| {
                            look.mine.retain(|m| m.id != id);
                            look.scheme = look::FLOAT.into();
                        });
                    })),
                )
        });
        let notice = self.notice.clone().map(|words| {
            div()
                .mt(px(space::S3))
                .p(px(space::S3))
                .flex()
                .gap(px(space::S2))
                .rounded(px(7.0))
                .bg(th.error_soft)
                .text_color(th.error)
                .text_size(px(text::CAPTION))
                .child(icon("alert", 14.0, th.error))
                .child(words)
        });
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
            .child(
                div()
                    .flex()
                    .gap(px(space::S6))
                    .child(
                        div()
                            .flex_none()
                            .child(heading("settings-from-desktop"))
                            .child(system),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(heading("settings-yours"))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap(px(space::S3))
                                    .children(mine)
                                    .child(make)
                                    .child(import),
                            ),
                    ),
            )
            .children(own_actions)
            .children(notice)
            .child(self.accent_row(&current, cx))
            .child(self.appearance_more(&current, cx))
            .into_any_element()
    }

    /// The accent over the scheme's own (X01): the scheme's, or one of
    /// Katna Mail's seven.
    fn accent_row(&self, current: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        use katna_ui::theme::Accent;
        let th = self.theme;
        let picked = current.accent;
        let ring = |on: bool| {
            div()
                .size(px(26.0))
                .flex_none()
                .rounded_full()
                .p(px(2.0))
                .when(on, |d| d.border_2().border_color(th.text))
        };
        let own = div()
            .id("settings-accent-own")
            .cursor_pointer()
            .child(
                ring(picked == Accent::Scheme).child(
                    div()
                        .size_full()
                        .rounded_full()
                        .border_1()
                        .border_dashed()
                        .border_color(th.dim),
                ),
            )
            .on_click(cx.listener(|_, _, _, cx| Look::set(cx, |l| l.accent = Accent::Scheme)));
        let swatches = look::ACCENTS.iter().enumerate().map(|(i, &rgb)| {
            let accent = Accent::Color(rgb << 8 | 0xff);
            div()
                .id(("settings-accent", i))
                .cursor_pointer()
                .child(
                    ring(picked == accent)
                        .child(div().size_full().rounded_full().bg(gpui::rgb(rgb))),
                )
                .on_click(cx.listener(move |_, _, _, cx| Look::set(cx, |l| l.accent = accent)))
        });
        let caption = match picked {
            Accent::Scheme => "settings-accent-own",
            _ => "settings-accent-mine",
        };
        div()
            .mt(px(space::S5))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .child(
                div()
                    .w(px(60.0))
                    .flex_none()
                    .text_size(px(text::BODY))
                    .child(t("settings-accent", &[])),
            )
            .child(own)
            .children(swatches)
            .child(
                div()
                    .flex_1()
                    .min_w(px(80.0))
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .truncate()
                    .child(t(caption, &[])),
            )
    }

    /// Appearance's other rows (X09): the pages at night, the size of
    /// everything, the dock's labels and Katna Mail's colours.
    fn appearance_more(&self, current: &Look, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let prefs = cx.global::<Prefs>().clone();
        let night = crate::viewer::colors_text(prefs.night_pages);
        let menu = self.night_menu.then(|| {
            let items = katna_pdf_render::recolor::PageColors::ALL
                .into_iter()
                .enumerate()
                .map(|(i, colors)| {
                    float::menu_item(
                        ("settings-night-pages", i),
                        (colors == prefs.night_pages).then_some("check"),
                        t(crate::viewer::colors_text(colors), &[]),
                        None,
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.night_menu = false;
                        Prefs::set(cx, |p| p.night_pages = colors);
                    }))
                });
            float::popover(
                float::menu("settings-night-menu", 200.0, &th).children(items),
                false,
                true,
            )
        });
        let pages = div()
            .relative()
            .child(
                div()
                    .id("settings-night-button")
                    .h(px(30.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(th.line)
                    .cursor_pointer()
                    .text_size(px(text::BODY))
                    .child(t(night, &[]))
                    .child(icon("chev", 14.0, th.dim))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.night_menu = !this.night_menu;
                        cx.notify();
                    })),
            )
            .children(menu)
            .into_any_element();
        let size = {
            let items = prefs::SIZES.into_iter().enumerate().map(|(i, value)| {
                seg_item(
                    ("settings-size", i),
                    None,
                    Some(size_words(value).into()),
                    (prefs.size - value).abs() < 0.001,
                    &th,
                )
                .flex_none()
                .on_click(cx.listener(move |_, _, _, cx| Prefs::set(cx, |p| p.size = value)))
            });
            seg(&th).children(items).into_any_element()
        };
        let labels = self.toggle(
            "settings-dock-labels",
            prefs.dock_labels,
            |p, v| p.dock_labels = v,
            cx,
        );
        let motion = self.choice(
            "settings-motion",
            prefs.motion,
            &[
                (Motion::Desktop, "settings-motion-desktop"),
                (Motion::Less, "settings-motion-less"),
                (Motion::Normal, "settings-motion-normal"),
            ],
            |p, v| p.motion = v,
            cx,
        );
        let desktop_still = crate::prefs::desktop_motion().off();
        let follow = look::mail_config_file().map(|_| {
            let on = current.follow_mail;
            float::switch("settings-follow-mail", on, &th)
                .on_click(cx.listener(move |_, _, _, cx| {
                    Look::set(cx, |l| {
                        l.follow_mail = !on;
                        if l.follow_mail {
                            l.take_mail();
                        }
                    })
                }))
                .into_any_element()
        });
        div()
            .mt(px(space::S4))
            .flex()
            .flex_col()
            .child(self.row(
                t("settings-night-pages", &[]),
                None,
                Some(t("settings-night-pages-detail", &[])),
                Some(pages),
            ))
            .child(self.row(
                t("settings-size", &[]),
                None,
                Some(t("settings-size-detail", &[])),
                Some(size),
            ))
            .child(self.row(
                t("settings-dock-labels", &[]),
                None,
                Some(t("settings-dock-labels-detail", &[])),
                Some(labels),
            ))
            .child(self.row(
                t("settings-motion", &[]),
                Some(t(
                    if desktop_still {
                        "settings-motion-desktop-still"
                    } else {
                        "settings-motion-desktop-moving"
                    },
                    &[],
                )),
                Some(t("settings-motion-detail", &[])),
                Some(motion),
            ))
            .children(follow.map(|follow| {
                self.row(
                    t("settings-follow-mail", &[]),
                    None,
                    Some(t("settings-follow-mail-detail", &[])),
                    Some(follow),
                )
            }))
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
        let web_links = self.choice(
            "settings-web-links",
            prefs.web_links,
            &[
                (WebLinks::Ask, "settings-web-ask"),
                (WebLinks::Open, "settings-web-open"),
                (WebLinks::Never, "settings-web-never"),
            ],
            |p, v| p.web_links = v,
            cx,
        );
        let forget_sites = (!prefs.web_sites.is_empty()).then(|| {
            float::button(
                "settings-web-forget",
                t("settings-web-forget", &[]),
                false,
                &self.theme,
            )
            .on_click(cx.listener(|_, _, _, cx| Prefs::set(cx, |p| p.web_sites.clear())))
            .into_any_element()
        });
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
        let new_window = self.toggle(
            "settings-new-window",
            prefs.new_window,
            |p, v| p.new_window = v,
            cx,
        );
        let workers = self.toggle("settings-workers", prefs.workers, |p, v| p.workers = v, cx);
        let recent = self.choice(
            "settings-recent",
            prefs.recent,
            &[
                (4, "settings-recent-4"),
                (8, "settings-recent-8"),
                (16, "settings-recent-16"),
            ],
            |p, v| p.recent = v,
            cx,
        );
        let find_case = self.toggle(
            "settings-find-case",
            prefs.find_case,
            |p, v| p.find_case = v,
            cx,
        );
        let find_words = self.toggle(
            "settings-find-words",
            prefs.find_words,
            |p, v| p.find_words = v,
            cx,
        );
        let find_accents = self.toggle(
            "settings-find-accents",
            prefs.find_accents,
            |p, v| p.find_accents = v,
            cx,
        );
        let present_seconds = self.choice(
            "settings-present-seconds",
            prefs.present_seconds,
            &[
                (0, "settings-present-keys"),
                (5, "settings-seconds-5"),
                (10, "settings-seconds-10"),
                (30, "settings-seconds-30"),
            ],
            |p, v| p.present_seconds = v,
            cx,
        );
        let present_loop = self.toggle(
            "settings-present-loop",
            prefs.present_loop,
            |p, v| p.present_loop = v,
            cx,
        );
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
                t("settings-new-window", &[]),
                None,
                Some(t("settings-new-window-detail", &[])),
                Some(new_window),
            ))
            .child(self.row(
                t("settings-workers", &[]),
                None,
                Some(t("settings-workers-detail", &[])),
                Some(workers),
            ))
            .child(self.row(
                t("settings-recent", &[]),
                None,
                Some(t("settings-recent-detail", &[])),
                Some(recent),
            ))
            .child(self.group("settings-group-find"))
            .child(self.row(
                t("settings-find-case", &[]),
                None,
                Some(t("settings-find-detail", &[])),
                Some(find_case),
            ))
            .child(self.row(t("settings-find-words", &[]), None, None, Some(find_words)))
            .child(self.row(
                t("settings-find-accents", &[]),
                None,
                None,
                Some(find_accents),
            ))
            .child(self.group("settings-group-present"))
            .child(self.row(
                t("settings-present-seconds", &[]),
                None,
                Some(t("settings-present-seconds-detail", &[])),
                Some(present_seconds),
            ))
            .child(self.row(
                t("settings-present-loop", &[]),
                None,
                None,
                Some(present_loop),
            ))
            .child(self.group("settings-group-web"))
            .child(self.row(
                t("settings-web-links", &[]),
                None,
                Some(t("settings-web-links-detail", &[])),
                Some(web_links),
            ))
            .when(prefs.web_links == WebLinks::Ask, |d| {
                d.child(self.row(
                    t("settings-web-sites", &[]),
                    Some(if prefs.web_sites.is_empty() {
                        t("settings-web-sites-none", &[])
                    } else {
                        prefs.web_sites.join(", ")
                    }),
                    None,
                    forget_sites,
                ))
            })
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
        let scripts = self.toggle(
            "settings-form-scripts",
            prefs.form_scripts,
            |p, v| p.form_scripts = v,
            cx,
        );
        let recovery = self.toggle(
            "settings-recovery",
            prefs.recovery,
            |p, v| p.recovery = v,
            cx,
        );
        let every = self.choice(
            "settings-recovery-every",
            prefs.recovery_minutes,
            &[
                (1, "settings-minutes-1"),
                (5, "settings-minutes-5"),
                (10, "settings-minutes-10"),
            ],
            |p, v| p.recovery_minutes = v,
            cx,
        );
        let hover = self.toggle(
            "settings-comment-hover",
            prefs.comment_hover,
            |p, v| p.comment_hover = v,
            cx,
        );
        let every_comment = self.toggle(
            "settings-comment-every",
            prefs.comment_every,
            |p, v| p.comment_every = v,
            cx,
        );
        let tint = self.toggle(
            "settings-field-tint",
            prefs.field_tint,
            |p, v| p.field_tint = v,
            cx,
        );
        let calendar = self.toggle(
            "settings-date-calendar",
            prefs.date_calendar,
            |p, v| p.date_calendar = v,
            cx,
        );
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
            .child(self.row(
                t("settings-recovery", &[]),
                None,
                Some(t("settings-recovery-detail", &[])),
                Some(recovery),
            ))
            .when(prefs.recovery, |d| {
                d.child(self.row(t("settings-recovery-every", &[]), None, None, Some(every)))
            })
            .child(self.group("settings-group-comments"))
            .child(self.row(
                t("settings-comment-hover", &[]),
                None,
                Some(t("settings-comment-hover-detail", &[])),
                Some(hover),
            ))
            .child(self.row(
                t("settings-comment-every", &[]),
                None,
                Some(t("settings-comment-every-detail", &[])),
                Some(every_comment),
            ))
            .child(self.group("settings-group-forms"))
            .child(self.row(
                t("settings-form-scripts", &[]),
                None,
                Some(t("settings-form-scripts-detail", &[])),
                Some(scripts),
            ))
            .child(self.row(
                t("settings-field-tint", &[]),
                None,
                Some(t("settings-field-tint-detail", &[])),
                Some(tint),
            ))
            .child(self.row(t("settings-date-calendar", &[]), None, None, Some(calendar)))
            .child(self.group("settings-group-files"))
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

    /// Opens the scheme editor on `scheme`, or on a copy of the scheme
    /// in use for a new one.
    fn open_editor(&mut self, scheme: Option<Mine>, window: &mut Window, cx: &mut Context<Self>) {
        let look = cx.global::<Look>().clone();
        let dark = look.dark(window.appearance());
        let scheme = scheme.unwrap_or_else(|| Mine {
            id: Mine::new_id(&look.mine),
            name: String::new(),
            day: look.five(false),
            night: look.five(true),
        });
        let accent = self.theme.accent;
        let mut subs = Vec::new();
        let mut field = |text: String, placeholder: String, cx: &mut Context<Self>| {
            let input = cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input.set_text(text, cx);
                input
            });
            subs.push(
                cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Submit => this.save_editor(cx),
                    InputEvent::Cancel => {
                        this.editor = None;
                        cx.notify();
                    }
                    InputEvent::Changed => cx.notify(),
                }),
            );
            input
        };
        let colours = |five: Five| [five.page, five.card, five.text, five.faint, five.accent];
        let day = colours(scheme.day).map(|c| field(format!("#{c:06x}"), "#rrggbb".into(), cx));
        let night = colours(scheme.night).map(|c| field(format!("#{c:06x}"), "#rrggbb".into(), cx));
        let name = field(scheme.name.clone(), t("settings-scheme-name-hint", &[]), cx);
        window.focus(&gpui::Focusable::focus_handle(&name, cx), cx);
        self.notice = None;
        self.editor = Some(Editor {
            id: scheme.id,
            name,
            night: dark,
            fields: [day, night],
            _subs: subs,
        });
        cx.notify();
    }

    /// The five colours typed for one side, when each is a colour.
    fn typed_five(&self, editor: &Editor, night: bool, cx: &App) -> Option<Five> {
        let c: Vec<u32> = editor.fields[night as usize]
            .iter()
            .filter_map(|f| look::parse_hex(f.read(cx).text()))
            .collect();
        match c.as_slice() {
            &[page, card, text, faint, accent] => Some(Five::new(page, card, text, faint, accent)),
            _ => None,
        }
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let (Some(day), Some(night)) = (
            self.typed_five(editor, false, cx),
            self.typed_five(editor, true, cx),
        ) else {
            return;
        };
        let name = editor.name.read(cx).text().trim().to_owned();
        let name = if name.is_empty() {
            t("settings-scheme-untitled", &[])
        } else {
            name
        };
        let scheme = Mine {
            id: editor.id.clone(),
            name,
            day,
            night,
        };
        self.editor = None;
        Look::set(cx, move |look| {
            look.scheme = scheme.id.clone();
            match look.mine.iter_mut().find(|m| m.id == scheme.id) {
                Some(kept) => *kept = scheme,
                None => look.mine.push(scheme),
            }
        });
        cx.notify();
    }

    /// Asks for a KDE `.colors` file and opens it in the editor.
    fn import_scheme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.import_file(&path, window, cx))
                .ok();
        })
        .detach();
    }

    fn import_file(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let read = std::fs::read_to_string(path);
        let mine = cx.global::<Look>().mine.clone();
        match read.map(|text| look::import_kde(&text, &stem, &mine)) {
            Ok(Ok(scheme)) => self.open_editor(Some(scheme), window, cx),
            _ => {
                self.notice = Some(t("settings-scheme-not-one", &[("file", &name)]));
                cx.notify();
            }
        }
    }

    /// The scheme editor over the page (X10): its name, Day and Night,
    /// five colours a side, and a preview of the side shown.
    fn editor_card(&self, editor: &Editor, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let side = editor.night as usize;
        let sides = [
            (false, "sun", "settings-scheme-day"),
            (true, "moon", "settings-scheme-night"),
        ]
        .map(|(night, glyph, words)| {
            seg_item(
                ("settings-scheme-side", night as usize),
                Some(glyph),
                Some(t(words, &[]).into()),
                editor.night == night,
                &th,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(editor) = this.editor.as_mut() {
                    editor.night = night;
                }
                cx.notify();
            }))
        });
        let rows = editor.fields[side]
            .iter()
            .zip(FIVE_WORDS)
            .map(|(field, words)| {
                let colour = look::parse_hex(field.read(cx).text());
                div()
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .border_t_1()
                    .border_color(th.line_soft)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::BODY))
                            .child(t(words, &[])),
                    )
                    .child(
                        div()
                            .size(px(22.0))
                            .rounded_full()
                            .border_1()
                            .border_color(th.line)
                            .when_some(colour, |d, c| d.bg(gpui::rgb(c))),
                    )
                    .child(
                        div()
                            .w(px(100.0))
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .px(px(space::S3))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(if colour.is_some() { th.line } else { th.error })
                            .bg(th.field)
                            .text_size(px(text::SMALL))
                            .child(field.clone()),
                    )
            });
        let ready = self.typed_five(editor, false, cx).is_some()
            && self.typed_five(editor, true, cx).is_some();
        let preview = self.typed_five(editor, editor.night, cx).map(preview);
        let form = div()
            .flex_1()
            .min_w_0()
            .p(px(space::S5))
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .text_size(px(text::SUBTITLE))
                    .font_weight(FontWeight::BOLD)
                    .child(t("settings-scheme-title", &[])),
            )
            .child(self.boxed("settings-scheme-name", &editor.name))
            .child(seg(&th).children(sides))
            .child(div().flex().flex_col().children(rows))
            .child(
                div()
                    .mt(px(space::S2))
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        button(
                            "settings-scheme-cancel",
                            t("settings-cancel", &[]),
                            false,
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.editor = None;
                            cx.notify();
                        })),
                    )
                    .child(
                        button("settings-scheme-save", t("settings-save", &[]), true, &th)
                            .when(!ready, |b| b.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.save_editor(cx))),
                    ),
            );
        div()
            .id("settings-editor-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.35))
            .occlude()
            .child(
                div()
                    .id("settings-editor")
                    .w(px(600.0))
                    .max_w(px(740.0))
                    .flex()
                    .rounded(px(12.0))
                    .overflow_hidden()
                    .bg(th.card)
                    .shadow(th.lifted())
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(form)
                    .child(
                        div()
                            .w(px(200.0))
                            .flex_none()
                            .p(px(space::S4))
                            .flex()
                            .flex_col()
                            .gap(px(space::S2))
                            .bg(th.page)
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(t("settings-scheme-preview", &[])),
                            )
                            .children(preview),
                    ),
            )
            .into_any_element()
    }

    fn library(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let rows = self.folders.iter().enumerate().map(|(i, (folder, count))| {
            let folder = folder.clone();
            div()
                .h(px(44.0))
                .px(px(space::S4))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .when(i > 0, |d| d.border_t_1().border_color(th.line_soft))
                .child(icon("folder", 16.0, th.dim))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(text::BODY))
                        .child(crate::viewer::short_folder(&folder)),
                )
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(t("settings-folder-files", &[("count", &count.to_string())])),
                )
                .child(
                    float::ib(
                        ("settings-forget-folder", i),
                        "x",
                        t("settings-forget-folder", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Ok(lib) = katna_pdf_library::Library::open_default() {
                            let _ = lib.remove_folder(&folder);
                        }
                        this.folders = watched();
                        crate::library::folders_changed(cx);
                        cx.notify();
                    })),
                )
        });
        let list = (!self.folders.is_empty()).then(|| {
            div()
                .mb(px(space::S3))
                .rounded(px(8.0))
                .border_1()
                .border_color(th.line)
                .children(rows)
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .mb(px(space::S2))
                    .text_size(px(text::CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.dim)
                    .child(t("settings-watched", &[])),
            )
            .children(list)
            .when(self.folders.is_empty(), |d| {
                d.child(
                    div()
                        .mb(px(space::S3))
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(t("settings-no-folders", &[])),
                )
            })
            .child(
                div().flex().child(
                    button(
                        "settings-add-folder",
                        t("settings-add-folder", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(|_, _, window, cx| {
                        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
                            files: false,
                            directories: true,
                            multiple: true,
                            prompt: None,
                        });
                        cx.spawn_in(window, async move |this, cx| {
                            if let Ok(Ok(Some(paths))) = chosen.await {
                                this.update(cx, |this, cx| {
                                    if let Ok(lib) = katna_pdf_library::Library::open_default() {
                                        for path in &paths {
                                            let _ = lib.add_folder(path);
                                        }
                                    }
                                    this.folders = watched();
                                    crate::library::folders_changed(cx);
                                    cx.notify();
                                })
                                .ok();
                            }
                        })
                        .detach();
                    })),
                ),
            )
            .child(self.row(
                t("settings-library-elsewhere", &[]),
                Some(t("settings-library-elsewhere-detail", &[])),
                None,
                None,
            ))
            .into_any_element()
    }

    fn read_aloud(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let prefs = cx.global::<Prefs>().clone();
        let voice = match crate::speech::opened_name() {
            None => t("settings-voice-later", &[]),
            Some("") => t("settings-voice-none", &[]),
            Some(name) => name.to_owned(),
        };
        let speed = self.choice(
            "settings-speed",
            prefs.speed,
            &[
                (0.8, "settings-speed-slow"),
                (1.0, "settings-speed-1"),
                (1.2, "settings-speed-fast"),
                (1.5, "settings-speed-faster"),
            ],
            |p, v| p.speed = v,
            cx,
        );
        let pitch = self.choice(
            "settings-pitch",
            prefs.pitch,
            &[
                (-0.5, "settings-pitch-low"),
                (0.0, "settings-pitch-normal"),
                (0.5, "settings-pitch-high"),
            ],
            |p, v| p.pitch = v,
            cx,
        );
        // The desktop's own voice first, then each the voice offers.
        let own = (String::new(), t("settings-voice-own", &[]));
        let voices: Vec<(String, String)> = std::iter::once(own)
            .chain(self.voices.iter().flatten().cloned())
            .collect();
        let hover = th.hover;
        let list = div()
            .id("settings-voices")
            .mt(px(space::S2))
            .max_h(px(220.0))
            .overflow_y_scroll()
            .rounded(px(radius::MD))
            .border_1()
            .border_color(th.line_soft)
            .py(px(space::S1))
            .children(voices.into_iter().enumerate().map(|(i, (id, words))| {
                let on = prefs.voice == id;
                div()
                    .id(("settings-voice-item", i))
                    .h(px(30.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(radio(on, &th))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(words),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let id = id.clone();
                        Prefs::set(cx, move |p| p.voice = id);
                    }))
            }));
        let try_it = button(
            "settings-try-voice",
            t("settings-try-voice", &[]),
            false,
            &th,
        )
        .on_click(cx.listener(|_, _, _, cx| {
            let words = t("settings-try-voice-words", &[]);
            cx.spawn(async move |this, cx| {
                cx.background_spawn(async move {
                    if let Some(voice) = crate::speech::voice() {
                        let _ = voice.say(&words);
                    }
                })
                .await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }));
        div()
            .flex()
            .flex_col()
            .child(self.row(
                t("settings-voice", &[]),
                Some(voice),
                Some(t("settings-voice-detail", &[])),
                None,
            ))
            .children(
                self.voices
                    .as_ref()
                    .is_some_and(|v| !v.is_empty())
                    .then_some(list),
            )
            .child(self.row(t("settings-speed", &[]), None, None, Some(speed)))
            .child(self.row(t("settings-pitch", &[]), None, None, Some(pitch)))
            .child(div().mt(px(space::S4)).flex().child(try_it))
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
            .chain(
                crate::menus::MORE_KEYS
                    .iter()
                    .map(|(id, keys)| (t(id, &[]), (*keys).to_owned())),
            )
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

/// A small picture of a scheme's side: a card on its page, text lines,
/// and the accent as a button.
/// A size of everything, as a percentage.
fn size_words(size: f32) -> String {
    t(
        "settings-size-percent",
        &[("n", &format!("{:.0}", size * 100.0))],
    )
}

fn preview(five: Five) -> impl IntoElement {
    let c = |rgb: u32| gpui::rgb(rgb);
    let line = |w: f32, colour: u32| div().h(px(3.0)).w(px(w)).rounded(px(1.5)).bg(c(colour));
    div()
        .p(px(space::S3))
        .flex()
        .flex_col()
        .gap(px(space::S3))
        .rounded(px(8.0))
        .bg(c(five.page))
        .child(
            div()
                .p(px(space::S3))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .rounded(px(6.0))
                .bg(c(five.card))
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(c(five.text))
                        .child(t("settings-scheme-preview-file", &[])),
                )
                .child(line(120.0, five.faint))
                .child(line(90.0, five.faint)),
        )
        .child(
            div()
                .h(px(90.0))
                .p(px(space::S3))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .rounded(px(6.0))
                .bg(c(five.card))
                .child(line(80.0, five.text))
                .child(line(130.0, five.faint))
                .child(line(110.0, five.faint)),
        )
        .child(
            div().flex().justify_center().child(
                div()
                    .px(px(space::S3))
                    .py(px(3.0))
                    .rounded(px(6.0))
                    .bg(c(five.accent))
                    .text_size(px(text::CAPTION))
                    .text_color(c(if (five.accent >> 8 & 0xff) > 0x9a {
                        0x000000
                    } else {
                        0xffffff
                    }))
                    .child(t("settings-scheme-preview-button", &[])),
            ),
        )
}

/// The library's watched folders, each with how many files it holds.
fn watched() -> Vec<(std::path::PathBuf, usize)> {
    let Ok(lib) = katna_pdf_library::Library::open_default() else {
        return Vec::new();
    };
    let folders = lib.folders().unwrap_or_default();
    let docs = lib
        .docs(&katna_pdf_library::Filter::default())
        .unwrap_or_default();
    folders
        .into_iter()
        .map(|folder| {
            let count = docs.iter().filter(|d| d.path.starts_with(&folder)).count();
            (folder, count)
        })
        .collect()
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
            Page::Library => ("settings-library", self.library(cx)),
            Page::ReadAloud => ("settings-read-aloud", self.read_aloud(cx)),
            Page::Shortcuts => ("settings-shortcuts", self.shortcuts(cx)),
            Page::About => ("settings-about", self.about(cx)),
        };
        let editor = self
            .editor
            .as_ref()
            .map(|editor| self.editor_card(editor, cx));
        div()
            .id("settings")
            .size_full()
            .flex()
            .bg(th.card)
            .text_color(th.text)
            .font_family(".SystemUIFont")
            .relative()
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
            .children(editor)
    }
}
