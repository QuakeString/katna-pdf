// SPDX-License-Identifier: GPL-3.0-or-later

//! PDFs attached to mail in Katna Mail (docs/PLAN.md §4.6): listed from
//! its store, read-only, as its Files page lists them, and taken out of
//! their mail into a cache folder to open. A mail not downloaded yet is
//! asked of `katna-daemon` first. Without Katna Mail there are none.

use std::path::PathBuf;

use katna_core::Paths;
use katna_store::{LibraryFile, MessageId, Mode, Store};
use mail_parser::{MessageParser, MimeHeaders, PartType};

use crate::drives;

/// Attachments of every kind looked through for PDFs, newest first.
const SCAN: usize = 5000;

/// A PDF attached to a mail.
#[derive(Debug, Clone, PartialEq)]
pub struct MailPdf {
    pub message: i64,
    pub name: String,
    pub size: u64,
    pub date: Option<i64>,
    pub subject: String,
    /// Who sent it: their name, or their address.
    pub from: String,
    /// Its mail is downloaded, so it opens at once.
    pub downloaded: bool,
    nth: usize,
    order: usize,
}

impl MailPdf {
    /// Whether `words` (lower case) are all in its name, subject or
    /// sender.
    pub fn matches(&self, words: &[String]) -> bool {
        let text = format!("{} {} {}", self.name, self.subject, self.from).to_lowercase();
        words.iter().all(|w| text.contains(w.as_str()))
    }
}

fn is_pdf(file: &LibraryFile) -> bool {
    file.mime == "application/pdf" || file.name.to_ascii_lowercase().ends_with(".pdf")
}

fn store() -> Result<Option<Store>, String> {
    let Ok(paths) = Paths::from_env() else {
        return Ok(None);
    };
    match Store::open(&paths, Mode::ReadOnly) {
        Ok(store) => Ok(Some(store)),
        // Katna Mail was never set up here.
        Err(katna_store::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The PDFs attached to mail, newest first; none without Katna Mail.
pub fn pdfs() -> Result<Vec<MailPdf>, String> {
    let Some(store) = store()? else {
        return Ok(Vec::new());
    };
    let files = store.library_files(SCAN).map_err(|e| e.to_string())?;
    Ok(files
        .into_iter()
        .filter(is_pdf)
        .map(|f| MailPdf {
            message: f.message.0,
            from: f.from_name.unwrap_or(f.from_email),
            name: f.name,
            size: f.size,
            date: f.date,
            subject: f.subject,
            downloaded: f.downloaded,
            nth: f.nth,
            order: f.order,
        })
        .collect())
}

/// The stored mail `message`, if it is downloaded.
fn raw(store: &Store, message: i64) -> Result<Option<Vec<u8>>, String> {
    let Some(stored) = store
        .messages_by_id(&[MessageId(message)])
        .map_err(|e| e.to_string())?
        .pop()
    else {
        return Ok(None);
    };
    let Some(hash) = stored.blob_hash else {
        return Ok(None);
    };
    store.blobs().get(&hash).map_err(|e| e.to_string())
}

/// Takes `file` out of its mail, downloading the mail first if it must:
/// where the PDF is now.
pub fn open(file: &MailPdf) -> Result<PathBuf, String> {
    let gone = || crate::text::t("library-mail-gone", &[]);
    let store = store()?.ok_or_else(gone)?;
    let raw = match raw(&store, file.message)? {
        Some(raw) => raw,
        None => {
            drives::fetch_body(file.message)?;
            raw(&store, file.message)?.ok_or_else(gone)?
        }
    };
    let bytes = extract(&raw, &file.name, file.nth, file.order).ok_or_else(gone)?;
    let paths = Paths::from_env().map_err(|e| e.to_string())?;
    let dir = paths
        .cache_dir()
        .join("pdf-mail")
        .join(format!("{}-{}", file.message, file.order));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe_name(&file.name));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Attachment `name` of `raw`, the `nth` of that name, as Katna Mail
/// counts them; failing that, the first of that name, or the `order`th.
fn extract(raw: &[u8], name: &str, nth: usize, order: usize) -> Option<Vec<u8>> {
    let message = MessageParser::default().parse(raw)?;
    let parts: Vec<_> = message.attachments().collect();
    let named = |p: &&&mail_parser::MessagePart<'_>| {
        p.attachment_name().map(str::trim) == Some(name.trim())
    };
    let part = parts
        .iter()
        .filter(named)
        .nth(nth)
        .or_else(|| parts.iter().find(named))
        .or_else(|| parts.get(order))?;
    match &part.body {
        PartType::Message(_) => None,
        _ => Some(part.contents().to_vec()),
    }
}

/// `name` as a file name on every system.
fn safe_name(name: &str) -> String {
    let kept: String = name
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let kept = kept.trim_matches(|c| c == '.' || c == ' ').to_owned();
    if kept.is_empty() {
        "attachment.pdf".to_owned()
    } else {
        kept
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIL: &str = "From: a@example.org\r\n\
Subject: Two forms\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
Here.\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"form.pdf\"\r\n\
Content-Disposition: attachment; filename=\"form.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQgb25l\r\n\
--b\r\n\
Content-Type: application/pdf; name=\"form.pdf\"\r\n\
Content-Disposition: attachment; filename=\"form.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQgdHdv\r\n\
--b--\r\n";

    #[test]
    fn the_right_attachment_is_taken_out() {
        let raw = MAIL.as_bytes();
        assert_eq!(extract(raw, "form.pdf", 0, 0).unwrap(), b"%PDF-1.4 one");
        assert_eq!(extract(raw, "form.pdf", 1, 1).unwrap(), b"%PDF-1.4 two");
        // Renamed since it was listed: its place.
        assert_eq!(extract(raw, "other.pdf", 0, 1).unwrap(), b"%PDF-1.4 two");
        assert!(extract(raw, "other.pdf", 0, 5).is_none());
    }

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name(" a/b:c.pdf "), "a_b_c.pdf");
        assert_eq!(safe_name(".."), "attachment.pdf");
    }

    #[test]
    fn words_find_name_subject_and_sender() {
        let pdf = MailPdf {
            message: 1,
            name: "Lease.pdf".into(),
            size: 0,
            date: None,
            subject: "Flat in Pune".into(),
            from: "Asha Rao".into(),
            downloaded: true,
            nth: 0,
            order: 0,
        };
        assert!(pdf.matches(&["lease".into(), "pune".into()]));
        assert!(pdf.matches(&["asha".into()]));
        assert!(!pdf.matches(&["invoice".into()]));
    }
}
