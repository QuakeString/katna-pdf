// SPDX-License-Identifier: GPL-3.0-or-later

//! Cloud drives through Katna (docs/PLAN.md §4.6): the Google Drive and
//! OneDrive of each account already signed in to Katna Mail, asked of
//! `katna-daemon` over D-Bus. Nothing here starts the daemon: without
//! it, or without such an account, there are no drives and the library
//! shows none. Every call blocks; run them off the window's thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use futures_lite::future::block_on;
use katna_dbus::{CloudEntry, CloudListing, PimProxy, zbus};

pub use katna_dbus::{cloud_place, cloud_state, drive_state};

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

/// Where a PDF opened from a drive came from, to save it back there.
#[derive(Debug, Clone, PartialEq)]
pub struct Origin {
    pub account: i64,
    /// The file's id in the drive.
    pub file: String,
}

/// The drive file behind each copy [`fetch`] made, by the copy's path.
fn origins() -> &'static Mutex<HashMap<PathBuf, Origin>> {
    static ORIGINS: OnceLock<Mutex<HashMap<PathBuf, Origin>>> = OnceLock::new();
    ORIGINS.get_or_init(Mutex::default)
}

/// Downloads `entry` into Katna's cache: its path there. A drive's own
/// document comes as a PDF. A PDF is remembered as the drive file's
/// copy, so saving it saves the drive file too ([`save_back`]); a drive's
/// own document, made into a PDF, is not.
pub fn fetch(account: i64, entry: &CloudEntry) -> Result<PathBuf, String> {
    let pim = pim(&bus()?)?;
    let path = block_on(pim.cloud_fetch(account, entry))
        .map(PathBuf::from)
        .map_err(|e| e.to_string())?;
    if !entry.native {
        let origin = Origin {
            account,
            file: entry.id.clone(),
        };
        origins().lock().unwrap().insert(path.clone(), origin);
    }
    Ok(path)
}

/// The drive file `path` is a copy of, when it was opened from a drive.
pub fn origin(path: &Path) -> Option<Origin> {
    origins().lock().unwrap().get(path).cloned()
}

/// The drive of `account` as people know it: the provider's drive and
/// the account's address.
pub fn drive_name(account: i64) -> String {
    let drive = drives().into_iter().find(|d| d.account == account);
    let Some(drive) = drive else {
        return crate::text::t("drive-save-the-drive", &[]);
    };
    let id = match drive.provider.as_str() {
        "microsoft" => "drive-save-onedrive",
        _ => "drive-save-google",
    };
    crate::text::t(id, &[("address", &drive.address)])
}

/// How often [`save_back`] asks how the save is going.
const POLL: Duration = Duration::from_millis(250);

/// Sends the copy at `path` back as the new contents of the drive file it
/// came from, which keeps its name, folder, sharing and history. Returns
/// once the drive has it. Blocks; run it off the window's thread.
pub fn save_back(origin: &Origin, path: &Path) -> Result<(), String> {
    let bus = bus()?;
    if !block_on(katna_dbus::daemon_running(&bus)) {
        return Err(crate::text::t("library-mail-no-daemon", &[]));
    }
    let pim = pim(&bus)?;
    let upload = block_on(pim.cloud_replace(origin.account, &origin.file, &path.to_string_lossy()))
        .map_err(|e| e.to_string())?;
    loop {
        std::thread::sleep(POLL);
        let status = block_on(pim.drive_upload_status(upload)).map_err(|e| e.to_string())?;
        match status.state.as_str() {
            drive_state::DONE => return Ok(()),
            drive_state::NEEDS_PERMISSION => {
                return Err(crate::text::t("drive-save-permission", &[]));
            }
            drive_state::FAILED => return Err(status.error),
            _ => {}
        }
    }
}

/// The drive's picture of an entry, about `width` pixels wide (PNG or
/// JPEG).
pub fn thumbnail(account: i64, link: &str, width: u32) -> Result<Vec<u8>, String> {
    let pim = pim(&bus()?)?;
    block_on(pim.cloud_thumbnail(account, link, width)).map_err(|e| e.to_string())
}

/// Asks Katna to download mail `message` whole; returns once it is
/// stored.
pub fn fetch_body(message: i64) -> Result<(), String> {
    let bus = bus()?;
    if !block_on(katna_dbus::daemon_running(&bus)) {
        return Err(crate::text::t("library-mail-no-daemon", &[]));
    }
    block_on(pim(&bus)?.fetch_body(message)).map_err(|e| e.to_string())
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
