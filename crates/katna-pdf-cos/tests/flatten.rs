// SPDX-License-Identifier: GPL-3.0-or-later

//! Flattening: fields and comments drawn into the pages and taken out.

use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, Document, FieldSpec, FieldValue, Flatten, FormDesign,
    NewField, Object, text_pdf,
};

fn with_form_and_note() -> Document {
    let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
    let doc = Document::load(text_pdf("Flat", &lines)).unwrap();
    let text = |y: f32, name: &str| FieldSpec {
        page: 0,
        rect: [100.0, y, 250.0, y + 20.0],
        name: name.into(),
        kind: NewField::Text {
            multiline: false,
            password: false,
            max_len: None,
            comb: false,
        },
        tooltip: String::new(),
        required: false,
        read_only: false,
        value: None,
        scripts: katna_pdf_cos::Scripts::default(),
        look: katna_pdf_cos::Look::default(),
    };
    let doc = Document::load(
        doc.design_form(&FormDesign {
            add: vec![text(700.0, "Name"), text(650.0, "City")],
            ..FormDesign::default()
        })
        .unwrap(),
    )
    .unwrap();
    let form = doc.form();
    let doc = Document::load(
        doc.fill(&[(form[0].id, FieldValue::Text("Mozammel".into()))])
            .unwrap(),
    )
    .unwrap();
    let mut note = Annotation::new(0, AnnotKind::Square { fill: None });
    note.rect = [300.0, 700.0, 320.0, 720.0];
    Document::load(
        doc.annotate(&AnnotChanges {
            put: vec![note],
            remove: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap()
}

fn page_content(doc: &Document) -> String {
    let page = doc.get(doc.pages()[0]).unwrap();
    let contents = page.as_dict().unwrap().get("Contents").unwrap();
    let streams: Vec<Object> = match doc.resolve(contents) {
        Object::Array(a) => a.iter().map(|o| doc.resolve(o)).collect(),
        other => vec![other],
    };
    let mut out = String::new();
    for s in streams {
        let Object::Stream(s) = s else { panic!() };
        out.push_str(&String::from_utf8_lossy(
            &katna_pdf_cos::decode(&s.dict, &s.data).unwrap(),
        ));
        out.push('\n');
    }
    out
}

#[test]
fn fields_are_drawn_in_and_the_form_taken_out() {
    let doc = with_form_and_note();
    assert_eq!(doc.form().len(), 2);
    let count = |d: &Document| d.annotations().iter().map(Vec::len).sum::<usize>();
    let comments = count(&doc);
    assert!(comments >= 1);
    let flat = Document::load(
        doc.flatten(Flatten {
            forms: true,
            comments: false,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(flat.form().is_empty());
    assert!(flat.catalog().unwrap().get("AcroForm").is_none());
    // The square stays; the two fields are drawn by the page.
    assert_eq!(count(&flat), comments);
    let content = page_content(&flat);
    assert_eq!(content.matches(" Do").count(), 2, "{content}");
    assert!(content.contains("cm /"), "{content}");
    // The drawn field still shows the name.
    let page = flat.get(flat.pages()[0]).unwrap();
    let resources = flat.resolve(page.as_dict().unwrap().get("Resources").unwrap());
    let Object::Dict(resources) = resources else {
        panic!()
    };
    let Object::Dict(xobjects) = flat.resolve_key(&resources, "XObject") else {
        panic!()
    };
    let drawn: Vec<String> = xobjects
        .iter()
        .map(|(_, x)| match flat.resolve(x) {
            Object::Stream(s) => {
                String::from_utf8_lossy(&katna_pdf_cos::decode(&s.dict, &s.data).unwrap())
                    .into_owned()
            }
            _ => String::new(),
        })
        .collect();
    assert!(
        drawn.iter().any(|d| d.contains("(Mozammel) Tj")),
        "{drawn:?}"
    );
    // Comments too, and then nothing is left.
    let all = Document::load(
        doc.flatten(Flatten {
            forms: true,
            comments: true,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(count(&all), 0);
    assert!(all.form().is_empty());
    assert_eq!(page_content(&all).matches(" Do").count(), 2 + comments);
    let page = all.get(all.pages()[0]).unwrap();
    assert!(page.as_dict().unwrap().get("Annots").is_none());
}
