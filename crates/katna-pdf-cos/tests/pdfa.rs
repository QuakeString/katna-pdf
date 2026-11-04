// SPDX-License-Identifier: GPL-3.0-or-later

//! Files checked for and made PDF/A.

use katna_pdf_cos::{
    Dict, Document, FontWant, Object, PdfaIssue, Permissions, system_font, text_pdf,
};

fn js(script: &str) -> Object {
    let mut a = Dict::new();
    a.set("S", Object::name("JavaScript"));
    a.set("JS", Object::string(script));
    Object::Dict(a)
}

/// A report (standard fonts, no intent) with a script on opening, one
/// on printing and a file kept in it.
fn messy() -> Document {
    let doc = Document::load(text_pdf("Note", &["  Hello café".to_string()])).unwrap();
    let bytes = doc
        .attach_files(&[("a.txt".into(), b"hi".to_vec(), String::new())])
        .unwrap();
    let doc = Document::load(bytes).unwrap();
    let mut up = doc.update().unwrap();
    let root = doc.trailer().get("Root").and_then(Object::as_ref).unwrap();
    let mut catalog = doc.catalog().unwrap();
    catalog.set("OpenAction", js("app.alert('hi');"));
    let mut aa = Dict::new();
    aa.set("WP", js("this.print = false;"));
    catalog.set("AA", Object::Dict(aa));
    up.replace(root, Object::Dict(catalog));
    Document::load(up.finish()).unwrap()
}

#[test]
fn issues_are_found() {
    let issues = messy().pdfa_issues();
    assert!(issues.contains(&PdfaIssue::NoOutputIntent));
    assert!(issues.contains(&PdfaIssue::NotMarked));
    assert!(issues.contains(&PdfaIssue::Scripts(2)));
    assert!(issues.contains(&PdfaIssue::Attachments(1)));
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, PdfaIssue::FontNotEmbedded(_))),
        "{issues:?}"
    );
}

#[test]
fn the_file_is_made_pdfa() {
    let doc = messy();
    let done = doc.to_pdfa(&system_font).unwrap();
    assert_eq!(done.scripts_removed, 2);
    assert_eq!(done.attachments_removed, 1);
    let out = Document::load(done.bytes).unwrap();
    assert!(out.document_actions().is_empty());
    assert!(out.attachments().is_empty());
    let issues = out.pdfa_issues();
    let fonts_available = system_font(FontWant {
        family: katna_pdf_cos::FontFamily::Sans,
        bold: false,
        italic: false,
    })
    .is_some();
    if fonts_available {
        assert!(done.left.is_empty(), "{:?}", done.left);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(!done.fonts_embedded.is_empty());
        // The text is still what it was, through the embedded font.
        let text = out
            .text_blocks(0)
            .into_iter()
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Hello caf"), "{text}");
    } else {
        assert!(
            issues
                .iter()
                .all(|i| matches!(i, PdfaIssue::FontNotEmbedded(_)))
        );
    }
    let catalog = out.catalog().unwrap();
    assert!(matches!(out.resolve_key(&catalog, "OutputIntents"), Object::Array(a) if a.len() == 1));
    let Object::Stream(xmp) = out.resolve_key(&catalog, "Metadata") else {
        panic!("no XMP");
    };
    let xmp = String::from_utf8(xmp.data).unwrap();
    assert!(xmp.contains("<pdfaid:part>2</pdfaid:part>"));
    assert!(xmp.contains("<pdfaid:conformance>B</pdfaid:conformance>"));
    assert!(xmp.contains("<dc:title>"));
}

#[test]
fn a_locked_file_comes_out_open() {
    let doc = messy();
    let locked = doc.lock("user", "owner", Permissions::ALL).unwrap();
    let mut locked = Document::load(locked).unwrap();
    assert!(locked.pdfa_issues().contains(&PdfaIssue::Locked));
    // Not the owner: refused.
    locked.unlock("user").unwrap();
    assert!(locked.to_pdfa(&system_font).is_err());
    locked.unlock("owner").unwrap();
    let done = locked.to_pdfa(&system_font).unwrap();
    let out = Document::load(done.bytes).unwrap();
    assert!(!out.is_encrypted());
    assert!(!out.pdfa_issues().contains(&PdfaIssue::Locked));
}
