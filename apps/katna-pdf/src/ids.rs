// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's identifiers on the desktop, derived from Katna's prefix in
//! `katna_core::ids` so it is written in one place for the whole family.

/// The app ID: the desktop file's name and the Wayland app ID.
pub fn app_id() -> String {
    format!("{}.Pdf", katna_core::ids::PREFIX)
}

/// Where the menu bar is served on the session bus, for KDE's global
/// menu (`menu.rs`).
#[cfg(target_os = "linux")]
pub fn menu_bar_path() -> String {
    format!("/{}/Pdf/MenuBar", katna_core::ids::PREFIX.replace('.', "/"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::app_id;

    /// The desktop entry and icon in `packaging/` are named after the app
    /// ID and name it, so the desktop matches the window to its entry.
    #[test]
    fn packaging_uses_the_app_id() {
        let packaging = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging");
        let id = app_id();
        let desktop = std::fs::read_to_string(packaging.join(format!("desktop/{id}.desktop")))
            .expect("the desktop entry is named after the app ID");
        assert!(desktop.lines().any(|l| l == format!("Icon={id}")));
        assert!(desktop.lines().any(|l| l == format!("StartupWMClass={id}")));
        assert!(packaging.join(format!("icons/{id}.svg")).is_file());
        let policy = std::fs::read_to_string(packaging.join(format!("polkit/{id}.update.policy")))
            .expect("the update's polkit action is named after the app ID");
        assert!(policy.contains(&format!("<action id=\"{id}.update\">")));
        assert!(policy.contains(crate::update::ARCH_HELPER));
    }
}
