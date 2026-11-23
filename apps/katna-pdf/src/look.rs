// SPDX-License-Identifier: GPL-3.0-or-later

//! How the window looks, as people choose it in the profile menu and
//! Settings: day, night or as the desktop is (`Mode`), in a colour
//! scheme. The schemes are Float's own, Katna Mail's
//! (`katna_ui::schemes`) and the desktop's (`System`). Kept in the state
//! folder (`pdf/look.txt`), shared by every window.

use std::path::PathBuf;

use gpui::{App, Global, WindowAppearance};
use katna_platform::colors::{self, DesktopKind, SystemColors};
use katna_ui::schemes::{self, BUILT_IN};

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

/// The look chosen, and the desktop's colours for `System`.
#[derive(Debug, Clone)]
pub struct Look {
    pub mode: Mode,
    /// A scheme's id: [`FLOAT`], one of [`schemes`] or [`SYSTEM`].
    pub scheme: String,
    pub desktop: SystemColors,
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
        }
    }
}

impl Look {
    /// The look last chosen, with the desktop's colours read now.
    pub fn load() -> Self {
        let mut look = file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default();
        look.desktop = desktop_colors();
        look
    }

    fn parse(text: &str) -> Self {
        let mut look = Self::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("mode", v)) => look.mode = Mode::parse(v.trim()),
                Some(("scheme", v)) => look.scheme = v.trim().to_owned(),
                _ => {}
            }
        }
        look
    }

    fn text(&self) -> String {
        format!("mode={}\nscheme={}\n", self.mode.setting(), self.scheme)
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Err(err) = std::fs::write(&path, self.text()) {
            eprintln!("katna-pdf: saving the look: {err}");
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
        if self.scheme == SYSTEM || schemes().iter().any(|s| s.id == self.scheme) {
            &self.scheme
        } else {
            FLOAT
        }
    }

    /// The five colours for `dark`.
    pub fn five(&self, dark: bool) -> Five {
        self.five_for(&self.scheme, dark)
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
