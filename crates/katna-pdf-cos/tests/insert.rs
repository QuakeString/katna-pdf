// SPDX-License-Identifier: GPL-3.0-or-later

//! Putting text, pictures and shapes into a page's content.

use katna_pdf_cos::{
    Document, Image, Insert, Object, PageSource, Picture, ShapeKind, compose, decode, text_pdf,
};

/// All the page's content streams, decoded and joined.
fn content(doc: &Document, page: usize) -> String {
    let dict = doc.get(doc.pages()[page]).unwrap();
    let contents = dict.as_dict().unwrap().get("Contents").unwrap().clone();
    let streams = match doc.resolve(&contents) {
        Object::Array(a) => a,
        other => vec![other],
    };
    streams
        .iter()
        .map(|s| {
            let Object::Stream(s) = doc.resolve(s) else {
                panic!("not a stream");
            };
            String::from_utf8_lossy(&decode(&s.dict, &s.data).unwrap()).into_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn letter() -> Document {
    Document::load(text_pdf("Letter", &["  Dear Asha,".to_string()])).unwrap()
}

#[test]
fn text_is_drawn_and_can_be_edited() {
    let doc = letter();
    let out = doc
        .insert(
            0,
            &[Insert::Text {
                at: [100.0, 200.0],
                text: "Paid in full\nwith thanks, €500".into(),
                font: "Times-Bold".into(),
                size: 14.0,
                colour: [0.8, 0.0, 0.0],
            }],
        )
        .unwrap();
    let out = Document::load(out).unwrap();
    assert!(out.bytes().starts_with(doc.bytes()));
    let runs = out.text_runs(0);
    let added: Vec<_> = runs.iter().filter(|r| r.font == "Times-Bold").collect();
    assert_eq!(added.len(), 2, "{runs:?}");
    assert_eq!(added[0].text, "Paid in full");
    assert_eq!(added[1].text, "with thanks, €500");
    assert!((added[0].rect[0] - 100.0).abs() < 0.01);
    assert!((added[0].rect[1] - (200.0 - 2.8)).abs() < 0.01);
    assert!((added[1].rect[1] - (200.0 - 16.8 - 2.8)).abs() < 0.01);
    // The page's own text is as it was.
    assert!(runs.iter().any(|r| r.text == "Dear Asha,"));
    // It is the page's text, so Edit text changes it.
    let index = runs.iter().position(|r| r.text == "Paid in full").unwrap();
    let edited =
        Document::load(out.edit_text(0, &[(index, "Paid in part".into())]).unwrap()).unwrap();
    assert!(edited.text_runs(0).iter().any(|r| r.text == "Paid in part"));
}

#[test]
fn text_stands_upright_on_a_turned_page() {
    let doc = letter();
    let turned = compose(
        Some(&doc),
        &[PageSource {
            doc: &doc,
            page: 0,
            turns: 1,
            crop: None,
        }],
    )
    .unwrap();
    let turned = Document::load(turned).unwrap();
    let out = turned
        .insert(
            0,
            &[Insert::Text {
                at: [100.0, 100.0],
                text: "Up".into(),
                font: "Helvetica".into(),
                size: 10.0,
                colour: [0.0; 3],
            }],
        )
        .unwrap();
    let out = Document::load(out).unwrap();
    assert!(
        content(&out, 0).contains("0 1 -1 0 100 100 Tm"),
        "{}",
        content(&out, 0)
    );
}

#[test]
fn pictures_and_shapes_are_drawn() {
    let doc = letter();
    let jpeg = Image::Jpeg {
        data: vec![0xFF, 0xD8, 0xFF, 0xD9],
        width: 4,
        height: 2,
        components: 3,
    };
    let pixels = Image::Pixels(Picture {
        width: 1,
        height: 1,
        rgb: vec![255, 0, 0],
        alpha: Some(vec![128]),
    });
    let out = doc
        .insert(
            0,
            &[
                Insert::Image {
                    rect: [10.0, 20.0, 110.0, 70.0],
                    image: jpeg,
                },
                Insert::Image {
                    rect: [0.0, 0.0, 5.0, 5.0],
                    image: pixels,
                },
                Insert::Shape {
                    kind: ShapeKind::Rectangle,
                    rect: [10.0, 10.0, 50.0, 30.0],
                    stroke: Some([0.0, 0.0, 1.0]),
                    fill: Some([1.0, 1.0, 0.0]),
                    width: 2.0,
                },
                Insert::Shape {
                    kind: ShapeKind::Ellipse,
                    rect: [0.0, 0.0, 20.0, 10.0],
                    stroke: Some([0.0; 3]),
                    fill: None,
                    width: 1.0,
                },
                Insert::Shape {
                    kind: ShapeKind::Line,
                    rect: [0.0, 0.0, 100.0, 100.0],
                    stroke: Some([0.0; 3]),
                    fill: Some([1.0; 3]),
                    width: 1.0,
                },
            ],
        )
        .unwrap();
    let out = Document::load(out).unwrap();
    let c = content(&out, 0);
    assert!(c.starts_with("q\n"), "{c}");
    assert!(c.contains("q 100 0 0 50 10 20 cm /KatnaI1 Do Q"), "{c}");
    assert!(c.contains("/KatnaI2 Do"), "{c}");
    assert!(
        c.contains("q 0 0 1 RG 2 w 1 1 0 rg 10 10 40 20 re B Q"),
        "{c}"
    );
    assert!(c.contains(" c h S Q"), "{c}");
    // A line is stroked, never filled.
    assert!(c.contains("0 0 m 100 100 l S Q"), "{c}");
    let page = out.get(out.pages()[0]).unwrap();
    let resources = page.as_dict().unwrap().get("Resources").unwrap().clone();
    let xobjects = out.resolve_key(resources.as_dict().unwrap(), "XObject");
    let Object::Stream(jpeg) = out.resolve(xobjects.as_dict().unwrap().get("KatnaI1").unwrap())
    else {
        panic!("no picture");
    };
    assert_eq!(jpeg.dict.name("Filter"), Some(&b"DCTDecode"[..]));
    let Object::Stream(pixels) = out.resolve(xobjects.as_dict().unwrap().get("KatnaI2").unwrap())
    else {
        panic!("no picture");
    };
    assert!(pixels.dict.get("SMask").is_some());
}
