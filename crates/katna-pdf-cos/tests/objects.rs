// SPDX-License-Identifier: GPL-3.0-or-later

//! Picking, moving and deleting the pictures and shapes a page draws.

use katna_pdf_cos::{Document, Image, Insert, ObjectKind, Picture, ShapeKind, text_pdf};

/// A page with a picture and a rectangle put in, after its text.
fn page() -> Document {
    let doc = Document::load(text_pdf("Note", &["  Hello".to_string()])).unwrap();
    let out = doc
        .insert(
            0,
            &[
                Insert::Image {
                    rect: [100.0, 200.0, 200.0, 250.0],
                    image: Image::Pixels(Picture {
                        width: 2,
                        height: 1,
                        rgb: vec![255, 0, 0, 0, 0, 255],
                        alpha: None,
                    }),
                },
                Insert::Shape {
                    kind: ShapeKind::Rectangle,
                    rect: [300.0, 300.0, 340.0, 320.0],
                    stroke: Some([0.0; 3]),
                    fill: None,
                    width: 1.0,
                },
            ],
        )
        .unwrap();
    Document::load(out).unwrap()
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

#[test]
fn pictures_and_shapes_are_listed_where_they_are() {
    let doc = page();
    let objects = doc.page_objects(0);
    let kinds: Vec<ObjectKind> = objects.iter().map(|o| o.kind).collect();
    assert!(
        kinds.ends_with(&[ObjectKind::Picture, ObjectKind::Path]),
        "{objects:?}"
    );
    let n = objects.len();
    assert!(
        close(objects[n - 2].rect, [100.0, 200.0, 200.0, 250.0]),
        "{objects:?}"
    );
    assert!(
        close(objects[n - 1].rect, [300.0, 300.0, 340.0, 320.0]),
        "{objects:?}"
    );
}

#[test]
fn objects_move_resize_and_go() {
    let doc = page();
    let n = doc.page_objects(0).len();
    // The picture twice as wide, its left edge where it was, and 50 up.
    let m = [2.0, 0.0, 0.0, 1.0, -100.0, 50.0];
    let moved = Document::load(doc.transform_object(0, n - 2, m).unwrap()).unwrap();
    let objects = moved.page_objects(0);
    assert_eq!(objects.len(), n);
    assert!(
        close(objects[n - 2].rect, [100.0, 250.0, 300.0, 300.0]),
        "{objects:?}"
    );
    assert!(
        close(objects[n - 1].rect, [300.0, 300.0, 340.0, 320.0]),
        "{objects:?}"
    );
    // The text is where it was.
    assert_eq!(moved.text_runs(0), doc.text_runs(0));

    let gone = Document::load(moved.delete_object(0, n - 1).unwrap()).unwrap();
    let objects = gone.page_objects(0);
    assert_eq!(objects.len(), n - 1);
    assert_eq!(objects[n - 2].kind, ObjectKind::Picture);
    assert!(gone.bytes().starts_with(moved.bytes()));
}

/// The page's content, decoded.
fn content(doc: &Document) -> String {
    let page = doc.get(doc.pages()[0]).unwrap();
    let r = page.as_dict().unwrap().get("Contents").unwrap().clone();
    let parts = match doc.resolve(&r) {
        katna_pdf_cos::Object::Array(a) => a,
        o => vec![o],
    };
    let mut out = String::new();
    for p in parts {
        if let katna_pdf_cos::Object::Stream(s) = doc.resolve(&p) {
            out.push_str(&String::from_utf8_lossy(
                &katna_pdf_cos::decode(&s.dict, &s.data).unwrap(),
            ));
        }
    }
    out
}

#[test]
fn objects_restack_fade_recolour_and_change_picture() {
    use katna_pdf_cos::ObjectChange;
    let doc = page();
    let before = doc.page_objects(0);
    let n = before.len();
    let changed = |doc: &Document, index: usize, change: ObjectChange| {
        Document::load(doc.change_object(0, index, &change).unwrap()).unwrap()
    };

    // The picture to the back: listed first, in the same place.
    let back = changed(&doc, n - 2, ObjectChange::Back);
    let objects = back.page_objects(0);
    assert_eq!(objects.len(), n);
    assert_eq!(objects[0].kind, ObjectKind::Picture);
    assert!(close(objects[0].rect, before[n - 2].rect), "{objects:?}");
    assert!(close(objects[n - 1].rect, before[n - 1].rect));
    assert_eq!(back.text_runs(0), doc.text_runs(0));
    // And to the front again: last.
    let front = changed(&back, 0, ObjectChange::Front);
    let objects = front.page_objects(0);
    assert_eq!(objects[n - 1].kind, ObjectKind::Picture);
    assert!(
        close(objects[n - 1].rect, before[n - 2].rect),
        "{objects:?}"
    );
    assert_eq!(front.text_runs(0), doc.text_runs(0));

    // See-through, and a new colour: the same place, drawn so.
    let faded = changed(&doc, n - 1, ObjectChange::Opacity(0.5));
    assert!(content(&faded).contains(" gs "), "{}", content(&faded));
    assert!(close(faded.page_objects(0)[n - 1].rect, before[n - 1].rect));
    let green = changed(
        &doc,
        n - 1,
        ObjectChange::Colours {
            fill: None,
            stroke: Some([0.0, 1.0, 0.0]),
        },
    );
    assert!(
        content(&green).contains("q 0 1 0 RG"),
        "{}",
        content(&green)
    );
    // Changed again, the same wrapper changes: one colour, one state.
    let faded_green = changed(&green, n - 1, ObjectChange::Opacity(0.25));
    let again = changed(&faded_green, n - 1, ObjectChange::Opacity(1.0));
    let red = changed(
        &again,
        n - 1,
        ObjectChange::Colours {
            fill: None,
            stroke: Some([1.0, 0.0, 0.0]),
        },
    );
    let text = content(&red);
    assert_eq!(text.matches(" gs ").count(), 1, "{text}");
    assert!(
        text.contains("gs 1 0 0 RG 300 300 40 20 re S Q Q"),
        "{text}"
    );
    assert!(!text.contains("0 1 0 RG"), "{text}");
    assert!(close(red.page_objects(0)[n - 1].rect, before[n - 1].rect));

    // Another picture in the same box.
    let other = Image::Pixels(Picture {
        width: 1,
        height: 1,
        rgb: vec![0, 255, 0],
        alpha: None,
    });
    let swapped = changed(&doc, n - 2, ObjectChange::Picture(other));
    let objects = swapped.page_objects(0);
    assert_eq!(objects[n - 2].kind, ObjectKind::Picture);
    assert!(close(objects[n - 2].rect, before[n - 2].rect));
    assert!(
        doc.change_object(
            0,
            n - 1,
            &ObjectChange::Picture(Image::Pixels(Picture {
                width: 1,
                height: 1,
                rgb: vec![0, 0, 0],
                alpha: None,
            }))
        )
        .is_err()
    );
}

#[test]
fn objects_step_forward_and_back_and_change_together() {
    use katna_pdf_cos::ObjectChange;
    let doc = page();
    let before = doc.page_objects(0);
    let n = before.len();
    // The picture one step forward: over the rectangle, both where they
    // were.
    let forward =
        Document::load(doc.change_object(0, n - 2, &ObjectChange::Forward).unwrap()).unwrap();
    let objects = forward.page_objects(0);
    assert_eq!(objects.len(), n);
    assert_eq!(objects[n - 1].kind, ObjectKind::Picture);
    assert!(
        close(objects[n - 1].rect, before[n - 2].rect),
        "{objects:?}"
    );
    assert_eq!(objects[n - 2].kind, ObjectKind::Path);
    assert!(
        close(objects[n - 2].rect, before[n - 1].rect),
        "{objects:?}"
    );
    assert_eq!(forward.text_runs(0), doc.text_runs(0));
    // And one step back again: as it was.
    let back = Document::load(
        forward
            .change_object(0, n - 1, &ObjectChange::Backward)
            .unwrap(),
    )
    .unwrap();
    let objects = back.page_objects(0);
    assert_eq!(objects[n - 2].kind, ObjectKind::Picture);
    assert!(
        close(objects[n - 2].rect, before[n - 2].rect),
        "{objects:?}"
    );
    assert!(
        close(objects[n - 1].rect, before[n - 1].rect),
        "{objects:?}"
    );
    // Forward from the top, or back from the bottom: nothing changes.
    let same =
        Document::load(doc.change_object(0, n - 1, &ObjectChange::Forward).unwrap()).unwrap();
    assert_eq!(same.page_objects(0), before);

    // Both moved up by 10 in one change; then both taken out.
    let moved = Document::load(
        doc.transform_objects(
            0,
            &[
                (n - 2, [1.0, 0.0, 0.0, 1.0, 0.0, 10.0]),
                (n - 1, [1.0, 0.0, 0.0, 1.0, 0.0, 10.0]),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    let objects = moved.page_objects(0);
    assert!(
        close(objects[n - 2].rect, [100.0, 210.0, 200.0, 260.0]),
        "{objects:?}"
    );
    assert!(
        close(objects[n - 1].rect, [300.0, 310.0, 340.0, 330.0]),
        "{objects:?}"
    );
    let gone = Document::load(moved.delete_objects(0, &[n - 1, n - 2]).unwrap()).unwrap();
    assert_eq!(gone.page_objects(0).len(), n - 2);
    assert_eq!(gone.text_runs(0), doc.text_runs(0));
}

#[test]
fn objects_copy_and_paste_here_and_elsewhere() {
    let doc = page();
    let before = doc.page_objects(0);
    let n = before.len();
    let picture = doc.copy_object(0, n - 2).unwrap();
    let shape = doc.copy_object(0, n - 1).unwrap();
    assert_eq!(picture.kind, ObjectKind::Picture);
    assert!(close(picture.rect, before[n - 2].rect));

    // Pasted on the same page, moved along: two more objects, the
    // picture's stream used again rather than written twice.
    let pasted = Document::load(
        doc.paste_objects(0, &[&picture, &shape], [20.0, -30.0])
            .unwrap(),
    )
    .unwrap();
    let objects = pasted.page_objects(0);
    assert_eq!(objects.len(), n + 2, "{objects:?}");
    assert_eq!(objects[n].kind, ObjectKind::Picture);
    assert!(
        close(objects[n].rect, [120.0, 170.0, 220.0, 220.0]),
        "{objects:?}"
    );
    assert_eq!(objects[n + 1].kind, ObjectKind::Path);
    assert!(
        close(objects[n + 1].rect, [320.0, 270.0, 360.0, 290.0]),
        "{objects:?}"
    );
    let images = pasted
        .object_numbers()
        .filter(|&num| {
            matches!(
                pasted.get(katna_pdf_cos::Ref::new(num, 0)),
                Some(katna_pdf_cos::Object::Stream(s)) if s.dict.name("Subtype") == Some(b"Image")
            )
        })
        .count();
    assert_eq!(images, 1);
    assert_eq!(pasted.text_runs(0), doc.text_runs(0));

    // Pasted into another file, where it is drawn in the same place with
    // its picture brought along.
    let other = Document::load(text_pdf("Other", &["  Elsewhere".to_string()])).unwrap();
    assert!(other.page_objects(0).is_empty());
    let there = Document::load(other.paste_objects(0, &[&picture], [0.0, 0.0]).unwrap()).unwrap();
    let objects = there.page_objects(0);
    assert_eq!(objects.len(), 1, "{objects:?}");
    assert_eq!(objects[0].kind, ObjectKind::Picture);
    assert!(close(objects[0].rect, before[n - 2].rect), "{objects:?}");
    let images = there
        .object_numbers()
        .filter(|&num| {
            matches!(
                there.get(katna_pdf_cos::Ref::new(num, 0)),
                Some(katna_pdf_cos::Object::Stream(s)) if s.dict.name("Subtype") == Some(b"Image")
            )
        })
        .count();
    assert_eq!(images, 1);
}
