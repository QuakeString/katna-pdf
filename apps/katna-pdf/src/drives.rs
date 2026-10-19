// SPDX-License-Identifier: GPL-3.0-or-later

//! Cloud drives through Katna (docs/PLAN.md §4.6): the Google Drive and
//! OneDrive of each account already signed in to Katna Mail, asked of
//! `katna-daemon` over D-Bus. Nothing here starts the daemon: without
//! it, or without such an account, there are no drives and the library
//! shows none. Every call blocks; run them off the window's thread.

use std::path::PathBuf;
use std::sync::OnceLock;

use futures_lite::future::block_on;
use katna_dbus::{CloudEntry, CloudListing, PimProxy, zbus};

pub use katna_dbus::{cloud_place, cloud_state};

/// A drive: an account of Katna Mail's whose sign-in lets Katna read it.
#[derive(Debug, Clone, PartialEq)]
pub struct Drive {
    pub account: i64,
    /// `google` or `microsoft`.
    pub provider: String,
    /// The account's address, or its name when it has none.
    pub address: String,
}

/// The session bus, once reached.
fn bus() -> Result<zbus::Connection, String> {
    static BUS: OnceLock<zbus::Connection> = OnceLock::new();
    if let Some(bus) = BUS.get() {
        return Ok(bus.clone());
    }
    let bus = block_on(katna_dbus::session()).map_err(|e| e.to_string())?;
    Ok(BUS.get_or_init(|| bus).clone())
}

fn pim(bus: &zbus::Connection) -> Result<PimProxy<'static>, String> {
    block_on(PimProxy::new(bus)).map_err(|e| e.to_string())
}

/// The drives there are: none when the daemon is not running.
pub fn drives() -> Vec<Drive> {
    let Ok(bus) = bus() else {
        return Vec::new();
    };
    if !block_on(katna_dbus::daemon_running(&bus)) {
        return Vec::new();
    }
    let Ok(pim) = pim(&bus) else {
        return Vec::new();
    };
    let accounts = block_on(pim.accounts()).unwrap_or_default();
    accounts
        .into_iter()
        .filter(|a| matches!(a.sign_in.as_str(), "google" | "microsoft"))
        .filter(|a| block_on(pim.cloud_readable(a.id)).unwrap_or(false))
        .map(|a| Drive {
            account: a.id,
            provider: a.sign_in,
            address: if a.address.is_empty() {
                a.display_name
            } else {
                a.address
            },
        })
        .collect()
}

/// One page of a drive: folder `what` (`root` for the top), what was
/// shared with the account, or a search for the words in `what` (`place`
/// is one of [`cloud_place`]). `page` is the last page's `next`.
pub fn list(account: i64, place: &str, what: &str, page: &str) -> Result<CloudListing, String> {
    let pim = pim(&bus()?)?;
    block_on(pim.cloud_list(account, place, what, page)).map_err(|e| e.to_string())
}

/// Downloads `entry` into Katna's cache: its path there. A drive's own
/// document comes as a PDF.
pub fn fetch(account: i64, entry: &CloudEntry) -> Result<PathBuf, String> {
    let pim = pim(&bus()?)?;
    block_on(pim.cloud_fetch(account, entry))
        .map(PathBuf::from)
        .map_err(|e| e.to_string())
}

/// The drive's picture of an entry, about `width` pixels wide (PNG or
/// JPEG).
pub fn thumbnail(account: i64, link: &str, width: u32) -> Result<Vec<u8>, String> {
    let pim = pim(&bus()?)?;
    block_on(pim.cloud_thumbnail(account, link, width)).map_err(|e| e.to_string())
}

/// Whether the library lists `entry`: folders, PDFs, and the drive's own
/// documents, which open as PDFs.
pub fn shown(entry: &CloudEntry) -> bool {
    entry.folder
        || entry.native
        || entry.mime == "application/pdf"
        || entry.name.to_ascii_lowercase().ends_with(".pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_folders_and_pdfs_are_listed() {
        let entry = |name: &str, mime: &str, folder: bool, native: bool| CloudEntry {
            name: name.into(),
            mime: mime.into(),
            folder,
            native,
            ..CloudEntry::default()
        };
        assert!(shown(&entry("Tax", "", true, false)));
        assert!(shown(&entry("lease.PDF", "", false, false)));
        assert!(shown(&entry("x", "application/pdf", false, false)));
        assert!(shown(&entry(
            "Notes",
            "application/vnd.google-apps.document",
            false,
            true
        )));
        assert!(!shown(&entry("photo.jpg", "image/jpeg", false, false)));
    }
}
