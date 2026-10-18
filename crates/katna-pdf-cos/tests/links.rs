// SPDX-License-Identifier: GPL-3.0-or-later

//! Links on the pages.

use std::io::Write as _;

use katna_pdf_cos::{Destination, Document, Link, LinkTarget};

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

#[test]
fn links_go_to_pages_and_web_addresses() {
    let bytes = file(&[
        "<< /Type /Catalog /Pages 2 0 R /Dests << /end [4 0 R /XYZ 0 300 0] >> >>",
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /Annots [5 0 R 6 0 R 7 0 R 8 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        // A go-to link, its corners given the wrong way round.
        "<< /Type /Annot /Subtype /Link /Rect [200 120 100 100] /Dest /end >>",
        "<< /Type /Annot /Subtype /Link /Rect [10 10 50 20] \
           /A << /S /URI /URI (https://example.com/) >> >>",
        // Not a link, and a link going nowhere.
        "<< /Type /Annot /Subtype /Text /Rect [0 0 10 10] >>",
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [99 0 R /Fit] >>",
    ]);
    let doc = Document::load(bytes).unwrap();
    assert_eq!(
        doc.links(),
        vec![
            vec![
                Link {
                    rect: [100.0, 100.0, 200.0, 120.0],
                    target: LinkTarget::Page(Destination {
                        page: 1,
                        top: Some(300.0)
                    }),
                },
                Link {
                    rect: [10.0, 10.0, 50.0, 20.0],
                    target: LinkTarget::Uri("https://example.com/".into()),
                },
            ],
            vec![],
        ]
    );
}
