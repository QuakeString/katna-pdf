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
        content(&green).contains("q 1 w 0 1 0 RG"),
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
        text.contains("gs 1 0 0 RG 300 300 40 20 re S Q"),
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

/// One page drawing `content` (object 4), a picture XObject `/Im1` with
/// it: 2 by 1 RGB pixels, red then blue.
fn drawn(content: &str) -> Document {
    use std::io::Write as _;
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 600 800] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im1 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 6 >>\nstream\n\u{ff}\u{0}\u{0}\u{0}\u{0}\u{ff}\nendstream".to_string(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj", i + 1).unwrap();
        // The picture's bytes as bytes, not as UTF-8.
        if i == 4 {
            let (head, _) = o.split_once("stream\n").unwrap();
            out.extend_from_slice(head.as_bytes());
            out.extend_from_slice(b"stream\n\xff\x00\x00\x00\x00\xff\nendstream");
        } else {
            out.extend_from_slice(o.as_bytes());
        }
        out.extend_from_slice(b"\nendobj\n");
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

#[test]
fn shapes_say_how_they_are_painted_and_are_painted_another_way() {
    use katna_pdf_cos::ObjectChange;
    // A red-lined, dashed, 2 pt wide rectangle drawn at half size, and a
    // blue-filled curve.
    let doc = drawn(
        "q 0.5 0 0 0.5 0 0 cm 1 0 0 RG 4 w [6 2] 0 d 1 J 100 100 200 100 re S Q \
         0 0 1 rg 300 300 m 350 400 450 400 500 300 c h f",
    );
    let objects = doc.page_objects(0);
    assert_eq!(objects.len(), 2, "{objects:?}");
    let style = objects[0].style.as_ref().unwrap();
    assert_eq!(style.fill, None);
    assert_eq!(style.stroke, Some([1.0, 0.0, 0.0]));
    assert!((style.width - 2.0).abs() < 1e-3, "{style:?}");
    assert_eq!(style.dash, vec![3.0, 1.0]);
    assert_eq!(style.cap, 1);
    assert_eq!(objects[0].points, vec![[50.0, 50.0], [150.0, 100.0]]);
    let curve = objects[1].style.as_ref().unwrap();
    assert_eq!(curve.fill, Some([0.0, 0.0, 1.0]));
    assert_eq!(curve.stroke, None);
    assert_eq!(objects[1].points, vec![[300.0, 300.0], [500.0, 300.0]]);
    assert!(!objects[0].clipped);

    // The rectangle filled too, its line 3 pt (6 in its own units) and
    // solid; the curve stroked only.
    let filled = ObjectChange::Style {
        fill: Some(true),
        stroke: None,
        width: Some(3.0),
        dash: Some(Vec::new()),
        cap: None,
        join: Some(2),
    };
    let out = Document::load(doc.change_object(0, 0, &filled).unwrap()).unwrap();
    let objects = out.page_objects(0);
    let style = objects[0].style.as_ref().unwrap();
    assert_eq!(style.fill, Some([0.0, 0.0, 0.0]), "{}", content(&out));
    assert_eq!(style.stroke, Some([1.0, 0.0, 0.0]));
    assert!((style.width - 3.0).abs() < 1e-3, "{style:?}");
    assert_eq!(style.dash, Vec::<f32>::new());
    assert_eq!(style.join, 2);
    assert!(content(&out).contains("6 w [] 0 d 2 j"), "{}", content(&out));
    assert!(content(&out).contains("re B"), "{}", content(&out));
    let stroked = ObjectChange::Style {
        fill: Some(false),
        stroke: Some(true),
        width: None,
        dash: None,
        cap: None,
        join: None,
    };
    let out = Document::load(out.change_object(0, 1, &stroked).unwrap()).unwrap();
    let style = out.page_objects(0)[1].style.clone().unwrap();
    assert_eq!(style.fill, None, "{}", content(&out));
    assert_eq!(style.stroke, Some([0.0, 0.0, 0.0]));
    assert!(content(&out).contains("c h S"), "{}", content(&out));
    // Changed again, the first wrapper is changed, not wrapped again.
    let wider = ObjectChange::Style {
        fill: None,
        stroke: None,
        width: Some(5.0),
        dash: None,
        cap: None,
        join: None,
    };
    let again = Document::load(out.change_object(0, 0, &wider).unwrap()).unwrap();
    let style = again.page_objects(0)[0].style.clone().unwrap();
    assert!((style.width - 5.0).abs() < 1e-3, "{style:?}");
    assert_eq!(content(&again).matches(" w ").count(), 2, "{}", content(&again));

    // A corner of the rectangle moved: the opposite one stays; the
    // curve's end moved.
    let moved = ObjectChange::MovePoint {
        index: 0,
        to: [60.0, 40.0],
    };
    let out = Document::load(doc.change_object(0, 0, &moved).unwrap()).unwrap();
    let objects = out.page_objects(0);
    assert_eq!(objects[0].points, vec![[60.0, 40.0], [150.0, 100.0]]);
    assert!(close(objects[0].rect, [60.0, 40.0, 150.0, 100.0]), "{objects:?}");
    let moved = ObjectChange::MovePoint {
        index: 1,
        to: [520.0, 320.0],
    };
    let out = Document::load(doc.change_object(0, 1, &moved).unwrap()).unwrap();
    let objects = out.page_objects(0);
    assert_eq!(objects[1].points, vec![[300.0, 300.0], [520.0, 320.0]]);
    assert!(content(&out).contains("470 420 520 320 c"), "{}", content(&out));
}

#[test]
fn a_clipped_picture_is_drawn_whole_again() {
    use katna_pdf_cos::ObjectChange;
    // The picture 200 wide, clipped to its left half, then a shape.
    let doc = drawn(
        "q 100 100 100 100 re W n q 200 0 0 100 100 100 cm /Im1 Do Q Q \
         0 0 1 rg 400 400 50 50 re f",
    );
    let objects = doc.page_objects(0);
    assert_eq!(objects.len(), 2);
    assert!(objects[0].clipped, "{objects:?}");
    assert!(!objects[1].clipped);
    let out = Document::load(doc.change_object(0, 0, &ObjectChange::Unclip).unwrap()).unwrap();
    let objects = out.page_objects(0);
    assert_eq!(objects.len(), 2, "{}", content(&out));
    // Drawn where the clip ended: before the shape, the same size.
    assert_eq!(objects[0].kind, ObjectKind::Picture);
    assert!(!objects[0].clipped, "{}", content(&out));
    assert!(close(objects[0].rect, [100.0, 100.0, 300.0, 200.0]), "{objects:?}");
    assert_eq!(objects[1].kind, ObjectKind::Path);
    // Nothing to do when it is not clipped.
    let same = Document::load(out.change_object(0, 0, &ObjectChange::Unclip).unwrap()).unwrap();
    assert_eq!(same.page_objects(0), objects);
}

/// A 1 by 1 gray JPEG.
fn jpeg_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, 90);
    encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
    encoder
        .encode(&[128], 1, 1, jpeg_encoder::ColorType::Luma)
        .unwrap();
    out
}

#[test]
fn a_picture_comes_out_as_a_file_and_a_freehand_line_goes_in() {
    use katna_pdf_cos::PictureFile;
    let doc = drawn("q 200 0 0 100 100 100 cm /Im1 Do Q");
    match doc.picture_file(0, 0) {
        Some(PictureFile::Samples {
            width,
            height,
            components,
            bits,
            data,
        }) => {
            assert_eq!((width, height, components, bits), (2, 1, 3, 8));
            assert_eq!(data, vec![255, 0, 0, 0, 0, 255]);
        }
        other => panic!("{other:?}"),
    }
    // A JPEG comes out as it went in.
    let jpeg = jpeg_bytes();
    let with = Document::load(
        doc.insert(
            0,
            &[Insert::Image {
                rect: [10.0, 10.0, 20.0, 20.0],
                image: Image::Jpeg {
                    data: jpeg.clone(),
                    width: 1,
                    height: 1,
                    components: 1,
                },
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(with.picture_file(0, 1), Some(PictureFile::Jpeg(jpeg)));
    let out = Document::load(
        doc.insert(
            0,
            &[Insert::Path {
                points: vec![[10.0, 10.0], [20.0, 30.0], [40.0, 35.0]],
                stroke: [0.0, 0.5, 0.0],
                width: 2.5,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    let objects = out.page_objects(0);
    assert_eq!(objects.len(), 2, "{}", content(&out));
    let line = &objects[1];
    assert_eq!(line.kind, ObjectKind::Path);
    assert_eq!(line.points, vec![[10.0, 10.0], [20.0, 30.0], [40.0, 35.0]]);
    let style = line.style.as_ref().unwrap();
    assert_eq!(style.stroke, Some([0.0, 0.5, 0.0]));
    assert_eq!(style.fill, None);
    assert!((style.width - 2.5).abs() < 1e-3);
    assert_eq!((style.cap, style.join), (1, 1));
}
