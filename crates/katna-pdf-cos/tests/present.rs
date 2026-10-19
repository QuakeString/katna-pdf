// SPDX-License-Identifier: GPL-3.0-or-later

//! How pages come on in a presentation.

use std::io::Write as _;

use katna_pdf_cos::{Document, TransitionStyle};

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
fn transitions_and_times_are_read() {
    let doc = Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Dur 5 \
         /Trans << /S /Wipe /D 0.5 /Di 270 >> >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] \
         /Trans << /S /Split /Dm /V /M /I >> >>",
    ]))
    .unwrap();
    let shows = doc.page_shows();
    assert_eq!(shows.len(), 3);
    assert_eq!(shows[0].transition, None);
    assert_eq!(shows[0].seconds, None);
    let wipe = shows[1].transition.unwrap();
    assert_eq!(wipe.style, TransitionStyle::Wipe);
    assert_eq!(wipe.seconds, 0.5);
    assert_eq!(wipe.direction, Some(270.0));
    assert_eq!(shows[1].seconds, Some(5.0));
    let split = shows[2].transition.unwrap();
    assert_eq!(split.style, TransitionStyle::Split);
    assert_eq!(split.seconds, 1.0);
    assert!(!split.horizontal && !split.outward);
    assert_eq!(split.direction, Some(0.0));
}
