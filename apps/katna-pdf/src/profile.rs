// SPDX-License-Identifier: GPL-3.0-or-later

//! The person using the app (X00): their name, a short name, job title
//! and organisation, kept in the state folder (`pdf/profile.txt`) on
//! this computer only. Comments carry the short name (or the name), and
//! the title bar's picture shows the name's initials.

use std::path::PathBuf;

use gpui::Global;

/// Who the person is, as they typed it in Settings > Profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub short: String,
    pub title: String,
    pub organisation: String,
}

fn file() -> Option<PathBuf> {
    katna_core::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("profile.txt"))
}

impl Global for Profile {}

impl Profile {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut profile = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_owned();
            match key.trim() {
                "name" => profile.name = value,
                "short" => profile.short = value,
                "title" => profile.title = value,
                "organisation" => profile.organisation = value,
                _ => {}
            }
        }
        profile
    }

    fn text(&self) -> String {
        // One line each, so a line break typed in is kept out.
        let one = |s: &str| s.replace(['\n', '\r'], " ");
        format!(
            "name={}\nshort={}\ntitle={}\norganisation={}\n",
            one(&self.name),
            one(&self.short),
            one(&self.title),
            one(&self.organisation)
        )
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        if let Err(err) = std::fs::write(&path, self.text()) {
            eprintln!("katna-pdf: saving the profile: {err}");
        }
    }

    /// The person's name: as typed, else the computer's account's.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            crate::chrome::person()
        } else {
            self.name.trim().to_owned()
        }
    }

    /// The name comments carry: the short name, else the name, else
    /// none (the account's login then).
    pub fn author(&self) -> Option<String> {
        [&self.short, &self.name]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_reads_back_as_written() {
        let p = Profile {
            name: "Mozammel Hossain".into(),
            short: "M. Hossain".into(),
            title: "Purchase officer".into(),
            organisation: "District office,\nKolkata".into(),
        };
        let back = Profile::parse(&p.text());
        assert_eq!(back.name, p.name);
        assert_eq!(back.short, p.short);
        assert_eq!(back.organisation, "District office, Kolkata");
        assert_eq!(back.author().as_deref(), Some("M. Hossain"));
        assert_eq!(Profile::default().author(), None);
    }
}
