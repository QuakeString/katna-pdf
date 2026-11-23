// SPDX-License-Identifier: GPL-3.0-or-later

//! Portfolios: files kept in a document listed in columns of their own.

use katna_pdf_cos::{ColumnKind, Dict, Document, ItemValue, Name, Object, PortfolioView, text_pdf};

fn name(s: &str) -> Object {
    Object::Name(Name::new(s))
}

fn string(s: &str) -> Object {
    Object::String {
        bytes: s.as_bytes().to_vec(),
        hex: false,
    }
}

fn field(subtype: &str, title: &str, order: i64) -> Object {
    Object::Dict(Dict(vec![
        (Name::new("Subtype"), name(subtype)),
        (Name::new("N"), string(title)),
        (Name::new("O"), Object::Int(order)),
    ]))
}

/// A document keeping three files, made a portfolio with a column of
/// its own (Client), sorted by it downwards.
fn portfolio() -> Document {
    let doc = Document::load(text_pdf("Cover", &["  A portfolio".to_string()])).unwrap();
    let doc = Document::load(
        doc.attach_files(&[
            ("a.pdf".into(), b"%PDF-1.4 a".to_vec(), "First".into()),
            ("b.pdf".into(), b"%PDF-1.4 bb".to_vec(), String::new()),
            ("c.txt".into(), b"ccc".to_vec(), String::new()),
        ])
        .unwrap(),
    )
    .unwrap();
    let mut up = doc.update().unwrap();
    let catalog = doc.catalog().unwrap();
    let Object::Dict(names) = doc.resolve_key(&catalog, "Names") else {
        panic!("no names")
    };
    let Object::Dict(tree) = doc.resolve_key(&names, "EmbeddedFiles") else {
        panic!("no tree")
    };
    let Object::Array(pairs) = doc.resolve_key(&tree, "Names") else {
        panic!("no pairs")
    };
    let clients = [Some("Zeta"), None, Some("Alpha")];
    for (pair, client) in pairs.chunks(2).zip(clients) {
        let r = pair[1].as_ref().unwrap();
        let mut spec = doc.get(r).unwrap().as_dict().cloned().unwrap();
        let mut ci = Dict::new();
        if let Some(client) = client {
            ci.set("client", string(client));
        }
        ci.set("due", string("D:20261201000000Z"));
        ci.set(
            "fee",
            Object::Dict(Dict(vec![
                (Name::new("D"), Object::Int(1200)),
                (Name::new("P"), string("Rs ")),
            ])),
        );
        spec.set("CI", Object::Dict(ci));
        up.replace(r, Object::Dict(spec));
    }
    let schema = Dict(vec![
        (Name::new("client"), field("S", "Client", 2)),
        (Name::new("file"), field("F", "Name", 1)),
        (Name::new("size"), field("Size", "Size", 3)),
        (Name::new("due"), field("D", "Due", 4)),
        (Name::new("fee"), field("N", "Fee", 5)),
    ]);
    let collection = Dict(vec![
        (Name::new("Type"), name("Collection")),
        (Name::new("Schema"), Object::Dict(schema)),
        (Name::new("D"), string("b.pdf")),
        (Name::new("View"), name("D")),
        (
            Name::new("Sort"),
            Object::Dict(Dict(vec![
                (Name::new("S"), name("client")),
                (Name::new("A"), Object::Bool(false)),
            ])),
        ),
    ]);
    let mut catalog = catalog;
    catalog.set("Collection", Object::Dict(collection));
    let root = doc.trailer().get("Root").and_then(Object::as_ref).unwrap();
    up.replace(root, Object::Dict(catalog));
    Document::load(up.finish()).unwrap()
}

#[test]
fn a_plain_file_is_no_portfolio() {
    let doc = Document::load(text_pdf("Plain", &["  Hello".to_string()])).unwrap();
    assert!(doc.portfolio().is_none());
}

#[test]
fn a_portfolio_lists_its_columns_values_and_order() {
    let doc = portfolio();
    let p = doc.portfolio().unwrap();
    assert_eq!(
        p.columns
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect::<Vec<_>>(),
        [
            ("Name", ColumnKind::Name),
            ("Client", ColumnKind::Text),
            ("Size", ColumnKind::Size),
            ("Due", ColumnKind::Date),
            ("Fee", ColumnKind::Number),
        ]
    );
    assert_eq!(p.initial.as_deref(), Some("b.pdf"));
    assert_eq!(p.view, PortfolioView::Details);
    assert_eq!(p.sort, [("client".to_string(), false)]);
    let files = doc.attachments();
    let names = |order: Vec<usize>| -> Vec<String> {
        order.into_iter().map(|i| files[i].name.clone()).collect()
    };
    // Downwards by client; the file without one last.
    assert_eq!(names(p.order(&files, None)), ["a.pdf", "c.txt", "b.pdf"]);
    assert_eq!(
        names(p.order(&files, Some(("size", true)))),
        ["c.txt", "a.pdf", "b.pdf"]
    );
    let a = files.iter().find(|f| f.name == "a.pdf").unwrap();
    assert_eq!(
        a.value(p.column("client").unwrap()),
        Some(ItemValue::Text("Zeta".into()))
    );
    assert_eq!(
        a.value(p.column("due").unwrap()),
        Some(ItemValue::Date("D:20261201000000Z".into()))
    );
    // A number with a prefix stays a number (the prefix is for text).
    assert_eq!(
        a.value(p.column("fee").unwrap()),
        Some(ItemValue::Number(1200.0))
    );
    assert_eq!(
        a.value(p.column("size").unwrap()),
        Some(ItemValue::Number(10.0))
    );
}
