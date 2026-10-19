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
