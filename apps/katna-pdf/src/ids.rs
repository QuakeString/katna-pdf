// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's identifiers on the desktop, derived from Katna's prefix in
//! `katna_core::ids` so it is written in one place for the whole family.

/// The app ID: the desktop file's name and the Wayland app ID.
pub fn app_id() -> String {
    format!("{}.Pdf", katna_core::ids::PREFIX)
}
