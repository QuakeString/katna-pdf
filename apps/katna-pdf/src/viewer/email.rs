// SPDX-License-Identifier: GPL-3.0-or-later

//! Sends the file by email: a new message with it attached, in Katna
//! Mail when it is installed, else in the desktop's own mail app
//! (`xdg-email` on Linux, Outlook on Windows). Changes not yet saved go
//! in a copy, so what is sent is what is shown.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::Context;

use super::{Content, Viewer, said};

impl Viewer {
    /// Starts a new message with the file attached.
    pub(super) fn send_by_email(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        let unsaved = self.unsaved();
        cx.spawn(async move |this, cx| {
            let sent = cx
                .background_executor()
                .spawn(async move {
                    let file = match unsaved {
                        Some(bytes) => copy_with_changes(&path, &bytes)?,
                        None => path,
                    };
                    open_message(&file)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match sent {
                    Ok(Some(app)) => said("email-started", &[("app", app)]),
                    Ok(None) => said("email-started-desktop", &[]),
                    Err(error) => said("email-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The file as shown, changes and all, under its own name in a folder of
/// the cache emptied first.
fn copy_with_changes(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = katna_core::paths::Paths::from_env()
        .map_err(|e| e.to_string())?
        .cache_dir()
        .join("pdf")
        .join("mail");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .map_or_else(|| "document.pdf".into(), |n| n.to_owned());
    let copy = dir.join(name);
    std::fs::write(&copy, bytes).map_err(|e| e.to_string())?;
    Ok(copy)
}

/// Opens a new message with `file` attached; the name of the app that
/// took it, none for the desktop's own.
fn open_message(file: &Path) -> Result<Option<&'static str>, String> {
    // Katna Mail, when installed: it hands the files to the window
    // already open.
    if Command::new("katna-mail")
        .arg("--attach")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
    {
        return Ok(Some("Katna Mail"));
    }
    desktop_mail(file)
}

#[cfg(not(windows))]
fn desktop_mail(file: &Path) -> Result<Option<&'static str>, String> {
    let status = Command::new("xdg-email")
        .arg("--attach")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(None)
    } else {
        Err(format!("xdg-email ({status})"))
    }
}

#[cfg(windows)]
fn desktop_mail(file: &Path) -> Result<Option<&'static str>, String> {
    // Outlook, when installed, through its COM object; the path is passed
    // in the environment so nothing in it is read as a command.
    let script = "$o = New-Object -ComObject Outlook.Application; \
                  $m = $o.CreateItem(0); \
                  [void]$m.Attachments.Add($env:KATNA_PDF_ATTACH); \
                  $m.Display()";
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("KATNA_PDF_ATTACH", file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(Some("Outlook"))
    } else {
        Err(format!("Outlook ({status})"))
    }
}
