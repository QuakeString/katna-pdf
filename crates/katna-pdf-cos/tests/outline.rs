// SPDX-License-Identifier: GPL-3.0-or-later

//! The outline and its destinations.

use std::io::Write as _;

use katna_pdf_cos::{Destination, Document, OutlineItem, text_string};

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

fn to(page: usize, top: Option<f32>) -> Option<Destination> {
    Some(Destination { page, top })
}

#[test]
fn the_outline_is_read_with_every_kind_of_destination() {
    let bytes = file(&[
        // 1: catalog, with old-style /Dests and a /Names tree.
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R \
           /Dests << /old [4 0 R /Fit] >> /Names << /Dests 13 0 R >> >>",
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        // 6: the outline root.
        "<< /Type /Outlines /First 7 0 R /Last 9 0 R /Count 4 >>",
        // 7: an explicit destination, open, with two children.
        "<< /Title (Chapter 1) /Parent 6 0 R /Next 8 0 R /First 10 0 R /Last 11 0 R \
           /Count 2 /Dest [3 0 R /XYZ 0 700 0] >>",
        // 8: a go-to action, UTF-16 title.
        "<< /Title <FEFF00500061007200740020004900490020> /Parent 6 0 R /Prev 7 0 R \
           /Next 9 0 R /A << /S /GoTo /D [5 0 R /FitH 500] >> >>",
        // 9: a named destination through the name tree; closed children.
        "<< /Title (Appendix) /Parent 6 0 R /Prev 8 0 R /Dest (app) \
           /First 12 0 R /Last 12 0 R /Count -1 >>",
        // 10, 11: the chapter's children; 11 uses the old /Dests.
        "<< /Title (1.1 \\205 Start) /Parent 7 0 R /Next 11 0 R /Dest [4 0 R /Fit] >>",
        "<< /Title (1.2) /Parent 7 0 R /Prev 10 0 R /Dest /old >>",
        // 12: a link to another program, no destination here.
        "<< /Title (Website) /Parent 9 0 R /A << /S /URI /URI (https://example.com) >> >>",
        // 13: the name tree, with a kid whose limits skip it.
        "<< /Kids [14 0 R 15 0 R] >>",
        "<< /Limits [(a) (az)] /Names [(app) << /D [5 0 R /XYZ null null null] >>] >>",
        "<< /Limits [(b) (c)] /Names [(bad) [3 0 R /Fit]] >>",
    ]);
    let doc = Document::load(bytes).unwrap();
    let item = |title: &str, dest, open, children| OutlineItem {
        title: title.into(),
        dest,
        open,
        children,
    };
    assert_eq!(
        doc.outline(),
        vec![
            item(
                "Chapter 1",
                to(0, Some(700.0)),
                true,
                vec![
                    item("1.1 – Start", to(1, None), false, vec![]),
                    item("1.2", to(1, None), false, vec![]),
                ]
            ),
            item("Part II", to(2, Some(500.0)), false, vec![]),
            item(
                "Appendix",
                to(2, None),
                false,
                vec![item("Website", None, false, vec![])]
            ),
        ]
    );
}

#[test]
fn a_looping_outline_ends() {
    let bytes = file(&[
        "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /First 5 0 R >>",
        // Each item is the other's next, and its own first child.
        "<< /Title (A) /Next 6 0 R /First 5 0 R >>",
        "<< /Title (B) /Next 5 0 R >>",
    ]);
    let doc = Document::load(bytes).unwrap();
    let titles: Vec<_> = doc.outline().into_iter().map(|i| i.title).collect();
    assert_eq!(titles, ["A", "B"]);
}

#[test]
fn a_file_without_an_outline_has_none() {
    let bytes = file(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ]);
    assert!(Document::load(bytes).unwrap().outline().is_empty());
}

#[test]
fn text_strings_are_decoded() {
    assert_eq!(text_string(b"\xfe\xff\x00H\x00i"), "Hi");
    assert_eq!(text_string(b"\xef\xbb\xbfcaf\xc3\xa9"), "café");
    assert_eq!(text_string(b"\x8dQuote\x8e \xa0"), "“Quote” €");
    assert_eq!(text_string(b"  Two\nlines \r"), "Two lines");
}
