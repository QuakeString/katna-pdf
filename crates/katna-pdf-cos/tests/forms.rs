// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading and filling in forms.

use std::io::Write as _;

use katna_pdf_cos::{Document, Field, FieldKind, FieldValue, Object};

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

fn form() -> Document {
    Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R 7 0 R] \
           /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv << /Type /Font /Subtype /Type1 \
           /BaseFont /Helvetica >> >> >> >> >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] \
           /Annots [5 0 R 6 0 R 8 0 R 9 0 R] >>",
        // 4: a text field in a group, its widget a kid.
        "<< /T (person) /Kids [10 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 10 0 R /Rect [10 10 110 30] >>",
        // 6: a check box that is its own widget, on.
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /V /Yes /AS /Yes \
           /Rect [10 40 20 50] /AP << /N << /Yes 11 0 R /Off 11 0 R >> >> >>",
        // 7: a radio group with two buttons.
        "<< /FT /Btn /Ff 49152 /T (size) /V /Off /Kids [8 0 R 9 0 R] >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [10 60 20 70] /AS /Off \
           /AP << /N << /S 11 0 R /Off 11 0 R >> >> >>",
        "<< /Type /Annot /Subtype /Widget /Parent 7 0 R /Rect [30 60 40 70] /AS /Off \
           /AP << /N << /L 11 0 R /Off 11 0 R >> >> >>",
        // 10: the text field, under "person".
        "<< /FT /Tx /T (name) /Parent 4 0 R /V (Ada) /Kids [5 0 R] /DA (/Helv 10 Tf 0 g) >>",
        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length 0 >>\nstream\n\nendstream",
    ]))
    .unwrap()
}

fn find<'a>(fields: &'a [Field], name: &str) -> &'a Field {
    fields.iter().find(|f| f.name == name).unwrap()
}

#[test]
fn reads_text_fields_check_boxes_and_radio_groups() {
    let fields = form().form();
    assert_eq!(fields.len(), 3);
    let name = find(&fields, "person.name");
    assert!(matches!(
        name.kind,
        FieldKind::Text {
            multiline: false,
            ..
        }
    ));
    assert_eq!(name.value.as_deref(), Some("Ada"));
    assert_eq!(name.widgets.len(), 1);
    assert_eq!(name.widgets[0].rect, [10.0, 10.0, 110.0, 30.0]);
    let agree = find(&fields, "agree");
    assert_eq!(agree.kind, FieldKind::CheckBox);
    assert_eq!(agree.value.as_deref(), Some("Yes"));
    assert_eq!(agree.widgets[0].on.as_deref(), Some("Yes"));
    let size = find(&fields, "size");
    assert_eq!(size.kind, FieldKind::Radio);
    assert_eq!(size.value, None);
    let states: Vec<_> = size.widgets.iter().map(|w| w.on.as_deref()).collect();
    assert_eq!(states, [Some("S"), Some("L")]);
}

#[test]
fn filling_in_is_an_update_that_reads_back() {
    let doc = form();
    let fields = doc.form();
    let changes = [
        (
            find(&fields, "person.name").id,
            FieldValue::Text("Grace (é)".into()),
        ),
        (find(&fields, "agree").id, FieldValue::State(None)),
        (
            find(&fields, "size").id,
            FieldValue::State(Some("L".into())),
        ),
    ];
    let bytes = doc.fill(&changes).unwrap();
    assert!(bytes.starts_with(doc.bytes()));
    let filled = Document::load(bytes).unwrap();
    let fields = filled.form();
    assert_eq!(
        find(&fields, "person.name").value.as_deref(),
        Some("Grace (é)")
    );
    assert_eq!(find(&fields, "agree").value, None);
    assert_eq!(find(&fields, "size").value.as_deref(), Some("L"));
    // Each radio button shows whether it is the one on.
    let shown: Vec<_> = find(&fields, "size")
        .widgets
        .iter()
        .map(|w| match filled.get(w.id) {
            Some(Object::Dict(d)) => d.name("AS").map(<[u8]>::to_vec),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [Some(b"Off".to_vec()), Some(b"L".to_vec())]);
    // The text field's widget has a new appearance with the text.
    let widget = find(&fields, "person.name").widgets[0].id;
    let Some(Object::Dict(widget)) = filled.get(widget) else {
        panic!("no widget");
    };
    let Object::Dict(ap) = filled.resolve_key(&widget, "AP") else {
        panic!("no appearance");
    };
    let Object::Stream(normal) = filled.resolve_key(&ap, "N") else {
        panic!("no normal appearance");
    };
    let content = String::from_utf8_lossy(&normal.data);
    assert!(content.contains("/Helv 10.00 Tf"), "{content}");
    assert!(content.contains("(Grace \\(\\351\\)) Tj"), "{content}");
}
