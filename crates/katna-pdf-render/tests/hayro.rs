// SPDX-License-Identifier: GPL-3.0-or-later

//! The hayro engine on small PDFs written here, so the tests need no files.

use std::sync::Arc;

use katna_pdf_render::scene::{HEAVY_SEGMENTS, Unsupported, page_scene, quick_reader_agrees};
use katna_pdf_render::{Bitmap, Engine, Hayro, HayroPlain, OpenError, Region};

/// A PDF with one 200 × 100 pt page per entry of `pages`, each a content
/// stream; the xref offsets are worked out, so hayro reads it without
/// repairing it.
fn pdf(pages: &[&str]) -> Vec<u8> {
    pdf_with(pages, false)
}

/// [`pdf`], with each content stream Flate-compressed when `flate`.
fn pdf_with(pages: &[&str], flate: bool) -> Vec<u8> {
    let n = pages.len();
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")).into_bytes(),
    ];
    for (i, content) in pages.iter().enumerate() {
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents {} 0 R >>",
                4 + 2 * i
            )
            .into_bytes(),
        );
        let (filter, data) = if flate {
            use std::io::Write as _;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(content.as_bytes()).unwrap();
            (" /Filter /FlateDecode", encoder.finish().unwrap())
        } else {
            ("", content.as_bytes().to_vec())
        };
        let mut stream = format!("<< /Length {}{filter} >>\nstream\n", data.len()).into_bytes();
        stream.extend_from_slice(&data);
        stream.extend_from_slice(b"\nendstream");
        objects.push(stream);
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
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

/// A one-page PDF that draws a `width`×`height` one-bit image of vertical
/// stripes (every other pixel white) with `content`.
fn striped_image_pdf(width: u32, height: u32, content: &str) -> Vec<u8> {
    let row = vec![0xaa_u8; width.div_ceil(8) as usize];
    let data = row.repeat(height as usize);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    object(
        &mut out,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
          /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >>",
    );
    object(
        &mut out,
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .as_bytes(),
    );
    let mut image = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /DeviceGray /BitsPerComponent 1 /Length {} >>\nstream\n",
        data.len()
    )
    .into_bytes();
    image.extend_from_slice(&data);
    image.extend_from_slice(b"\nendstream");
    object(&mut out, &image);
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn a_huge_image_drawn_small_averages_to_grey() {
    // 8 megapixels drawn twice, at 200 × 100 and 100 × 50 points: averaged
    // down while decoding (vendor/hayro-interpret/KATNA.md), once per size.
    let content = "q 200 0 0 100 0 0 cm /Im1 Do Q q 100 0 0 50 0 0 cm /Im1 Do Q";
    let pdf = striped_image_pdf(4096, 2048, content);
    let doc = Hayro.open(Arc::new(pdf), "").unwrap();
    let page = doc.render(0, 1.0).unwrap();
    for (x, y) in [(150, 25), (50, 75), (100, 50)] {
        let [r, g, b, _] = pixel(&page, x, y);
        assert!((120..=136).contains(&r), "({x}, {y}) is {r}, not mid grey");
        assert_eq!((r, g), (g, b), "grey");
    }
}

/// A CAD-like page: more than [`HEAVY_SEGMENTS`] short hairlines spread
/// over the page, a dashed thick line, a clip and a filled box. `extra` is
/// appended, inside the clip.
fn drawing(extra: &str) -> String {
    let mut seed = 7u32;
    let mut next = move |range: f32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1 << 24) as f32 * range
    };
    let mut out = String::from("0.1 w 0 0 1 RG q 10 10 180 80 re W n\n");
    for _ in 0..HEAVY_SEGMENTS + 1000 {
        let (x, y) = (next(200.0), next(100.0));
        let (dx, dy) = (next(6.0) - 3.0, next(6.0) - 3.0);
        out.push_str(&format!(
            "{x:.2} {y:.2} m {:.2} {:.2} l S\n",
            x + dx,
            y + dy
        ));
    }
    out.push_str("1 0 0 rg 60 30 40 20 re f 2 w [4 2] 0 d 0 g 20 50 m 180 50 l S\n");
    out.push_str(extra);
    out.push_str("\nQ");
    out
}

/// The share of pixels that differ clearly between two drawings.
fn differs(a: &Bitmap, b: &Bitmap) -> f64 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let differ = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 64))
        .count();
    differ as f64 / (a.width * a.height) as f64
}

#[test]
fn a_drawing_is_drawn_from_its_scene_as_hayro_draws_it() {
    let bytes = Arc::new(pdf(&[&drawing("")]));
    let scene = page_scene(bytes.clone(), 0).unwrap().unwrap();
    assert!(scene.len() > HEAVY_SEGMENTS, "{} items", scene.len());
    let fast = Hayro.open(bytes.clone(), "").unwrap();
    let usual = HayroPlain.open(bytes, "").unwrap();
    let a = fast.render(0, 2.0).unwrap();
    let b = usual.render(0, 2.0).unwrap();
    assert!(differs(&a, &b) < 0.005, "whole page: {}", differs(&a, &b));
    // A deep tile over the red box and the dashed line.
    let region = Region {
        x: 1200,
        y: 700,
        width: 256,
        height: 256,
    };
    let a = fast.render_region(0, 16.0, region).unwrap();
    let b = usual.render_region(0, 16.0, region).unwrap();
    assert!(differs(&a, &b) < 0.005, "deep tile: {}", differs(&a, &b));
}

#[test]
fn a_drawing_with_an_image_is_drawn_the_usual_way() {
    let image = "q 20 0 0 20 150 60 cm BI /W 2 /H 2 /BPC 8 /CS /G /F /AHx ID 00FFFF00> EI Q";
    let bytes = Arc::new(pdf(&[&drawing(image)]));
    assert_eq!(
        page_scene(bytes.clone(), 0).unwrap().err(),
        Some(Unsupported::Image)
    );
    let a = Hayro
        .open(bytes.clone(), "")
        .unwrap()
        .render(0, 1.0)
        .unwrap();
    let b = HayroPlain.open(bytes, "").unwrap().render(0, 1.0).unwrap();
    assert_eq!(a, b);
}

#[test]
fn the_quick_reader_reads_plain_paths_as_hayro_does() {
    let pages = [
        SQUARE,
        // Strokes, caps, joins, dashes, curves, closes, and colours.
        "1 0 0 RG 2 w 1 J 1 j 4 M [3 0 2] 1 d 10 10 m 50 10 l 50 50 l h S \
         0.5 g 2 J 2 j [] 0 d 60 10 m 70 40 80 40 90 10 c 95 20 100 15 v 100 10 110 15 y s",
        // Fills of every kind, a zero-sized one, and one point twice.
        "0 0 1 rg 10 10 m 40 10 l 40 40 l 10 40 l f* 0.2 0.4 0.6 rg 50 10 30 30 re B \
         50 50 m 90 50 l f 60 60 m 60 60 l S 1 0 0 1 5 5 cm 10 60 20 20 re b*",
        // Clips: a rectangle inside the page, one covering only that, one
        // covering the page, a triangle,
        // with saves and restores around and between them.
        "q 20 20 100 50 re W n 0 g 0 0 200 100 re f q 10 10 150 70 re W n 0.5 g 0 0 200 100 re f Q \
         q -10 -10 300 300 re W* n \
         10 10 m 150 10 l 80 90 l h W n 1 0 0 rg 0 0 200 100 re f Q 0 1 0 rg \
         30 30 40 40 re f Q 0 0 1 RG 5 w 0 0 m 200 100 l S Q Q",
        // Odd but valid numbers, comments and white space.
        "% a comment\n+1 -.5 0 1. 0 0 cm\t.5 G 1.000 w 0 0 m\r\n-0 100 l 007 50 l S",
    ];
    let bytes = Arc::new(pdf(&pages));
    for i in 0..pages.len() {
        assert_eq!(
            quick_reader_agrees(bytes.clone(), i),
            Some(true),
            "page {i}"
        );
    }
}

#[test]
fn the_quick_reader_leaves_other_pages_to_hayro() {
    let pages = [
        "BT /F1 12 Tf (text) Tj ET",
        "0 0 0 1 k 10 10 50 50 re f",
        "/GS1 gs 10 10 50 50 re f",
        "1 2 m 3 l S",
        "10 10 m 20 20l S",
    ];
    let bytes = Arc::new(pdf(&pages));
    for i in 0..pages.len() {
        assert_eq!(quick_reader_agrees(bytes.clone(), i), None, "page {i}");
    }
}

#[test]
fn a_drawing_of_hairlines_is_drawn_as_hayro_draws_it() {
    // Only one-pixel lines, clipped by a rectangle: drawn by the scene's
    // quick line drawer when zoomed out. Polylines, curves and round
    // caps too.
    let mut page = drawing("").replace(
        "1 0 0 rg 60 30 40 20 re f 2 w [4 2] 0 d 0 g 20 50 m 180 50 l S\n",
        "",
    );
    page.insert_str(
        page.len() - 1,
        "1 J 1 j 0.3 0.3 0.3 RG 30 30 m 40 35 l 45 25 l 60 40 l S \
         70 70 m 80 90 100 50 110 70 c S 120 20 m 140 30 l 130 45 l h S\n",
    );
    let bytes = Arc::new(pdf(&[&page]));
    let fast = Hayro.open(bytes.clone(), "").unwrap();
    let usual = HayroPlain.open(bytes, "").unwrap();
    for scale in [1.0, 3.0] {
        let a = fast.render(0, scale).unwrap();
        let b = usual.render(0, scale).unwrap();
        assert!(differs(&a, &b) < 0.005, "at {scale}: {}", differs(&a, &b));
    }
}

#[test]
fn the_quick_reader_reads_a_compressed_drawing_in_pieces() {
    // More than a megabyte inflated, so it arrives in several pieces, with
    // comments and dash patterns along the way.
    let page = drawing("").replace(
        " l S\n",
        " l S % a comment that may be cut\n[0.5 0.25] 0.1 d\n",
    );
    assert!(page.len() > 3 << 20, "{} bytes", page.len());
    let bytes = Arc::new(pdf_with(&[&page], true));
    assert_eq!(quick_reader_agrees(bytes.clone(), 0), Some(true));
    let scene = page_scene(bytes, 0).unwrap().unwrap();
    assert!(scene.len() > HEAVY_SEGMENTS, "{} items", scene.len());
}
