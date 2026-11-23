// SPDX-License-Identifier: GPL-3.0-or-later

//! What Settings' Viewing and Editing pages choose (X02, X03), kept in
//! the state folder (`pdf/prefs.txt`) as `key=value` lines. Anything
//! missing or not understood keeps its default.

use std::path::PathBuf;

use gpui::Global;

use katna_pdf_render::recolor::PageColors;

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

/// Whether the app's motion (cards coming on, panes sliding) runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// As the desktop says (KDE's animation speed, GNOME's animations).
    #[default]
    Desktop,
    /// None: everything comes on at once.
    Less,
    /// Always, whatever the desktop says.
    Normal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Open a file where it was left (else at its first page).
    pub open_at_last: bool,
    pub zoom: StartZoom,
    pub layout: Spread,
    /// Open the last session's files when the app starts with none.
    pub reopen_session: bool,
    /// Files opened from outside (a file manager, a second launch) get
    /// a window of their own rather than a tab.
    pub new_window: bool,
    /// Draw pages in worker processes (from the next start).
    pub workers: bool,
    pub units: Units,
    /// Snap moved objects to the grid from the start.
    pub grid: bool,
    /// How fast reading aloud goes, 1.0 the voice's own pace.
    pub speed: f32,
    /// The pages' colours when the window is dark (X09).
    pub night_pages: PageColors,
    /// The size of everything, over the desktop's scale.
    pub size: f32,
    /// Show the name of the mode chosen in the dock.
    pub dock_labels: bool,
    /// Run forms' own scripts (checks, totals, buttons).
    pub form_scripts: bool,
    /// Keep a copy of changes not saved, to recover after a crash.
    pub recovery: bool,
    /// How often that copy is kept, in minutes.
    pub recovery_minutes: u32,
    /// How many files the start screen lists as opened lately.
    pub recent: usize,
    /// Show a comment's words in a card while the pointer is over it.
    pub comment_hover: bool,
    /// Open every new comment for typing, not only notes and text boxes.
    pub comment_every: bool,
    /// Tint form fields so they stand out from the page.
    pub field_tint: bool,
    /// Show a calendar under date fields being filled in.
    pub date_calendar: bool,
    /// Seconds each page shows in a presentation when the file gives
    /// none; 0 waits for a key.
    pub present_seconds: u32,
    /// Start again at the first page after the last.
    pub present_loop: bool,
    /// What find matches from the start: capitals, whole words, accents.
    pub find_case: bool,
    pub find_words: bool,
    pub find_accents: bool,
    pub motion: Motion,
}

/// The intervals offered for keeping changes, in minutes.
pub const RECOVERY_MINUTES: [u32; 3] = [1, 5, 10];
/// How many recent files the start screen can list.
pub const RECENT: [usize; 3] = [4, 8, 16];
/// The seconds a presentation's pages can show for.
pub const PRESENT_SECONDS: [u32; 4] = [0, 5, 10, 30];

/// The sizes offered.
pub const SIZES: [f32; 4] = [0.9, 1.0, 1.1, 1.25];

/// The setting for page colours, and back.
const PAGE_COLORS: [(PageColors, &str); 6] = [
    (PageColors::Normal, "normal"),
    (PageColors::Night, "night"),
    (PageColors::Invert, "invert"),
    (PageColors::InvertLightness, "invert-lightness"),
    (PageColors::Sepia, "sepia"),
    (PageColors::BlackWhite, "black-white"),
];

impl Default for Prefs {
    fn default() -> Self {
        Self {
            open_at_last: true,
            zoom: StartZoom::default(),
            layout: Spread::default(),
            reopen_session: false,
            new_window: false,
            workers: true,
            units: Units::default(),
            grid: false,
            speed: 1.0,
            night_pages: PageColors::Night,
            size: 1.0,
            dock_labels: true,
            form_scripts: true,
            recovery: true,
            recovery_minutes: 1,
            recent: 8,
            comment_hover: true,
            comment_every: false,
            field_tint: true,
            date_calendar: true,
            present_seconds: 0,
            present_loop: false,
            find_case: false,
            find_words: false,
            find_accents: false,
            motion: Motion::Desktop,
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
                "new-window" => prefs.new_window = on,
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
                "night-pages" => {
                    if let Some(&(colors, _)) = PAGE_COLORS.iter().find(|(_, w)| *w == value) {
                        prefs.night_pages = colors;
                    }
                }
                "size" => {
                    if let Ok(size) = value.parse::<f32>()
                        && (0.5..=2.0).contains(&size)
                    {
                        prefs.size = size;
                    }
                }
                "dock-labels" => prefs.dock_labels = value != "off",
                "form-scripts" => prefs.form_scripts = value != "off",
                "recovery" => prefs.recovery = value != "off",
                "recovery-every" => {
                    if let Ok(m) = value.parse::<u32>()
                        && RECOVERY_MINUTES.contains(&m)
                    {
                        prefs.recovery_minutes = m;
                    }
                }
                "recent" => {
                    if let Ok(n) = value.parse::<usize>()
                        && RECENT.contains(&n)
                    {
                        prefs.recent = n;
                    }
                }
                "comment-hover" => prefs.comment_hover = value != "off",
                "comment-every" => prefs.comment_every = on,
                "field-tint" => prefs.field_tint = value != "off",
                "date-calendar" => prefs.date_calendar = value != "off",
                "present-seconds" => {
                    if let Ok(n) = value.parse::<u32>()
                        && PRESENT_SECONDS.contains(&n)
                    {
                        prefs.present_seconds = n;
                    }
                }
                "present-loop" => prefs.present_loop = on,
                "find-case" => prefs.find_case = on,
                "find-words" => prefs.find_words = on,
                "find-accents" => prefs.find_accents = on,
                "motion" => {
                    prefs.motion = match value {
                        "less" => Motion::Less,
                        "normal" => Motion::Normal,
                        _ => Motion::Desktop,
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
        let night = PAGE_COLORS
            .iter()
            .find(|(c, _)| *c == self.night_pages)
            .map_or("night", |(_, w)| w);
        let motion = match self.motion {
            Motion::Desktop => "desktop",
            Motion::Less => "less",
            Motion::Normal => "normal",
        };
        format!(
            "open-at={}\nzoom={zoom}\nlayout={layout}\nreopen-session={}\nnew-window={}\nworkers={}\nunits={}\ngrid={}\nread-speed={}\nnight-pages={night}\nsize={}\ndock-labels={}\nform-scripts={}\nrecovery={}\nrecovery-every={}\nrecent={}\ncomment-hover={}\ncomment-every={}\nfield-tint={}\ndate-calendar={}\npresent-seconds={}\npresent-loop={}\nfind-case={}\nfind-words={}\nfind-accents={}\nmotion={motion}\n",
            if self.open_at_last { "last" } else { "first" },
            on(self.reopen_session),
            on(self.new_window),
            on(self.workers),
            self.units.suffix(),
            on(self.grid),
            self.speed,
            self.size,
            on(self.dock_labels),
            on(self.form_scripts),
            on(self.recovery),
            self.recovery_minutes,
            self.recent,
            on(self.comment_hover),
            on(self.comment_every),
            on(self.field_tint),
            on(self.date_calendar),
            self.present_seconds,
            on(self.present_loop),
            on(self.find_case),
            on(self.find_words),
            on(self.find_accents),
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
        if prefs.motion != cx.global::<Prefs>().motion {
            apply_motion(prefs.motion, cx);
        }
        let resized = prefs.size != cx.global::<Prefs>().size;
        katna_ui::scale::set_scale(prefs.size);
        cx.set_global(prefs);
        if resized {
            cx.refresh_windows();
        }
    }
}

/// Runs the app's motion or not, as `motion` and the desktop say.
pub fn apply_motion(motion: Motion, cx: &mut gpui::App) {
    cx.set_reduce_motion(match motion {
        Motion::Desktop => desktop_motion().off(),
        Motion::Less => true,
        Motion::Normal => false,
    });
}

/// What the desktop says about animations, read once a run (GNOME's
/// answer takes running `gsettings`).
pub fn desktop_motion() -> katna_platform::motion::DesktopMotion {
    static READ: std::sync::OnceLock<katna_platform::motion::DesktopMotion> =
        std::sync::OnceLock::new();
    *READ.get_or_init(|| {
        let kde = crate::look::desktop_kind() == katna_platform::colors::DesktopKind::Kde;
        katna_platform::motion::read(kde, katna_platform::colors::config_home().as_deref())
    })
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
            new_window: true,
            workers: false,
            units: Units::Millimetres,
            grid: true,
            speed: 1.25,
            night_pages: PageColors::Sepia,
            size: 1.1,
            dock_labels: false,
            form_scripts: false,
            recovery: false,
            recovery_minutes: 5,
            recent: 16,
            comment_hover: false,
            comment_every: true,
            field_tint: false,
            date_calendar: false,
            present_seconds: 10,
            present_loop: true,
            find_case: true,
            find_words: true,
            find_accents: true,
            motion: Motion::Less,
        };
        assert_eq!(Prefs::parse(&prefs.text()), prefs);
        assert_eq!(Prefs::parse("nonsense\nzoom=huge"), Prefs::default());
    }
}
