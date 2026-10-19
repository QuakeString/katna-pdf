// SPDX-License-Identifier: GPL-3.0-or-later

//! Annotations: read, added, changed and removed.

use std::io::Write as _;

use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, Document, MarkupStyle, Object, stamp_label,
};

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

/// Two pages; the first has a link, a note with a pop-up and a caret,
/// its list of annotations an object of its own.
fn two() -> Document {
    Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] /MediaBox [0 0 600 800] >>",
        "<< /Type /Page /Parent 2 0 R /Annots 5 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "[6 0 R 7 0 R 8 0 R 9 0 R]",
        "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>",
        "<< /Type /Annot /Subtype /Text /Rect [100 700 120 720] /C [0 0 1] \
           /Contents (Look here) /T (Asha) /M (D:20260101000000Z) /Popup 8 0 R >>",
        "<< /Type /Annot /Subtype /Popup /Rect [120 600 300 720] /Parent 7 0 R >>",
        "<< /Type /Annot /Subtype /Caret /Rect [50 50 60 60] /IRT 7 0 R \
           /Contents <FEFF0928092E0938094D09240947> >>",
    ]))
    .unwrap()
}

#[test]
fn annotations_are_read() {
    let doc = two();
    let all = doc.annotations();
    assert_eq!(all.len(), 2);
    assert!(all[1].is_empty());
    let [note, caret] = all[0].as_slice() else {
        panic!("{:?}", all[0]);
    };
    assert_eq!(note.kind, AnnotKind::Note);
    assert_eq!(note.rect, [100.0, 700.0, 120.0, 720.0]);
    assert_eq!(note.colour, [0.0, 0.0, 1.0]);
    assert_eq!(note.contents, "Look here");
    assert_eq!(note.author, "Asha");
    assert_eq!(note.modified.as_deref(), Some("D:20260101000000Z"));
    assert_eq!(caret.kind, AnnotKind::Other("Caret".into()));
    assert_eq!(caret.contents, "नमस्ते");
    assert_eq!(caret.reply_to, note.id);
}

#[test]
fn every_kind_is_written_and_read_back() {
    let doc = two();
    let quad = [100.0, 520.0, 300.0, 520.0, 100.0, 500.0, 300.0, 500.0];
    let mut put = vec![
        Annotation::new(1, AnnotKind::Markup(MarkupStyle::Highlight, vec![quad])),
        Annotation::new(1, AnnotKind::Markup(MarkupStyle::Underline, vec![quad])),
        Annotation::new(1, AnnotKind::Markup(MarkupStyle::StrikeOut, vec![quad])),
        Annotation::new(1, AnnotKind::Markup(MarkupStyle::Squiggly, vec![quad])),
        Annotation::new(
            1,
            AnnotKind::Ink(vec![
                vec![[10.0, 10.0], [20.0, 30.0], [40.0, 15.0]],
                vec![[50.0, 50.0]],
            ]),
        ),
        Annotation::new(
            1,
            AnnotKind::Line {
                from: [10.0, 100.0],
                to: [200.0, 150.0],
                arrow: true,
            },
        ),
        Annotation::new(
            1,
            AnnotKind::Polygon(vec![[0.0, 0.0], [50.0, 0.0], [25.0, 40.0]]),
        ),
        Annotation::new(
            1,
            AnnotKind::PolyLine(vec![[0.0, 0.0], [50.0, 0.0], [25.0, 40.0]]),
        ),
    ];
    for (kind, rect) in [
        (AnnotKind::Note, [300.0, 300.0, 320.0, 320.0]),
        (
            AnnotKind::FreeText { size: 14.0 },
            [50.0, 600.0, 250.0, 650.0],
        ),
        (
            AnnotKind::Square {
                fill: Some([0.0, 1.0, 0.0]),
            },
            [10.0, 10.0, 60.0, 40.0],
        ),
        (AnnotKind::Circle { fill: None }, [10.0, 10.0, 60.0, 40.0]),
        (
            AnnotKind::Stamp("Approved".into()),
            [400.0, 700.0, 550.0, 750.0],
        ),
    ] {
        let mut a = Annotation::new(1, kind);
        a.rect = rect;
        put.push(a);
    }
    for a in &mut put {
        a.contents = "A comment".into();
        a.author = "Mozammel".into();
        a.width = 2.0;
        a.colour = [1.0, 0.0, 0.0];
        a.opacity = 0.5;
    }
    let out = doc
        .annotate(&AnnotChanges {
            put: put.clone(),
            remove: vec![],
        })
        .unwrap();
    // An incremental update: the old file is still at the front.
    assert!(out.starts_with(doc.bytes()));
    let again = Document::load(out).unwrap();
    assert!(!again.was_repaired());
    let read = &again.annotations()[1];
    assert_eq!(read.len(), put.len());
    for (got, want) in read.iter().zip(&put) {
        assert_eq!(got.kind, want.kind);
        assert_eq!(got.contents, "A comment");
        assert_eq!(got.author, "Mozammel");
        assert_eq!(got.colour, [1.0, 0.0, 0.0]);
        assert_eq!(got.opacity, 0.5);
        assert!(
            got.modified
                .as_deref()
                .is_some_and(|m| m.starts_with("D:20"))
        );
        // Each has its own appearance, drawn in its rectangle.
        let Some(Object::Dict(dict)) = again.get(got.id.unwrap()) else {
            panic!();
        };
        let Object::Dict(ap) = again.resolve_key(&dict, "AP") else {
            panic!("no appearance for {:?}", got.kind);
        };
        let Object::Stream(n) = again.resolve_key(&ap, "N") else {
            panic!();
        };
        let bbox: Vec<f32> = n
            .dict
            .get("BBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_number().unwrap() as f32)
            .collect();
        assert_eq!(bbox, got.rect);
        assert!(!n.data.is_empty());
    }
    // Rectangles reach round the points, and the arrowhead.
    assert_eq!(read[0].rect, [99.0, 499.0, 301.0, 521.0]);
    assert!(read[5].rect[2] > 200.0 + 2.0);
}

#[test]
fn annotations_are_changed_and_removed() {
    let doc = two();
    let all = doc.annotations();
    let mut note = all[0][0].clone();
    let caret = all[0][1].clone();
    // A changed comment keeps what the note had.
    note.contents = "Changed".into();
    let out = doc
        .annotate(&AnnotChanges {
            put: vec![note.clone()],
            remove: vec![],
        })
        .unwrap();
    let again = Document::load(out).unwrap();
    let Some(Object::Dict(dict)) = again.get(note.id.unwrap()) else {
        panic!();
    };
    assert!(
        dict.get("AP").is_some(),
        "the note had no look and gets one"
    );
    assert!(dict.get("Popup").is_some(), "what it had is kept");
    assert_eq!(again.annotations()[0][0].contents, "Changed");

    // Removing the note takes its pop-up off the page too; the link stays.
    let out = doc
        .annotate(&AnnotChanges {
            put: vec![],
            remove: vec![note.id.unwrap(), caret.id.unwrap()],
        })
        .unwrap();
    let again = Document::load(out).unwrap();
    assert!(again.annotations()[0].is_empty());
    let Some(Object::Array(list)) = again.get(katna_pdf_cos::Ref::new(5, 0)) else {
        panic!();
    };
    assert_eq!(list, vec![Object::Ref(katna_pdf_cos::Ref::new(6, 0))]);
}

#[test]
fn stamps_read_as_words() {
    assert_eq!(stamp_label("ForPublicRelease"), "FOR PUBLIC RELEASE");
}

#[test]
fn annotations_go_to_xfdf_and_back() {
    let doc = two();
    let mut ink = Annotation::new(
        1,
        AnnotKind::Ink(vec![vec![[10.0, 10.0], [20.5, 30.25]], vec![[5.0, 5.0]]]),
    );
    ink.colour = [1.0, 0.0, 0.0];
    ink.width = 3.0;
    ink.contents = "Says \"hi\" & <bye>\nline two".into();
    ink.author = "Asha".into();
    let mut line = Annotation::new(
        1,
        AnnotKind::Line {
            from: [1.0, 2.0],
            to: [3.0, 4.0],
            arrow: true,
        },
    );
    line.opacity = 0.5;
    let mut text = Annotation::new(1, AnnotKind::FreeText { size: 14.0 });
    text.rect = [10.0, 10.0, 200.0, 50.0];
    text.contents = "नमस्ते".into();
    let out = doc
        .annotate(&AnnotChanges {
            put: vec![ink, line, text],
            remove: vec![],
        })
        .unwrap();
    let doc = Document::load(out).unwrap();
    let all = doc.annotations();
    let xml = katna_pdf_cos::to_xfdf(&all, "two.pdf");
    assert!(xml.contains("<ink page=\"1\""));
    let back = katna_pdf_cos::from_xfdf(&xml).unwrap();
    // The note, its caret reply (not ours: left out), the three added.
    assert_eq!(back.len(), 4);
    let want: Vec<_> = all
        .iter()
        .flatten()
        .filter(|a| !matches!(a.kind, AnnotKind::Other(_)))
        .collect();
    for (got, want) in back.iter().zip(want) {
        let got = &got.annotation;
        assert_eq!(got.kind, want.kind);
        assert_eq!(got.page, want.page);
        assert_eq!(got.rect, want.rect);
        // Colours go through XFDF as bytes.
        for (g, w) in got.colour.iter().zip(want.colour) {
            assert!(
                (g - w).abs() <= 0.5 / 255.0,
                "{:?} {:?}",
                got.colour,
                want.colour
            );
        }
        assert_eq!(got.opacity, want.opacity);
        assert_eq!(got.contents, want.contents);
        assert_eq!(got.author, want.author);
        assert_eq!(got.name, want.name);
        assert_eq!(got.modified, want.modified);
    }
    // Read into the same file, each one with a name replaces itself (the
    // note had none); into another, each is added.
    let changes = doc.xfdf_changes(&xml).unwrap();
    let replaced = |c: &AnnotChanges| c.put.iter().filter(|a| a.id.is_some()).count();
    assert_eq!(replaced(&changes), 3);
    assert_eq!(replaced(&two().xfdf_changes(&xml).unwrap()), 0);
    assert!(katna_pdf_cos::from_xfdf("<html/>").is_err());
}

#[test]
fn measurements_keep_their_scale() {
    let doc = Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] \
           /VP [<< /Type /Viewport /BBox [0 0 600 800] /Measure << /Type /Measure \
           /Subtype /RL /R (1 in = 10 ft) /X [<< /U (ft) /C 0.13889 >>] >> >>] >>",
    ]))
    .unwrap();
    let scale = doc.page_scale(0).unwrap();
    assert_eq!(scale.unit, "ft");
    assert_eq!(scale.length(72.0), "10 ft");
    let mut a = Annotation::new(
        0,
        AnnotKind::Line {
            from: [100.0, 100.0],
            to: [172.0, 100.0],
            arrow: false,
        },
    );
    a.contents = scale.length(72.0);
    a.measure = Some(scale.clone());
    a.rect = a.bounds();
    // The value's box is in the annotation's.
    assert!(a.rect[3] > 110.0);
    let out = doc
        .annotate(&AnnotChanges {
            put: vec![a],
            remove: vec![],
        })
        .unwrap();
    let again = Document::load(out).unwrap();
    let read = &again.annotations()[0][0];
    assert_eq!(read.measure.as_ref().map(|s| s.unit.as_str()), Some("ft"));
    assert_eq!(read.contents, "10 ft");
    let Some(Object::Dict(dict)) = again.get(read.id.unwrap()) else {
        panic!();
    };
    assert_eq!(dict.name("IT"), Some(&b"LineDimension"[..]));
}
