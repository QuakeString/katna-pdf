// SPDX-License-Identifier: GPL-3.0-or-later

//! Putting a password on, opening with it, changing a locked file, and
//! taking the password off.

use katna_pdf_cos::{Document, Insert, LockError, Permissions, UnlockError, text_pdf};

fn letter() -> Document {
    Document::load(text_pdf("Letter", &["  Dear Asha, ünïcode".to_string()])).unwrap()
}

fn text(doc: &Document) -> String {
    doc.text_runs(0)
        .iter()
        .map(|r| r.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_locked_file_opens_only_with_its_password() {
    let plain = letter();
    let locked = plain.lock("asha", "boss", Permissions::ALL).unwrap();
    // Nothing readable is left in the clear.
    assert!(!locked.windows(4).any(|w| w == b"Dear"));
    let mut doc = Document::load(locked).unwrap();
    assert!(doc.is_encrypted() && doc.is_locked());
    assert!(text(&doc).is_empty());
    assert_eq!(doc.unlock("nobody"), Err(UnlockError::WrongPassword));
    doc.unlock("asha").unwrap();
    assert!(!doc.is_locked());
    assert!(!doc.opened_as_owner());
    assert_eq!(text(&doc), text(&plain));
    assert_eq!(doc.metadata().title.as_deref(), Some("Letter"));
    // Only the owner may take the password off.
    assert_eq!(doc.without_lock(), Err(LockError::NotOwner));
    let mut owner = Document::load(doc.bytes().to_vec()).unwrap();
    owner.unlock("boss").unwrap();
    assert!(owner.opened_as_owner());
    let open = Document::load(owner.without_lock().unwrap()).unwrap();
    assert!(!open.is_encrypted());
    assert_eq!(text(&open), text(&plain));
}

#[test]
fn an_empty_user_password_opens_by_itself_with_its_permissions() {
    let only_print = Permissions {
        print: true,
        print_well: false,
        change: false,
        copy: false,
        comment: false,
        fill_forms: false,
        assemble: false,
    };
    let locked = letter().lock("", "boss", only_print).unwrap();
    let doc = Document::load(locked).unwrap();
    assert!(doc.is_encrypted());
    assert!(!doc.is_locked());
    assert_eq!(doc.permissions(), only_print);
    assert!(text(&doc).contains("Dear Asha"));
}

#[test]
fn changes_to_a_locked_file_are_locked_too() {
    let locked = letter().lock("asha", "asha", Permissions::ALL).unwrap();
    let mut doc = Document::load(locked).unwrap();
    assert_eq!(
        doc.insert(0, &[]).map(|_| ()).unwrap_err().to_string(),
        "the PDF is encrypted"
    );
    doc.unlock("asha").unwrap();
    let out = doc
        .insert(
            0,
            &[Insert::Text {
                at: [72.0, 100.0],
                text: "Paid in full".into(),
                font: "Helvetica".into(),
                size: 12.0,
                colour: [0.0; 3],
            }],
        )
        .unwrap();
    assert!(out.starts_with(doc.bytes()));
    assert!(!out.windows(4).any(|w| w == b"Paid"));
    let mut again = Document::load(out).unwrap();
    assert!(again.is_locked());
    again.unlock("asha").unwrap();
    assert!(text(&again).contains("Paid in full"), "{}", text(&again));
    assert!(text(&again).contains("Dear Asha"));
}
