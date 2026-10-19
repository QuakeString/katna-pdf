// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing the outline, the links and the named destinations.

use std::io::Write as _;

use katna_pdf_cos::{Destination, Document, Link, LinkTarget, NavError, OutlineItem};

/// A file of `objs`, object 1 first, with a classic table.
fn file(objs: &[&str]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{o}\nendobj\n", i + 1).unwrap();
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).unwrap();
    for o in offsets {
        writeln!(out, "{o:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out
}

fn three_pages() -> Document {
    Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R /Dests << /old [4 0 R /Fit] >> >>",
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] \
           /Annots [6 0 R 7 0 R 8 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] >>",
        "<< /Type /Annot /Subtype /Link /Rect [10 10 50 30] /Dest /old >>",
        "<< /Type /Annot /Subtype /Text /Rect [60 10 80 30] >>",
        "<< /Type /Annot /Subtype /Link /Rect [100 10 150 30] \
           /A << /S /URI /URI (https://a.example) >> >>",
    ]))
    .unwrap()
}

fn to(page: usize, top: Option<f32>) -> Option<Destination> {
    Some(Destination { page, top })
}

fn item(
    title: &str,
    dest: Option<Destination>,
    open: bool,
    children: Vec<OutlineItem>,
) -> OutlineItem {
    OutlineItem {
        title: title.into(),
        dest,
        open,
        children,
    }
}

#[test]
fn the_outline_is_written_read_back_and_taken_out() {
    let doc = three_pages();
    let items = vec![
        item(
            "Chapter one",
            to(0, Some(700.0)),
            true,
            vec![item(
                "Part ā",
                to(1, None),
                false,
                vec![item("Deep", to(2, Some(10.0)), false, vec![])],
            )],
        ),
        item("Heading only", None, false, vec![]),
        item("End", to(2, None), false, vec![]),
    ];
    let out = Document::load(doc.set_outline(&items).unwrap()).unwrap();
    assert_eq!(out.outline(), items);
    // Chapter one is open with one visible child: 3 + 1 shown.
    let catalog = out.catalog().unwrap();
    let outlines = out.resolve_key(&catalog, "Outlines");
    assert_eq!(outlines.as_dict().unwrap().int("Count"), Some(4));
    let gone = Document::load(out.set_outline(&[]).unwrap()).unwrap();
    assert!(gone.outline().is_empty());
    assert!(gone.catalog().unwrap().get("Outlines").is_none());
    assert_eq!(
        doc.set_outline(&[item("x", to(9, None), false, vec![])]),
        Err(NavError::NoPage(9))
    );
}

#[test]
fn links_are_taken_out_and_put_in() {
    let doc = three_pages();
    let links = doc.links();
    assert_eq!(links[0].len(), 2);
    let web = Link {
        rect: [200.0, 300.0, 260.0, 320.0],
        target: LinkTarget::Uri("https://b.example/?q=1".into()),
    };
    let page = Link {
        rect: [20.0, 400.0, 90.0, 420.0],
        target: LinkTarget::Page(Destination {
            page: 2,
            top: Some(500.0),
        }),
    };
    // The first link out, two new in; the note stays.
    let out = Document::load(
        doc.change_links(0, &[0], &[web.clone(), page.clone()])
            .unwrap(),
    )
    .unwrap();
    let after = out.links();
    assert_eq!(after[0], vec![links[0][1].clone(), web.clone(), page]);
    assert_eq!(out.annotations()[0].len(), 1);
    // Links onto a page that had none.
    let out = Document::load(
        out.change_links(1, &[], std::slice::from_ref(&web))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(out.links()[1], vec![web]);
    assert_eq!(doc.change_links(0, &[5], &[]), Err(NavError::NoLink(5)));
}

#[test]
fn named_destinations_are_listed_and_changed() {
    let doc = three_pages();
    assert_eq!(
        doc.named_destinations(),
        vec![("old".into(), to(1, None).unwrap())]
    );
    let names = vec![
        ("old".to_string(), to(2, None).unwrap()),
        ("intro".to_string(), to(0, Some(750.0)).unwrap()),
    ];
    let out = Document::load(doc.set_named_destinations(&names).unwrap()).unwrap();
    let mut want = names.clone();
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(out.named_destinations(), want);
    // The link that went by name now finds the name's new page.
    assert_eq!(
        out.links()[0][0].target,
        LinkTarget::Page(to(2, None).unwrap())
    );
    assert_eq!(
        doc.set_named_destinations(&[(" ".into(), to(0, None).unwrap())]),
        Err(NavError::EmptyName)
    );
    let none = Document::load(out.set_named_destinations(&[]).unwrap()).unwrap();
    assert!(none.named_destinations().is_empty());
}
