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
    pdf(title, lines, false)
}

/// [`text_pdf`] with its headings as the file's bookmarks, open beside
/// the pages: for longer texts read on screen, such as the user guide.
pub fn text_pdf_outlined(title: &str, lines: &[String]) -> Vec<u8> {
    pdf(title, lines, true)
}

fn pdf(title: &str, lines: &[String], outlined: bool) -> Vec<u8> {
    let mut pages: Vec<String> = Vec::new();
    // Each heading, the page it is on and how far up.
    let mut headings: Vec<(String, usize, f64)> = Vec::new();
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
            if heading && i == 0 {
                headings.push((line.clone(), pages.len(), y + LEADING));
            }
            // Wrapped parts hang under the text, past a label's colon or
            // a bullet; a guide's paragraphs wrap flush.
            let bullet = line.trim_start().starts_with('\u{2022}');
            let hang = if i > 0 && (!outlined || bullet) {
                SIZE
            } else {
                0.0
            };
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
    let mut catalog = vec![
        (Name::new("Type"), Object::name("Catalog")),
        (Name::new("Pages"), Object::Ref(Ref::new(2, 0))),
    ];
    if outlined && !headings.is_empty() {
        let root = objects.len() as u32 + 1;
        let first = root + 1;
        let last = root + headings.len() as u32;
        objects.push(Object::Dict(Dict(vec![
            (Name::new("Type"), Object::name("Outlines")),
            (Name::new("First"), Object::Ref(Ref::new(first, 0))),
            (Name::new("Last"), Object::Ref(Ref::new(last, 0))),
            (Name::new("Count"), Object::Int(headings.len() as i64)),
        ])));
        for (i, (words, on, top)) in headings.iter().enumerate() {
            let me = first + i as u32;
            let mut item = Dict(vec![
                (Name::new("Title"), crate::forms::text_object(words)),
                (Name::new("Parent"), Object::Ref(Ref::new(root, 0))),
                (
                    Name::new("Dest"),
                    Object::Array(vec![
                        kids[*on].clone(),
                        Object::name("XYZ"),
                        Object::Null,
                        Object::Real(*top),
                        Object::Null,
                    ]),
                ),
            ]);
            if me > first {
                item.set("Prev", Ref::new(me - 1, 0));
            }
            if me < last {
                item.set("Next", Ref::new(me + 1, 0));
            }
            objects.push(Object::Dict(item));
        }
        catalog.push((Name::new("Outlines"), Object::Ref(Ref::new(root, 0))));
        catalog.push((Name::new("PageMode"), Object::name("UseOutlines")));
    }
    objects[0] = Object::Dict(Dict(catalog));
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
        win_ansi(text)
    );
}

/// `text` as a PDF string in WinAnsi: Latin-1 as it is, and the
/// punctuation WinAnsi keeps at 0x80 to 0x9f (bullets, dashes, curly
/// quotes, the ellipsis) at its codes there; anything else is `?`.
fn win_ansi(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| match c {
            '\u{2022}' => '\u{95}',
            '\u{2013}' => '\u{96}',
            '\u{2014}' => '\u{97}',
            '\u{2018}' => '\u{91}',
            '\u{2019}' => '\u{92}',
            '\u{201c}' => '\u{93}',
            '\u{201d}' => '\u{94}',
            '\u{2026}' => '\u{85}',
            '\u{80}'..='\u{9f}' => '?',
            c => c,
        })
        .collect();
    let mut out = String::from("(");
    for c in mapped.chars() {
        match c {
            '\u{80}'..='\u{9f}' => out.push_str(&format!("\\{:03o}", c as u32)),
            c => {
                let one = literal(&c.to_string());
                out.push_str(&one[1..one.len() - 1]);
            }
        }
    }
    out.push(')');
    out
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
    fn headings_become_bookmarks() {
        let mut lines = vec!["Opening".to_owned()];
        lines.extend((0..70).map(|i| format!("  line {i}")));
        lines.push("Signing".to_owned());
        lines.push("  a line".to_owned());
        let doc = Document::load(text_pdf_outlined("Guide", &lines)).unwrap();
        let outline = doc.outline();
        let titles: Vec<_> = outline.iter().map(|o| o.title.as_str()).collect();
        assert_eq!(titles, ["Opening", "Signing"]);
        let pages: Vec<_> = outline
            .iter()
            .map(|o| o.dest.as_ref().unwrap().page)
            .collect();
        assert_eq!(pages, [0, 1]);
        let bullet = text_pdf_outlined("Guide", &["  \u{2022} one".to_owned()]);
        assert!(String::from_utf8_lossy(&bullet).contains("(\\225 one) Tj"));
        let plain = Document::load(text_pdf("Report", &lines)).unwrap();
        assert!(plain.outline().is_empty());
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
