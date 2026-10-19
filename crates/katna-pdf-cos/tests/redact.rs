// SPDX-License-Identifier: GPL-3.0-or-later

//! Redacting: what lies under an area leaves the file, not only the page.

use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, Document, Image, Insert, Object, Pattern, Permissions,
    Picture, RedactOptions, Stream, decode, text_pdf,
};

fn letter() -> Document {
    Document::load(text_pdf(
        "Letter",
        &[
            "  Name: Asha Rao".to_string(),
            "  PAN: ABCDE1234F, Aadhaar 2345 6789 0123".to_string(),
            "  Call +91 98765 43210 or write to asha@example.in".to_string(),
        ],
    ))
    .unwrap()
}

fn text(doc: &Document) -> String {
    doc.text_runs(0)
        .iter()
        .map(|r| r.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Everything the file holds, its streams decoded.
fn everything(doc: &Document) -> Vec<u8> {
    let mut out = doc.bytes().to_vec();
    for num in doc.object_numbers() {
        let Some(entry) = doc.entry(num) else {
            continue;
        };
        let generation = match entry {
            katna_pdf_cos::Entry::InFile { generation, .. } => generation,
            _ => 0,
        };
        if let Some(Object::Stream(s)) = doc.get(katna_pdf_cos::Ref::new(num, generation))
            && let Ok(data) = decode(&s.dict, &s.data)
        {
            out.extend(data);
        }
    }
    out
}

fn holds(doc: &Document, what: &str) -> bool {
    everything(doc)
        .windows(what.len())
        .any(|w| w == what.as_bytes())
}

#[test]
fn found_numbers_leave_the_text_and_the_rest_stays_put() {
    let doc = letter();
    let before = doc.text_runs(0);
    let found = doc.find_redactions(0, &[Pattern::Pan, Pattern::Aadhaar]);
    assert_eq!(found.len(), 2, "{found:?}");
    let areas: Vec<(usize, [f32; 4])> = found.iter().map(|r| (0, *r)).collect();
    let out = Document::load(doc.redact(&areas, &RedactOptions::default()).unwrap()).unwrap();
    let after = text(&out);
    assert!(!after.contains("ABCDE"), "{after}");
    assert!(!after.contains("6789"), "{after}");
    assert!(after.contains("PAN:"), "{after}");
    assert!(after.contains("Aadhaar"), "{after}");
    assert!(after.contains("Name: Asha Rao"), "{after}");
    assert!(!holds(&out, "ABCDE") && !holds(&out, "1234F"));
    // Text after a cut starts where it did.
    let runs = out.text_runs(0);
    let call = runs.iter().find(|r| r.text.starts_with("Call")).unwrap();
    let was = before.iter().find(|r| r.text.starts_with("Call")).unwrap();
    assert_eq!(call.rect, was.rect);
    // A box is drawn over each area.
    assert!(holds(&out, " re\n"));
}

#[test]
fn phones_and_emails_are_found() {
    let doc = letter();
    let found = doc.find_redactions(0, &[Pattern::Phone, Pattern::Email]);
    assert_eq!(found.len(), 2, "{found:?}");
    let words = doc.find_redactions(0, &[Pattern::Words("asha rao".into())]);
    assert_eq!(words.len(), 1);
    // A date or an amount is not a phone number.
    let other = Document::load(text_pdf(
        "Bill",
        &["  Due 2026-10-19: Rs 1,25,000 for 3 items".to_string()],
    ))
    .unwrap();
    assert!(other.find_redactions(0, &[Pattern::Phone]).is_empty());
}

#[test]
fn older_revisions_do_not_keep_what_was_taken_out() {
    let doc = letter();
    // An edit leaves the old text in the file's earlier revision.
    let runs = doc.text_runs(0);
    let name = runs
        .iter()
        .position(|r| r.text.contains("Asha Rao"))
        .unwrap();
    let edited =
        Document::load(doc.edit_text(0, &[(name, "Name: Asha R.".into())]).unwrap()).unwrap();
    assert!(holds(&edited, "Rao"));
    let area = edited
        .find_redactions(0, &[Pattern::Words("Asha R.".into())])
        .remove(0);
    let out = Document::load(
        edited
            .redact(
                &[(0, area)],
                &RedactOptions {
                    fill: None,
                    scrub: true,
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert!(!holds(&out, "Asha"), "{}", text(&out));
    assert!(!holds(&out, "Rao"));
    assert!(text(&out).contains("Name:"));
    // Scrubbed: no title left.
    assert_eq!(out.metadata().title, None);
}

#[test]
fn invisible_text_is_taken_out_too() {
    let doc = letter();
    let mut update = doc.update().unwrap();
    let content = update.add(Object::Stream(Stream {
        dict: katna_pdf_cos::Dict::new(),
        data: b"BT 3 Tr /F1 12 Tf 72 400 Td (Hidden secret) Tj ET".to_vec(),
    }));
    let page = doc.pages()[0];
    let mut dict = doc.get(page).unwrap().as_dict().unwrap().clone();
    let old = dict.get("Contents").cloned().unwrap();
    dict.set("Contents", Object::Array(vec![old, Object::Ref(content)]));
    update.replace(page, Object::Dict(dict));
    let doc = Document::load(update.finish()).unwrap();
    let found = doc.find_redactions(0, &[Pattern::Words("secret".into())]);
    assert_eq!(found.len(), 1);
    let out = Document::load(
        doc.redact(&[(0, found[0])], &RedactOptions::default())
            .unwrap(),
    )
    .unwrap();
    assert!(!holds(&out, "secret"));
    assert!(holds(&out, "Hidden"));
}

#[test]
fn pixels_under_an_area_are_blacked_out() {
    let doc = letter();
    let white = Image::Pixels(Picture {
        width: 10,
        height: 10,
        rgb: vec![255; 300],
        alpha: None,
    });
    let with = Document::load(
        doc.insert(
            0,
            &[Insert::Image {
                rect: [100.0, 100.0, 200.0, 200.0],
                image: white,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let out = Document::load(
        with.redact(
            &[(0, [90.0, 90.0, 150.0, 210.0])],
            &RedactOptions::default(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut found = false;
    for num in out.object_numbers() {
        let Some(Object::Stream(s)) = out.get(katna_pdf_cos::Ref::new(num, 0)) else {
            continue;
        };
        if s.dict.name("Subtype") != Some(b"Image") {
            continue;
        }
        let pixels = decode(&s.dict, &s.data).unwrap();
        for y in 0..10 {
            for x in 0..10 {
                let v = pixels[(y * 10 + x) * 3];
                assert_eq!(v, if x < 5 { 0 } else { 255 }, "pixel {x},{y}");
            }
        }
        found = true;
    }
    assert!(found);
}

#[test]
fn annotations_over_an_area_go() {
    let doc = letter();
    let mut note = Annotation::new(0, AnnotKind::Note);
    note.rect = [300.0, 300.0, 320.0, 320.0];
    note.contents = "Her PAN is on file".into();
    let mut far = Annotation::new(0, AnnotKind::Note);
    far.rect = [500.0, 50.0, 520.0, 70.0];
    let doc = Document::load(
        doc.annotate(&AnnotChanges {
            put: vec![note, far],
            remove: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(doc.annotations()[0].len(), 2);
    let out = Document::load(
        doc.redact(
            &[(0, [290.0, 290.0, 330.0, 330.0])],
            &RedactOptions::default(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(out.annotations()[0].len(), 1);
    assert!(!holds(&out, "on file"));
}

#[test]
fn a_locked_file_stays_locked() {
    let locked = letter().lock("asha", "boss", Permissions::ALL).unwrap();
    let mut doc = Document::load(locked).unwrap();
    assert!(doc.redact(&[], &RedactOptions::default()).is_err());
    doc.unlock("asha").unwrap();
    let area = doc.find_redactions(0, &[Pattern::Pan]).remove(0);
    let out = doc.redact(&[(0, area)], &RedactOptions::default()).unwrap();
    assert!(!out.windows(4).any(|w| w == b"Name"));
    let mut again = Document::load(out).unwrap();
    assert!(again.is_locked());
    again.unlock("asha").unwrap();
    assert!(!text(&again).contains("ABCDE"));
    assert!(text(&again).contains("Asha Rao"));
}
