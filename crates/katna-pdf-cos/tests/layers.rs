// SPDX-License-Identifier: GPL-3.0-or-later

//! Layers and files kept in the document.

use katna_pdf_cos::{Document, Insert, ShapeKind, text_pdf};

fn page() -> Document {
    let doc = Document::load(text_pdf("Note", &["  Hello".to_string()])).unwrap();
    let out = doc
        .insert(
            0,
            &[Insert::Shape {
                kind: ShapeKind::Rectangle,
                rect: [300.0, 300.0, 340.0, 320.0],
                stroke: Some([0.0; 3]),
                fill: None,
                width: 1.0,
            }],
        )
        .unwrap();
    Document::load(out).unwrap()
}

#[test]
fn layers_are_added_objects_put_on_them_and_hidden() {
    let doc = page();
    assert!(doc.layers().is_empty());
    let (bytes, notes) = doc.add_layer("Notes").unwrap();
    let doc = Document::load(bytes).unwrap();
    let (bytes, draft) = doc.add_layer("Draft").unwrap();
    let doc = Document::load(bytes).unwrap();
    let layers = doc.layers();
    assert_eq!(
        layers
            .iter()
            .map(|l| (l.name.as_str(), l.on))
            .collect::<Vec<_>>(),
        [("Notes", true), ("Draft", true)]
    );
    assert_eq!(layers[0].id, notes);

    // The rectangle goes on Draft.
    let n = doc.page_objects(0).len();
    let doc = Document::load(doc.put_on_layer(0, &[n - 1], Some(draft)).unwrap()).unwrap();
    let objects = doc.page_objects(0);
    assert_eq!(objects.len(), n);
    assert_eq!(objects[n - 1].layer, Some(draft));
    let text = String::from_utf8_lossy(&doc.page_content_bytes(0).unwrap()).to_string();
    assert!(text.contains("/OC /oc1 BDC"), "{text}");
    assert!(text.contains("EMC"), "{text}");

    // Moved, it stays on the layer, with one wrapper.
    let doc = Document::load(
        doc.transform_object(0, n - 1, [1.0, 0.0, 0.0, 1.0, 10.0, 0.0])
            .unwrap(),
    )
    .unwrap();
    let doc = Document::load(doc.put_on_layer(0, &[n - 1], Some(notes)).unwrap()).unwrap();
    let objects = doc.page_objects(0);
    assert_eq!(objects.len(), n);
    assert_eq!(objects[n - 1].layer, Some(notes));
    let text = String::from_utf8_lossy(&doc.page_content_bytes(0).unwrap()).to_string();
    assert_eq!(text.matches("BDC").count(), 1, "{text}");

    // Hidden, then taken off the layer.
    let doc = Document::load(doc.show_layers(&[(notes, false)]).unwrap()).unwrap();
    assert_eq!(
        doc.layers().iter().map(|l| l.on).collect::<Vec<_>>(),
        [false, true]
    );
    let doc = Document::load(doc.put_on_layer(0, &[n - 1], None).unwrap()).unwrap();
    assert_eq!(doc.page_objects(0)[n - 1].layer, None);
    let doc = Document::load(doc.rename_layer(notes, "Remarks").unwrap()).unwrap();
    assert_eq!(doc.layers()[0].name, "Remarks");
    let doc = Document::load(doc.remove_layer(draft).unwrap()).unwrap();
    assert_eq!(doc.layers().len(), 1);
}

#[test]
fn files_are_kept_in_the_document_renamed_and_taken_out() {
    let doc = page();
    assert!(doc.attachments().is_empty());
    let files = vec![
        (
            "notes.txt".to_string(),
            b"hello".to_vec(),
            "Notes".to_string(),
        ),
        (
            "data.csv".to_string(),
            b"a,b\n1,2\n".to_vec(),
            String::new(),
        ),
    ];
    let doc = Document::load(doc.attach_files(&files).unwrap()).unwrap();
    let kept = doc.attachments();
    assert_eq!(
        kept.iter()
            .map(|a| (a.name.as_str(), a.description.as_str(), a.size))
            .collect::<Vec<_>>(),
        [("data.csv", "", Some(8)), ("notes.txt", "Notes", Some(5))]
    );
    assert_eq!(doc.attachment_data(&kept[1]).unwrap(), b"hello");

    let doc = Document::load(
        doc.rename_attachment(&kept[1], "readme.txt", "Read me")
            .unwrap(),
    )
    .unwrap();
    let kept = doc.attachments();
    assert_eq!(kept[1].name, "readme.txt");
    assert_eq!(kept[1].description, "Read me");
    assert_eq!(doc.attachment_data(&kept[1]).unwrap(), b"hello");

    let doc = Document::load(doc.remove_attachments(&["data.csv".to_string()]).unwrap()).unwrap();
    let kept = doc.attachments();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "readme.txt");
}
