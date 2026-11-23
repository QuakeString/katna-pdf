// SPDX-License-Identifier: GPL-3.0-or-later

//! What Settings' Viewing and Editing pages choose (X02, X03), kept in
//! the state folder (`pdf/prefs.txt`) as `key=value` lines. Anything
//! missing or not understood keeps its default.

use std::path::PathBuf;

use gpui::Global;

use crate::spread::Spread;

/// The zoom a file opens at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartZoom {
    #[default]
    FitWidth,
    FitPage,
    Actual,
}

/// The unit lengths are shown and typed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Units {
    #[default]
    Points,
    Millimetres,
    Inches,
}

impl Units {
    /// Points in one of these.
    pub fn points(self) -> f32 {
        match self {
            Units::Points => 1.0,
            Units::Millimetres => 72.0 / 25.4,
            Units::Inches => 72.0,
        }
    }

    /// How it is written after a number.
    pub fn suffix(self) -> &'static str {
        match self {
            Units::Points => "pt",
            Units::Millimetres => "mm",
            Units::Inches => "in",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Open a file where it was left (else at its first page).
    pub open_at_last: bool,
    pub zoom: StartZoom,
    pub layout: Spread,
    /// Open the last session's files when the app starts with none.
    pub reopen_session: bool,
    /// Draw pages in worker processes (from the next start).
    pub workers: bool,
    pub units: Units,
    /// Snap moved objects to the grid from the start.
    pub grid: bool,
    /// How fast reading aloud goes, 1.0 the voice's own pace.
    pub speed: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            open_at_last: true,
            zoom: StartZoom::default(),
            layout: Spread::default(),
            reopen_session: false,
            workers: true,
            units: Units::default(),
            grid: false,
            speed: 1.0,
        }
    }
}

impl Global for Prefs {}

fn file() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("prefs.txt"))
}

impl Prefs {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut prefs = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let on = value == "on";
            match key.trim() {
                "open-at" => prefs.open_at_last = value != "first",
                "zoom" => {
                    prefs.zoom = match value {
                        "page" => StartZoom::FitPage,
                        "actual" => StartZoom::Actual,
                        _ => StartZoom::FitWidth,
                    }
                }
                "layout" => {
                    prefs.layout = match value {
                        "two" => Spread::Facing,
                        "book" => Spread::Book,
                        _ => Spread::Single,
                    }
                }
                "reopen-session" => prefs.reopen_session = on,
                "workers" => prefs.workers = value != "off",
                "units" => {
                    prefs.units = match value {
                        "mm" => Units::Millimetres,
                        "in" => Units::Inches,
                        _ => Units::Points,
                    }
                }
                "grid" => prefs.grid = on,
                "read-speed" => {
                    if let Ok(speed) = value.parse::<f32>()
                        && (0.5..=2.0).contains(&speed)
                    {
                        prefs.speed = speed;
                    }
                }
                _ => {}
            }
        }
        prefs
    }

    fn text(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        let zoom = match self.zoom {
            StartZoom::FitWidth => "width",
            StartZoom::FitPage => "page",
            StartZoom::Actual => "actual",
        };
        let layout = match self.layout {
            Spread::Single => "one",
            Spread::Facing => "two",
            Spread::Book => "book",
        };
        format!(
            "open-at={}\nzoom={zoom}\nlayout={layout}\nreopen-session={}\nworkers={}\nunits={}\ngrid={}\nread-speed={}\n",
            if self.open_at_last { "last" } else { "first" },
            on(self.reopen_session),
            on(self.workers),
            self.units.suffix(),
            on(self.grid),
            self.speed,
        )
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Err(err) = std::fs::write(&path, self.text()) {
            eprintln!("katna-pdf: saving settings: {err}");
        }
    }

    /// Changes the settings for every window, and keeps them.
    pub fn set(cx: &mut gpui::App, change: impl FnOnce(&mut Prefs)) {
        let mut prefs = cx.global::<Prefs>().clone();
        change(&mut prefs);
        prefs.save();
        crate::speech::set_speed(prefs.speed);
        cx.set_global(prefs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_read_back_as_written() {
        let prefs = Prefs {
            open_at_last: false,
            zoom: StartZoom::FitPage,
            layout: Spread::Book,
            reopen_session: true,
            workers: false,
            units: Units::Millimetres,
            grid: true,
            speed: 1.25,
        };
        assert_eq!(Prefs::parse(&prefs.text()), prefs);
        assert_eq!(Prefs::parse("nonsense\nzoom=huge"), Prefs::default());
    }
}
