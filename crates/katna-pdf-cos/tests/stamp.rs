// SPDX-License-Identifier: GPL-3.0-or-later

//! Headers, footers, watermarks and Bates numbers on many pages.

use katna_pdf_cos::{
    Background, Bates, Document, Image, Object, PageSource, Picture, Stamp, StampPlace, compose,
    decode, text_pdf,
};

/// All the page's content streams, decoded, one string each.
fn streams(doc: &Document, page: usize) -> Vec<String> {
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
        .collect()
}

/// Three A4 letter pages, the middle one turned a quarter when `turned`.
fn three(turned: bool) -> Document {
    let one = Document::load(text_pdf("Letter", &["  Dear Asha,".to_string()])).unwrap();
    let pages: Vec<_> = (0..3)
        .map(|i| PageSource {
            doc: &one,
            page: 0,
            turns: i32::from(turned && i == 1),
            crop: None,
        })
        .collect();
    Document::load(compose(None, &pages).unwrap()).unwrap()
}

fn footer(text: &str, place: StampPlace) -> Stamp {
    Stamp {
        text: text.into(),
        place,
        font: "Helvetica".into(),
        size: 10.0,
        colour: [0.0; 3],
        opacity: 1.0,
        margin: 36.0,
        under: false,
        bates: None,
    }
}

#[test]
fn footers_carry_each_page_number() {
    let doc = three(false);
    let out = doc
        .stamp(
            &[0, 1, 2],
            &footer("Page {page} of {pages}", StampPlace::BottomCentre),
        )
        .unwrap();
    let out = Document::load(out).unwrap();
    assert!(out.bytes().starts_with(doc.bytes()));
    for page in 0..3 {
        let runs = out.text_runs(page);
        let run = runs
            .iter()
            .find(|r| r.text == format!("Page {} of 3", page + 1))
            .unwrap_or_else(|| panic!("{runs:?}"));
        // In the middle of the page's foot, its baseline at the margin.
        let middle = (run.rect[0] + run.rect[2]) / 2.0;
        assert!((middle - 595.0 / 2.0).abs() < 0.5, "{run:?}");
        assert!(run.rect[1] < 36.0 && run.rect[3] > 36.0, "{run:?}");
        // The page's own text is as it was.
        assert!(runs.iter().any(|r| r.text == "Dear Asha,"));
    }
}

#[test]
fn bates_numbers_run_on_over_the_pages_stamped() {
    let doc = three(false);
    let mut stamp = footer("{bates}", StampPlace::BottomRight);
    stamp.bates = Some(Bates {
        prefix: "ACME-".into(),
        suffix: String::new(),
        start: 7,
        digits: 4,
    });
    let out = Document::load(doc.stamp(&[0, 2], &stamp).unwrap()).unwrap();
    let has = |page: usize, text: &str| out.text_runs(page).iter().any(|r| r.text == text);
    assert!(has(0, "ACME-0007"));
    assert!(has(2, "ACME-0008"));
    assert!(!out.text_runs(1).iter().any(|r| r.text.starts_with("ACME")));
    // At the right, the margin from the edge.
    let run = out
        .text_runs(0)
        .into_iter()
        .find(|r| r.text == "ACME-0007")
        .unwrap();
    assert!((run.rect[2] - (595.0 - 36.0)).abs() < 0.5, "{run:?}");
}

#[test]
fn a_watermark_is_faint_and_turned_and_can_go_under() {
    let doc = three(false);
    let mut stamp = footer("DRAFT", StampPlace::Across);
    stamp.size = 60.0;
    stamp.opacity = 0.3;
    stamp.under = true;
    let out = Document::load(doc.stamp(&[0], &stamp).unwrap()).unwrap();
    let all = streams(&out, 0);
    // Drawn first, so the page draws over it.
    assert!(all[0].contains("(DRAFT) Tj"), "{all:?}");
    assert!(all[0].contains("gs"), "{all:?}");
    // Turned along the page's diagonal.
    let angle = 842f32.atan2(595.0);
    let turn = format!("{:.3}", angle.cos());
    assert!(all[0].contains(turn.trim_end_matches('0')), "{all:?}");
    let dict = out.get(out.pages()[0]).unwrap();
    let resources = out.resolve(dict.as_dict().unwrap().get("Resources").unwrap());
    let Object::Dict(states) = out.resolve_key(resources.as_dict().unwrap(), "ExtGState") else {
        panic!("no graphics states");
    };
    let (_, state) = states.iter().next().unwrap();
    let Object::Dict(state) = out.resolve(state) else {
        panic!("not a dictionary");
    };
    let faint = state.get("ca").and_then(Object::as_number).unwrap();
    assert!((faint - 0.3).abs() < 1e-6, "{faint}");
}

#[test]
fn a_footer_sits_at_the_foot_of_a_turned_page() {
    let doc = three(true);
    let out = doc
        .stamp(&[1], &footer("Foot", StampPlace::BottomLeft))
        .unwrap();
    let out = Document::load(out).unwrap();
    let all = streams(&out, 1).join("\n");
    // The page is turned a quarter clockwise: its shown foot is the
    // page's right side, read upwards.
    assert!(all.contains("q 0 1 -1 0 595 0 cm"), "{all}");
    let run = out
        .text_runs(1)
        .into_iter()
        .find(|r| r.text == "Foot")
        .unwrap();
    assert!(run.rect[0] > 545.0 && run.rect[1] > 30.0, "{run:?}");
}

#[test]
fn a_background_goes_under_each_page_and_shares_one_picture() {
    let doc = three(true);
    // A 2 by 1 picture: fitted to the A4 page's width, centred upright.
    let picture = Image::Pixels(Picture {
        width: 2,
        height: 1,
        rgb: vec![255, 0, 0, 0, 0, 255],
        alpha: None,
    });
    let background = Background {
        colour: Some([1.0, 1.0, 0.8]),
        image: Some(picture),
        stretch: false,
        opacity: 0.5,
    };
    let out = Document::load(doc.background(&[0, 1, 2], &background).unwrap()).unwrap();
    let first = streams(&out, 0);
    assert!(
        first[0].contains("1 1 0.8 rg 0 0 595 842 re f"),
        "{first:?}"
    );
    assert!(first[0].contains("595 0 0 297.5 0 272.25 cm"), "{first:?}");
    assert!(first[0].contains(" Do"), "{first:?}");
    assert!(first[0].contains(" gs"), "{first:?}");
    assert!(first.last().unwrap().contains("Dear Asha"), "{first:?}");
    // The turned page: its shown width is the page's height.
    let turned = streams(&out, 1);
    assert!(turned[0].contains("0 0 842 595 re f"), "{turned:?}");
    // Each page names the same picture.
    let picture_of = |page: usize| {
        let dict = out.get(out.pages()[page]).unwrap();
        let res = out.resolve(dict.as_dict().unwrap().get("Resources").unwrap());
        let Object::Dict(x) = out.resolve_key(res.as_dict().unwrap(), "XObject") else {
            panic!("no pictures");
        };
        x.iter().next().unwrap().1.as_ref().unwrap()
    };
    assert_eq!(picture_of(0), picture_of(2));
}
