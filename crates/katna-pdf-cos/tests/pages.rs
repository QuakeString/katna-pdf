// SPDX-License-Identifier: GPL-3.0-or-later

//! Page operations: one file of pages taken from others.

use std::io::Write as _;

use katna_pdf_cos::{ComposeError, Document, Object, PageSource, compose};

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
        "trailer\n<< /Size {} /Root 1 0 R /Info 9 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out
}

/// Three pages under a two-level tree that hands down a media box and a
/// rotation; the outline points at the third page; the second page's
/// content is big.
fn three() -> Document {
    Document::load(file(&[
        "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R >>",
        "<< /Type /Pages /Count 3 /Kids [4 0 R 3 0 R] /MediaBox [0 0 300 200] >>",
        "<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [5 0 R 6 0 R] /Rotate 90 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 8 0 R >>",
        "<< /Type /Page /Parent 3 0 R /Contents 7 0 R >>",
        "<< /Type /Page /Parent 3 0 R /Rotate 270 >>",
        &format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            4000,
            "x".repeat(4000)
        ),
        "<< /Length 3 >>\nstream\nq Q\nendstream",
        "<< /Title (Three) >>",
        "<< /Type /Outlines /First 11 0 R /Last 11 0 R /Count 1 >>",
        "<< /Title (End) /Parent 10 0 R /Dest [6 0 R /Fit] >>",
    ]))
    .unwrap()
}

fn rotation(doc: &Document, page: usize) -> i64 {
    let p = doc.get(doc.pages()[page]).unwrap();
    p.as_dict().unwrap().int("Rotate").unwrap_or(0)
}

#[test]
fn pages_are_reordered_turned_and_dropped() {
    let doc = three();
    // Third, then first turned right; the second left out.
    let out = compose(
        Some(&doc),
        &[
            PageSource {
                doc: &doc,
                page: 2,
                turns: 0,
                crop: None,
            },
            PageSource {
                doc: &doc,
                page: 0,
                turns: 1,
                crop: None,
            },
        ],
    )
    .unwrap();
    // The second page's big content went with it.
    assert!(out.len() < doc.bytes().len() - 3000, "{} bytes", out.len());
    let new = Document::load(out).unwrap();
    assert!(!new.was_repaired());
    assert_eq!(new.pages().len(), 2);
    assert_eq!(rotation(&new, 0), 270);
    assert_eq!(rotation(&new, 1), 90);
    // Inherited attributes came along.
    let first = new.get(new.pages()[0]).unwrap();
    let media = new.resolve_key(first.as_dict().unwrap(), "MediaBox");
    assert_eq!(media.as_array().unwrap()[2].as_number(), Some(300.0));
    // The outline still leads to the page it led to, now the first.
    let items = new.outline();
    assert_eq!(items[0].title, "End");
    assert_eq!(items[0].dest.as_ref().map(|d| d.page), Some(0));
    // And the information dictionary is kept.
    let info = new.resolve_key(new.trailer(), "Info");
    assert!(info.as_dict().unwrap().get("Title").is_some());
}

#[test]
fn files_are_merged_and_pages_repeated() {
    let a = three();
    let b = three();
    let mut pages: Vec<PageSource> = (0..3)
        .map(|page| PageSource {
            doc: &a,
            page,
            turns: 0,
            crop: None,
        })
        .collect();
    pages.extend((0..3).map(|page| PageSource {
        doc: &b,
        page,
        turns: -1,
        crop: None,
    }));
    pages.push(PageSource {
        doc: &a,
        page: 0,
        turns: 0,
        crop: None,
    });
    let new = Document::load(compose(Some(&a), &pages).unwrap()).unwrap();
    assert_eq!(new.pages().len(), 7);
    assert_eq!(rotation(&new, 4), 0);
    assert_eq!(rotation(&new, 5), 180);
    // The repeated page is a page of its own.
    assert_ne!(new.pages()[0], new.pages()[6]);
}

#[test]
fn a_split_off_page_stands_alone() {
    let doc = three();
    let new = Document::load(
        compose(
            None,
            &[PageSource {
                doc: &doc,
                page: 1,
                turns: 0,
                crop: None,
            }],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(new.pages().len(), 1);
    assert!(new.outline().is_empty());
    let contents = new.resolve_key(
        new.get(new.pages()[0]).unwrap().as_dict().unwrap(),
        "Contents",
    );
    assert!(matches!(contents, Object::Stream(s) if s.data.len() == 4000));
    assert_eq!(
        compose(
            None,
            &[PageSource {
                doc: &doc,
                page: 3,
                turns: 0,
                crop: None
            }]
        )
        .err(),
        Some(ComposeError::NoSuchPage(3))
    );
    assert_eq!(compose(Some(&doc), &[]).err(), Some(ComposeError::Empty));
}

#[test]
fn a_composed_file_composes_again() {
    let doc = three();
    let all = |doc, turns: [i32; 3]| -> Vec<PageSource<'_>> {
        (0..3)
            .map(|page| PageSource {
                doc,
                page,
                turns: turns[page],
                crop: None,
            })
            .collect()
    };
    let once = Document::load(compose(Some(&doc), &all(&doc, [1, 0, 0])).unwrap()).unwrap();
    // Every page's parent is the new page tree.
    let root = once
        .catalog()
        .unwrap()
        .get("Pages")
        .and_then(Object::as_ref);
    for &page in &once.pages() {
        let page = once.get(page).unwrap();
        assert_eq!(
            page.as_dict()
                .unwrap()
                .get("Parent")
                .and_then(Object::as_ref),
            root
        );
    }
    // Swapping the first two pages moves their turns with them.
    let swapped: Vec<_> = [1, 0, 2]
        .into_iter()
        .map(|page| PageSource {
            doc: &once,
            page,
            turns: 0,
            crop: None,
        })
        .collect();
    let twice = Document::load(compose(Some(&once), &swapped).unwrap()).unwrap();
    assert_eq!(
        (0..3).map(|p| rotation(&twice, p)).collect::<Vec<_>>(),
        [90, 90, 270]
    );
}

#[test]
fn page_boxes_and_turns_are_read_through_the_tree() {
    let doc = three();
    let first = doc.page_info(0).unwrap();
    assert_eq!(first.media, [0.0, 0.0, 100.0, 100.0]);
    assert_eq!(first.rotate, 0);
    // The second takes its box and turn from the tree above it.
    let second = doc.page_info(1).unwrap();
    assert_eq!(second.media, [0.0, 0.0, 300.0, 200.0]);
    assert_eq!(second.crop, second.media);
    assert_eq!(second.rotate, 90);
    assert_eq!(second.size(), (200.0, 300.0));
    assert_eq!(doc.page_info(2).unwrap().rotate, 270);
    assert!(doc.page_info(3).is_none());
}

#[test]
fn pages_are_cropped_within_their_media_and_uncropped() {
    let doc = three();
    let crop = |doc: &Document, page, crop| {
        Document::load(
            compose(
                Some(doc),
                &[PageSource {
                    doc,
                    page,
                    turns: 0,
                    crop: Some(crop),
                }],
            )
            .unwrap(),
        )
        .unwrap()
    };
    // Cut to the media box where it reaches past it.
    let cropped = crop(&doc, 1, [10.0, 20.0, 400.0, 150.0]);
    let info = cropped.page_info(0).unwrap();
    assert_eq!(info.crop, [10.0, 20.0, 300.0, 150.0]);
    assert_eq!(info.media, [0.0, 0.0, 300.0, 200.0]);
    assert_eq!(info.size(), (130.0, 290.0));
    // The media box uncrops.
    let uncropped = crop(&cropped, 0, info.media);
    assert_eq!(uncropped.page_info(0).unwrap().crop, info.media);
    let page = uncropped.get(uncropped.pages()[0]).unwrap();
    assert!(page.as_dict().unwrap().get("CropBox").is_none());
    // Nothing left: the crop is not changed.
    let none = crop(&cropped, 0, [500.0, 500.0, 600.0, 600.0]);
    assert_eq!(none.page_info(0).unwrap().crop, info.crop);
}
