// SPDX-License-Identifier: GPL-3.0-or-later

//! The hayro engine on small PDFs written here, so the tests need no files.

use std::sync::Arc;

use katna_pdf_render::{Engine, Hayro, OpenError, Region};

/// A PDF with one 200 × 100 pt page per entry of `pages`, each a content
/// stream; the xref offsets are worked out, so hayro reads it without
/// repairing it.
fn pdf(pages: &[&str]) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")),
    ];
    for (i, content) in pages.iter().enumerate() {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents {} 0 R >>",
            4 + 2 * i
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// A black square from (50, 25) to (100, 75) pt, PDF coordinates (y up).
const SQUARE: &str = "0 0 0 rg 50 25 50 50 re f";

fn pixel(bitmap: &katna_pdf_render::Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * bitmap.width + x) * 4) as usize;
    bitmap.rgba[i..i + 4].try_into().unwrap()
}

#[test]
fn opens_and_measures_pages() {
    let doc = Hayro.open(Arc::new(pdf(&[SQUARE, ""])), "").unwrap();
    assert_eq!(doc.page_count(), 2);
    assert_eq!(doc.page_size(0), Some((200.0, 100.0)));
    assert_eq!(doc.page_size(2), None);
}

#[test]
fn draws_a_whole_page() {
    let doc = Hayro.open(Arc::new(pdf(&[SQUARE])), "").unwrap();
    let page = doc.render(0, 2.0).unwrap();
    assert_eq!((page.width, page.height), (400, 200));
    // Inside the square (y flipped: the page's top is row 0), and paper.
    assert_eq!(pixel(&page, 150, 100), [0, 0, 0, 255]);
    assert_eq!(pixel(&page, 10, 10), [255, 255, 255, 255]);
}

#[test]
fn draws_a_region_as_the_whole_page_would() {
    let doc = Hayro.open(Arc::new(pdf(&[SQUARE])), "").unwrap();
    let whole = doc.render(0, 2.0).unwrap();
    let region = Region {
        x: 80,
        y: 40,
        width: 64,
        height: 48,
    };
    let tile = doc.render_region(0, 2.0, region).unwrap();
    assert_eq!((tile.width, tile.height), (64, 48));
    for (x, y) in [(0, 0), (63, 47), (20, 10), (40, 30)] {
        assert_eq!(
            pixel(&tile, x, y),
            pixel(&whole, region.x + x, region.y + y),
            "at {x},{y}"
        );
    }
}

#[test]
fn refuses_what_is_not_a_pdf() {
    let err = Hayro.open(Arc::new(b"hello".to_vec()), "").err();
    assert_eq!(err, Some(OpenError::Invalid));
}
