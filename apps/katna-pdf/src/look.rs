// SPDX-License-Identifier: GPL-3.0-or-later

//! How the window looks, as people choose it in the profile menu and
//! Settings: day, night or as the desktop is (`Mode`), in a colour
//! scheme. The schemes are Float's own, Katna Mail's
//! (`katna_ui::schemes`) and the desktop's (`System`). Kept in the state
//! folder (`pdf/look.txt`), shared by every window. With `follow_mail`
//! the mode, scheme and accent are Katna Mail's (its `config.toml`), and
//! picking them here changes Katna Mail's too.

use std::path::PathBuf;

use gpui::{App, Global, WindowAppearance};
use katna_platform::colors::{self, DesktopKind, SystemColors};
use katna_ui::schemes::{self, BUILT_IN};
use katna_ui::theme::Accent;

use crate::theme::Five;

/// Float's own scheme, the default.
pub const FLOAT: &str = "float";
/// The desktop's colours.
pub const SYSTEM: &str = schemes::SYSTEM;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Light or dark as the desktop is.
    #[default]
    System,
    Day,
    Night,
}

impl Mode {
    fn parse(s: &str) -> Self {
        match s {
            "day" => Self::Day,
            "night" => Self::Night,
            _ => Self::System,
        }
    }

    fn setting(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Day => "day",
            Self::Night => "night",
        }
    }
}

/// A scheme people can pick: its id, the id of its name in the `.ftl`,
/// and its day and night colours.
#[derive(Debug, Clone, Copy)]
pub struct Scheme {
    pub id: &'static str,
    pub name: &'static str,
    pub day: Five,
    pub night: Five,
}

pub fn float_five(dark: bool) -> Five {
    if dark {
        Five::new(0x121418, 0x1b1e24, 0xe8eaee, 0x9aa1ad, 0x5b8cff)
    } else {
        Five::new(0xeceef2, 0xffffff, 0x16181d, 0x5d6470, 0x2f6bff)
    }
}

fn five_of(side: &schemes::Side) -> Five {
    Five::new(
        side.page >> 8,
        side.card >> 8,
        side.text >> 8,
        side.faint >> 8,
        side.accent >> 8,
    )
}

/// Float's own first, then Katna Mail's, in Settings' order.
pub fn schemes() -> Vec<Scheme> {
    let mut list = vec![Scheme {
        id: FLOAT,
        name: "scheme-float",
        day: float_five(false),
        night: float_five(true),
    }];
    list.extend(BUILT_IN.iter().map(|b| Scheme {
        id: b.id,
        name: b.name,
        day: five_of(&b.light),
        night: five_of(&b.dark),
    }));
    list
}

/// A scheme the person made or brought in (X10, X11), kept in the
/// state folder (`pdf/schemes.txt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mine {
    /// `mine-N`, never one of the built-in ids.
    pub id: String,
    pub name: String,
    pub day: Five,
    pub night: Five,
}

impl Mine {
    /// A new id, not used by any of `mine`.
    pub fn new_id(mine: &[Mine]) -> String {
        (1..)
            .map(|n| format!("mine-{n}"))
            .find(|id| mine.iter().all(|m| &m.id != id))
            .unwrap_or_default()
    }
}

/// A colour written as `rrggbb`, with or without `#`.
pub fn parse_hex(text: &str) -> Option<u32> {
    let text = text.trim().trim_start_matches('#');
    (text.len() == 6 && text.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

fn five_text(five: &Five) -> String {
    [five.page, five.card, five.text, five.faint, five.accent]
        .map(|c| format!("{c:06x}"))
        .join(" ")
}

fn five_parse(text: &str) -> Option<Five> {
    let c: Vec<u32> = text.split_whitespace().filter_map(parse_hex).collect();
    match c.as_slice() {
        &[page, card, text, faint, accent] => Some(Five::new(page, card, text, faint, accent)),
        _ => None,
    }
}

fn mine_file() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("schemes.txt"))
}

fn parse_mine(text: &str) -> Vec<Mine> {
    let mut list = Vec::new();
    for block in text.split("\n\n") {
        let (mut id, mut name, mut day, mut night) = (None, None, None, None);
        for line in block.lines() {
            match line.split_once('=') {
                Some(("id", v)) => id = Some(v.trim().to_owned()),
                Some(("name", v)) => name = Some(v.trim().to_owned()),
                Some(("day", v)) => day = five_parse(v),
                Some(("night", v)) => night = five_parse(v),
                _ => {}
            }
        }
        if let (Some(id), Some(name), Some(day), Some(night)) = (id, name, day, night)
            && id.starts_with("mine-")
        {
            list.push(Mine {
                id,
                name,
                day,
                night,
            });
        }
    }
    list
}

fn mine_text(mine: &[Mine]) -> String {
    mine.iter()
        .map(|m| {
            format!(
                "id={}\nname={}\nday={}\nnight={}\n",
                m.id,
                m.name.replace('\n', " "),
                five_text(&m.day),
                five_text(&m.night)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Why a file can't be brought in as a scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// Not a KDE colour scheme: it has no `[Colors:Window]` group.
    NotAScheme,
}

/// A KDE `.colors` file as one of the person's schemes: its own side
/// as it is, the other side Float's with the file's accent, to change
/// after. Its name is the file's `Name=`, else `fallback`.
pub fn import_kde(contents: &str, fallback: &str, mine: &[Mine]) -> Result<Mine, ImportError> {
    if !contents.contains("[Colors:Window]") {
        return Err(ImportError::NotAScheme);
    }
    let (scheme, _) = colors::parse_kdeglobals(contents);
    let o = |c: u32| c >> 8;
    let side = Five::new(
        o(scheme.window_bg),
        o(scheme.view_bg),
        o(scheme.view_fg),
        o(scheme.inactive_fg),
        o(scheme.accent),
    );
    let dark = scheme.dark();
    let mut other = float_five(!dark);
    other.accent = side.accent;
    let mut general = false;
    let mut name = None;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            general = line == "[General]";
        } else if general && let Some(n) = line.strip_prefix("Name=") {
            name = Some(n.trim().to_owned()).filter(|n| !n.is_empty());
        }
    }
    let (day, night) = if dark { (other, side) } else { (side, other) };
    Ok(Mine {
        id: Mine::new_id(mine),
        name: name.unwrap_or_else(|| fallback.to_owned()),
        day,
        night,
    })
}

/// The look chosen, and the desktop's colours for `System`.
#[derive(Debug, Clone)]
pub struct Look {
    pub mode: Mode,
    /// A scheme's id: [`FLOAT`], one of [`schemes`], one of `mine` or
    /// [`SYSTEM`].
    pub scheme: String,
    pub desktop: SystemColors,
    /// The person's own schemes.
    pub mine: Vec<Mine>,
    /// The accent over the scheme's own (X01).
    pub accent: Accent,
    /// Use Katna Mail's mode, scheme and accent, and change both (X09).
    pub follow_mail: bool,
}

impl Global for Look {}

fn file() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("look.txt"))
}

impl Default for Look {
    fn default() -> Self {
        Self {
            mode: Mode::System,
            scheme: FLOAT.into(),
            desktop: SystemColors::default(),
            mine: Vec::new(),
            accent: Accent::Scheme,
            follow_mail: false,
        }
    }
}

/// The accents offered beside the scheme's own: Katna Mail's.
pub const ACCENTS: [u32; 7] = [
    0x0b57d0, 0x00807f, 0x2e9e4f, 0xe2a400, 0xe8590c, 0xd6336c, 0x7048e8,
];

/// Katna Mail's settings file, when Katna Mail has written one.
pub fn mail_config_file() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.config_file())
        .filter(|path| path.exists())
}

impl Look {
    /// The look last chosen, with the desktop's colours read now.
    pub fn load() -> Self {
        let mut look = file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default();
        look.desktop = desktop_colors();
        look.mine = mine_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| parse_mine(&text))
            .unwrap_or_default();
        if look.follow_mail {
            look.take_mail();
        }
        look
    }

    /// Takes Katna Mail's mode, scheme and accent; a scheme of its own
    /// that Katna PDF doesn't have leaves this one as it is.
    pub fn take_mail(&mut self) {
        let Some(config) = mail_config_file().and_then(|f| katna_core::Config::load(&f).ok())
        else {
            return;
        };
        self.take_mail_view(&config.mail);
    }

    fn take_mail_view(&mut self, view: &katna_core::config::MailView) {
        use katna_core::config::Theme;
        self.mode = match view.theme {
            Theme::System => Mode::System,
            Theme::Light => Mode::Day,
            Theme::Dark => Mode::Night,
        };
        let id = match view.colors() {
            "katna" => FLOAT,
            id => id,
        };
        if id == SYSTEM || schemes().iter().any(|s| s.id == id) {
            self.scheme = id.to_owned();
        }
        self.accent = Accent::parse(&view.accent);
    }

    /// Puts this look into Katna Mail's settings; one of the person's
    /// own schemes stays Katna Mail's scheme as it was.
    fn give_mail(&self, view: &mut katna_core::config::MailView) {
        use katna_core::config::Theme;
        view.theme = match self.mode {
            Mode::System => Theme::System,
            Mode::Day => Theme::Light,
            Mode::Night => Theme::Dark,
        };
        if self.scheme == FLOAT {
            view.set_colors("katna");
        } else if self.scheme == SYSTEM || schemes().iter().any(|s| s.id == self.scheme) {
            view.set_colors(&self.scheme);
        }
        view.accent = self.accent.setting();
    }

    /// Katna Mail's look again, if it changed while this window was away.
    pub fn follow(cx: &mut App) {
        let look = cx.global::<Look>();
        if !look.follow_mail {
            return;
        }
        let mut again = look.clone();
        again.take_mail();
        let look = cx.global::<Look>();
        if (again.mode, &again.scheme, again.accent) != (look.mode, &look.scheme, look.accent) {
            again.save();
            cx.set_global(again);
        }
    }

    fn parse(text: &str) -> Self {
        let mut look = Self::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("mode", v)) => look.mode = Mode::parse(v.trim()),
                Some(("scheme", v)) => look.scheme = v.trim().to_owned(),
                Some(("accent", v)) => look.accent = Accent::parse(v.trim()),
                Some(("follow-mail", v)) => look.follow_mail = v.trim() == "on",
                _ => {}
            }
        }
        look
    }

    fn text(&self) -> String {
        format!(
            "mode={}\nscheme={}\naccent={}\nfollow-mail={}\n",
            self.mode.setting(),
            self.scheme,
            self.accent.setting(),
            if self.follow_mail { "on" } else { "off" },
        )
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Err(err) = std::fs::write(&path, self.text()) {
            eprintln!("katna-pdf: saving the look: {err}");
        }
        if let Some(path) = mine_file()
            && (path.exists() || !self.mine.is_empty())
            && let Err(err) = std::fs::write(&path, mine_text(&self.mine))
        {
            eprintln!("katna-pdf: saving colour schemes: {err}");
        }
    }

    /// Whether the window is dark: as the mode says, or the desktop.
    pub fn dark(&self, appearance: WindowAppearance) -> bool {
        match self.mode {
            Mode::Day => false,
            Mode::Night => true,
            Mode::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        }
    }

    /// The id the colours are mixed for, Float's when the scheme is gone.
    pub fn scheme_id(&self) -> &str {
        if self.scheme == SYSTEM
            || schemes().iter().any(|s| s.id == self.scheme)
            || self.mine.iter().any(|m| m.id == self.scheme)
        {
            &self.scheme
        } else {
            FLOAT
        }
    }

    /// The five colours for `dark`, with the accent picked.
    pub fn five(&self, dark: bool) -> Five {
        let mut five = self.five_for(&self.scheme, dark);
        match self.accent {
            Accent::Scheme => {}
            Accent::System => {
                if let Some(accent) = self.desktop.accent {
                    five.accent = accent >> 8;
                }
            }
            Accent::Color(color) => five.accent = color >> 8,
        }
        five
    }

    /// The five colours of scheme `scheme` for `dark`.
    pub fn five_for(&self, scheme: &str, dark: bool) -> Five {
        if scheme == SYSTEM {
            if let Some(s) = self.desktop.scheme_for(dark) {
                return Five::new(
                    s.window_bg >> 8,
                    s.view_bg >> 8,
                    s.view_fg >> 8,
                    s.inactive_fg >> 8,
                    s.accent >> 8,
                );
            }
            let mut five = float_five(dark);
            if let Some(accent) = self.desktop.accent {
                five.accent = accent >> 8;
            }
            return five;
        }
        if let Some(m) = self.mine.iter().find(|m| m.id == scheme) {
            return if dark { m.night } else { m.day };
        }
        schemes()
            .into_iter()
            .find(|s| s.id == scheme)
            .map_or_else(|| float_five(dark), |s| if dark { s.night } else { s.day })
    }

    /// Changes the look for every window, and keeps it.
    pub fn set(cx: &mut App, change: impl FnOnce(&mut Look)) {
        let mut look = cx.global::<Look>().clone();
        change(&mut look);
        look.save();
        if look.follow_mail
            && let Some(path) = mail_config_file()
        {
            match katna_core::Config::load(&path) {
                Ok(mut config) => {
                    let before = config.mail.clone();
                    look.give_mail(&mut config.mail);
                    if config.mail != before
                        && let Err(err) = config.save(&path)
                    {
                        eprintln!("katna-pdf: saving Katna Mail's colours: {err}");
                    }
                }
                Err(err) => eprintln!("katna-pdf: reading Katna Mail's settings: {err}"),
            }
        }
        cx.set_global(look);
    }
}

/// The desktop's colours: KDE's scheme, GNOME's accent, Windows'.
fn desktop_colors() -> SystemColors {
    let kind = if cfg!(windows) {
        DesktopKind::Windows
    } else {
        let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let has = |name: &str| desktops.split(':').any(|d| d.eq_ignore_ascii_case(name));
        if has("KDE") {
            DesktopKind::Kde
        } else if has("GNOME") {
            DesktopKind::Gnome
        } else {
            DesktopKind::Other
        }
    };
    match colors::config_home() {
        _ if kind == DesktopKind::Windows => colors::read(kind, std::path::Path::new(""), None),
        Some(home) => colors::read(kind, &home, None),
        None => SystemColors::accent_only(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes() {
        let look = Look {
            mode: Mode::Night,
            scheme: "nord".into(),
            ..Look::default()
        };
        let again = Look::parse(&look.text());
        assert_eq!(again.mode, Mode::Night);
        assert_eq!(again.scheme, "nord");
        assert_eq!(again.accent, Accent::Scheme);
        let look = Look {
            accent: Accent::Color(0x2e9e4fff),
            follow_mail: true,
            ..Look::default()
        };
        let again = Look::parse(&look.text());
        assert_eq!(again.accent, look.accent);
        assert!(again.follow_mail);
        assert_eq!(again.five(false).accent, 0x2e9e4f);
    }

    #[test]
    fn katna_mails_look_goes_both_ways() {
        let mut view = katna_core::Config::default().mail;
        let look = Look {
            mode: Mode::Night,
            scheme: FLOAT.into(),
            accent: Accent::Color(0xd6336cff),
            ..Look::default()
        };
        look.give_mail(&mut view);
        assert_eq!(view.colors(), "katna");
        assert_eq!(view.accent, "#d6336c");
        let mut back = Look {
            scheme: "nord".into(),
            ..Look::default()
        };
        back.take_mail_view(&view);
        assert_eq!(back.mode, Mode::Night);
        assert_eq!(back.scheme, FLOAT);
        assert_eq!(back.accent, look.accent);
        view.set_colors("someones-own");
        back.take_mail_view(&view);
        assert_eq!(back.scheme, FLOAT, "a scheme only Katna Mail has is left");
    }

    #[test]
    fn own_schemes_read_back_and_are_used() {
        let mine = vec![Mine {
            id: "mine-1".into(),
            name: "Office green".into(),
            day: Five::new(0xeceef2, 0xffffff, 0x14213d, 0x5c6b84, 0x0b6e4f),
            night: float_five(true),
        }];
        assert_eq!(parse_mine(&mine_text(&mine)), mine);
        let look = Look {
            scheme: "mine-1".into(),
            mine: mine.clone(),
            ..Look::default()
        };
        assert_eq!(look.scheme_id(), "mine-1");
        assert_eq!(look.five(false).accent, 0x0b6e4f);
        assert_eq!(Mine::new_id(&mine), "mine-2");
        assert_eq!(parse_hex("#0B6E4F"), Some(0x0b6e4f));
        assert_eq!(parse_hex("0b6e4"), None);
    }

    #[test]
    fn kde_colour_files_come_in_with_a_made_day_side() {
        let dark = "[General]\nName=Materia Dark\n\n[Colors:Window]\nBackgroundNormal=30,30,30\n\
            [Colors:View]\nBackgroundNormal=40,40,40\nForegroundNormal=220,220,220\n\
            [Colors:Selection]\nBackgroundNormal=#3daee9\n";
        let m = import_kde(dark, "MateriaDark", &[]).unwrap();
        assert_eq!(m.name, "Materia Dark");
        assert_eq!(m.night.page, 0x1e1e1e);
        assert_eq!(m.night.accent, 0x3daee9);
        assert_eq!(m.day.page, float_five(false).page);
        assert_eq!(m.day.accent, 0x3daee9);
        assert_eq!(
            import_kde("just notes", "notes", &[]),
            Err(ImportError::NotAScheme)
        );
    }

    #[test]
    fn lists_float_and_katna_mails_schemes() {
        let list = schemes();
        assert_eq!(list[0].id, FLOAT);
        assert!(list.iter().any(|s| s.id == "nord"));
        assert_eq!(list.len(), BUILT_IN.len() + 1);
    }

    #[test]
    fn a_gone_scheme_falls_back_to_float() {
        let look = Look {
            scheme: "gone".into(),
            ..Look::default()
        };
        assert_eq!(look.scheme_id(), FLOAT);
        assert_eq!(look.five(false), float_five(false));
    }
}
