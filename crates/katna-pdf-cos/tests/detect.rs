// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding the fields of a form printed to PDF.

use std::io::Write as _;

use katna_pdf_cos::{Document, FieldKind, FormDesign, NewField};

/// A one-page file drawing `content` in Helvetica.
fn page(content: &str) -> Document {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 600 800] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
    ];
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
    Document::load(out).unwrap()
}

fn text(x: f32, y: f32, s: &str) -> String {
    format!("BT /F1 12 Tf {x} {y} Td ({s}) Tj ET\n")
}

#[test]
fn blanks_boxes_and_ticks_become_fields() {
    let mut c = String::new();
    // A frame round the page is not a field.
    c += "30 30 540 740 re S\n";
    c += &text(50.0, 700.0, "Name: ____________________");
    c += &text(50.0, 650.0, "Email");
    c += "120 648 m 400 648 l S\n";
    c += &text(50.0, 610.0, "Address");
    c += "50 540 300 60 re S\n";
    c += &text(50.0, 500.0, "PAN");
    for i in 0..10 {
        c += &format!("{} 496 12 12 re S\n", 100 + i * 14);
    }
    c += "50 450 10 10 re S\n";
    c += &text(65.0, 451.0, "I agree");
    // A table's rules are not blanks.
    c += "50 400 m 300 400 l S 50 380 m 300 380 l S 50 380 m 50 400 l S 300 380 m 300 400 l S\n";
    c += &text(55.0, 385.0, "Total");
    let doc = page(&c);
    let found = doc.detect_fields(0, &["Name".into()]);
    let names: Vec<&str> = found.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Name 2", "Email", "Address", "PAN", "I agree"]);
    assert!(matches!(
        found[0].kind,
        NewField::Text {
            multiline: false,
            ..
        }
    ));
    assert!(found[1].rect[0] >= 119.0 && found[1].rect[2] <= 401.0);
    assert!(matches!(
        found[2].kind,
        NewField::Text {
            multiline: true,
            ..
        }
    ));
    assert!(matches!(
        found[3].kind,
        NewField::Text {
            max_len: Some(10),
            ..
        }
    ));
    assert_eq!(found[3].rect, [100.0, 496.0, 238.0, 508.0]);
    assert_eq!(found[4].kind, NewField::CheckBox);

    // Added, they are a form; found again, there is nothing new.
    let added = Document::load(
        doc.design_form(&FormDesign {
            add: found,
            ..FormDesign::default()
        })
        .unwrap(),
    )
    .unwrap();
    let form = added.form();
    assert_eq!(form.len(), 5);
    assert_eq!(form[4].kind, FieldKind::CheckBox);
    let names: Vec<String> = form.iter().map(|f| f.name.clone()).collect();
    assert!(added.detect_fields(0, &names).is_empty());
}
