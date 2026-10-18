// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain text as a new PDF: reports, such as what was found checking a
//! file's signatures, that people keep or send on. A4 pages in the
//! standard Helvetica fonts, so nothing is embedded; text outside
//! WinAnsi shows as `?`.

use crate::forms::literal;
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::write::write_file;

const WIDTH: f64 = 595.0;
const HEIGHT: f64 = 842.0;
const MARGIN: f64 = 56.0;
const TITLE_SIZE: f64 = 16.0;
const SIZE: f64 = 10.0;
const LEADING: f64 = 14.0;

/// A PDF of `title`, then `lines`. A line that does not start with a
/// space is a heading, in bold with space above it; an indented line
/// wraps under its own indent. Empty lines are kept.
pub fn text_pdf(title: &str, lines: &[String]) -> Vec<u8> {
    let mut pages: Vec<String> = Vec::new();
    let mut page = String::new();
    let mut y = HEIGHT - MARGIN - TITLE_SIZE;
    show(&mut page, "F2", TITLE_SIZE, MARGIN, y, title);
    y -= LEADING * 2.0;
    let usable = WIDTH - 2.0 * MARGIN;
    for line in lines {
        let indent = line.chars().take_while(|c| *c == ' ').count();
        let heading = indent == 0 && !line.is_empty();
        if heading && y < HEIGHT - MARGIN - TITLE_SIZE - LEADING * 2.0 {
            y -= LEADING / 2.0;
        }
        let (font, bold) = if heading { ("F2", true) } else { ("F1", false) };
        let x = MARGIN + indent as f64 * SIZE * 0.5;
        let wrapped = wrap(line.trim_start(), usable - (x - MARGIN), bold);
        for (i, part) in wrapped.iter().enumerate() {
            if y < MARGIN {
                pages.push(std::mem::take(&mut page));
                y = HEIGHT - MARGIN - SIZE;
            }
            // Wrapped parts hang under the text, past a label's colon.
            let hang = if i > 0 { SIZE } else { 0.0 };
            show(&mut page, font, SIZE, x + hang, y, part);
            y -= LEADING;
        }
    }
    pages.push(page);

    // 1 catalog, 2 page tree, 3 and 4 fonts, then each page and its content.
    let mut objects = vec![Object::Null; 4];
    let mut kids = Vec::new();
    for content in pages {
        objects.push(Object::Stream(Stream {
            dict: Dict::new(),
            data: content.into_bytes(),
        }));
        let content = objects.len() as u32;
        objects.push(Object::Dict(Dict(vec![
            (Name::new("Type"), Object::name("Page")),
            (Name::new("Parent"), Object::Ref(Ref::new(2, 0))),
            (
                Name::new("MediaBox"),
                Object::Array(vec![
                    Object::Int(0),
                    Object::Int(0),
                    Object::Int(WIDTH as i64),
                    Object::Int(HEIGHT as i64),
                ]),
            ),
            (
                Name::new("Resources"),
                Object::Dict(Dict(vec![(
                    Name::new("Font"),
                    Object::Dict(Dict(vec![
                        (Name::new("F1"), Object::Ref(Ref::new(3, 0))),
                        (Name::new("F2"), Object::Ref(Ref::new(4, 0))),
                    ])),
                )])),
            ),
            (Name::new("Contents"), Object::Ref(Ref::new(content, 0))),
        ])));
        kids.push(Object::Ref(Ref::new(objects.len() as u32, 0)));
    }
    let count = kids.len() as i64;
    objects[0] = Object::Dict(Dict(vec![
        (Name::new("Type"), Object::name("Catalog")),
        (Name::new("Pages"), Object::Ref(Ref::new(2, 0))),
    ]));
    objects[1] = Object::Dict(Dict(vec![
        (Name::new("Type"), Object::name("Pages")),
        (Name::new("Kids"), Object::Array(kids)),
        (Name::new("Count"), Object::Int(count)),
    ]));
    for (i, base) in ["Helvetica", "Helvetica-Bold"].iter().enumerate() {
        objects[2 + i] = Object::Dict(Dict(vec![
            (Name::new("Type"), Object::name("Font")),
            (Name::new("Subtype"), Object::name("Type1")),
            (Name::new("BaseFont"), Object::name(base)),
            (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
        ]));
    }
    let mut info = Dict::new();
    info.set("Title", crate::forms::text_object(title));
    info.set("Producer", crate::forms::text_object("Katna PDF"));
    objects.push(Object::Dict(info));
    let info = objects.len() as u32;
    let mut trailer = Dict::new();
    trailer.set("Root", Ref::new(1, 0));
    trailer.set("Info", Ref::new(info, 0));
    write_file(&objects, trailer)
}

fn show(out: &mut String, font: &str, size: f64, x: f64, y: f64, text: &str) {
    use std::fmt::Write as _;
    let _ = writeln!(
        out,
        "BT /{font} {size} Tf {x:.2} {y:.2} Td {} Tj ET",
        literal(text)
    );
}

/// `text` broken at spaces into lines no wider than `width` points; a
/// word wider than that is broken where it must be.
fn wrap(text: &str, width: f64, bold: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || measure(&candidate, bold) <= width {
            line = candidate;
        } else {
            out.push(std::mem::replace(&mut line, word.to_owned()));
        }
        while measure(&line, bold) > width && line.chars().count() > 1 {
            let mut cut = line.len();
            while measure(&line[..cut], bold) > width {
                cut = line[..cut].char_indices().last().map_or(0, |(i, _)| i);
            }
            let cut = cut.max(line.chars().next().map_or(1, char::len_utf8));
            let rest = line.split_off(cut);
            out.push(std::mem::replace(&mut line, rest));
        }
    }
    out.push(line);
    out
}

/// The width of `text` in points at the body size.
fn measure(text: &str, bold: bool) -> f64 {
    let table = if bold { &BOLD } else { &REGULAR };
    let units: u32 = text
        .chars()
        .map(|c| match c {
            ' '..='~' => u32::from(table[c as usize - 32]),
            _ => 556,
        })
        .sum();
    f64::from(units) * SIZE / 1000.0
}

/// Helvetica's widths for ' ' to '~', in thousandths of the size.
const REGULAR: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// Helvetica-Bold's.
const BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    #[test]
    fn reports_are_files_with_their_text() {
        let long = format!("  note: {}", "word ".repeat(60));
        let mut lines = vec!["Signature1: intact".to_owned(), long];
        lines.extend((0..80).map(|i| format!("  line {i} (é)")));
        let pdf = text_pdf("Signature report", &lines);
        let doc = Document::load(pdf).unwrap();
        assert_eq!(doc.pages().len(), 2);
        let text = String::from_utf8_lossy(doc.bytes()).into_owned();
        assert!(text.contains("(Signature1: intact) Tj"));
        assert!(text.contains("(line 79 \\(\\351\\)) Tj"));
    }

    #[test]
    fn long_lines_wrap_inside_the_page() {
        let parts = wrap(&"word ".repeat(60), 400.0, false);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| measure(p, false) <= 400.0));
        let parts = wrap(&"x".repeat(200), 100.0, true);
        assert!(parts.iter().all(|p| measure(p, true) <= 100.0));
        assert_eq!(parts.concat(), "x".repeat(200));
    }
}
