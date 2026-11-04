// SPDX-License-Identifier: GPL-3.0-or-later

//! The actions a document carries, listed.

use katna_pdf_cos::{
    ActionPlace, Dict, DocEvent, Document, FieldAction, FieldEvent, Object, text_pdf,
};

fn js(script: &str) -> Object {
    let mut a = Dict::new();
    a.set("S", Object::name("JavaScript"));
    a.set("JS", Object::string(script));
    Object::Dict(a)
}

#[test]
fn document_page_and_chained_actions_are_listed() {
    let doc = Document::load(text_pdf("Note", &["  Hello".to_string()])).unwrap();
    let mut up = doc.update().unwrap();
    let root = doc.trailer().get("Root").and_then(Object::as_ref).unwrap();
    let mut catalog = doc.catalog().unwrap();
    // A script of the document's own.
    let mut tree = Dict::new();
    tree.set(
        "Names",
        vec![Object::string("Setup"), js("var ready = true;")],
    );
    let mut names = Dict::new();
    names.set("JavaScript", Object::Dict(tree));
    catalog.set("Names", Object::Dict(names));
    // Opening goes to a page, then runs a script, chained.
    let mut open = Dict::new();
    open.set("S", Object::name("GoTo"));
    open.set("D", vec![Object::Ref(doc.pages()[0]), Object::name("Fit")]);
    open.set("Next", js("app.alert('hi');"));
    catalog.set("OpenAction", Object::Dict(open));
    let mut aa = Dict::new();
    aa.set("WP", js("this.print = false;"));
    catalog.set("AA", Object::Dict(aa));
    up.replace(root, Object::Dict(catalog));
    // The page says when it is opened.
    let page_ref = doc.pages()[0];
    let mut page = doc.get(page_ref).unwrap().as_dict().cloned().unwrap();
    let mut paa = Dict::new();
    paa.set("O", js("opened();"));
    page.set("AA", Object::Dict(paa));
    up.replace(page_ref, Object::Dict(page));
    let doc = Document::load(up.finish()).unwrap();

    let actions = doc.document_actions();
    let places: Vec<(&ActionPlace, &str, String)> = actions
        .iter()
        .map(|a| (&a.place, a.action.kind(), a.action.detail()))
        .collect();
    assert_eq!(
        places,
        [
            (
                &ActionPlace::Document("Setup".into()),
                "javascript",
                "var ready = true;".to_owned()
            ),
            (&ActionPlace::Open, "go-to", "page 1".to_owned()),
            (
                &ActionPlace::Open,
                "javascript",
                "app.alert('hi');".to_owned()
            ),
            (
                &ActionPlace::DocumentEvent(DocEvent::WillPrint),
                "javascript",
                "this.print = false;".to_owned()
            ),
            (
                &ActionPlace::Page {
                    page: 0,
                    opened: true
                },
                "javascript",
                "opened();".to_owned()
            ),
        ]
    );
    assert_eq!(FieldEvent::Format.name(), "format");
    assert!(matches!(actions[1].action, FieldAction::GoTo(_)));
}

#[test]
fn a_plain_file_has_none() {
    let doc = Document::load(text_pdf("Note", &["  Hello".to_string()])).unwrap();
    assert!(doc.document_actions().is_empty());
}
