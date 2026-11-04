// SPDX-License-Identifier: GPL-3.0-or-later

//! The document's text taken out as plain text or as a Word file, and
//! a PDF made of pictures (a scanned-looking copy, or picture files as
//! pages). The Word file is the least a `.docx` can be: a zip of three
//! XML parts, the paragraphs in the order the pages draw them, bold and
//! italic where the font's name says so, a page break between pages.

use std::fmt::Write as _;

use crate::document::Document;
use crate::edit::{EditError, TextBlock};
use crate::insert::{Image, Insert};
use crate::pages::blank_pdf;

/// The text of `pages` (by index), paragraph by paragraph, a blank line
/// between paragraphs and a form feed between pages.
pub fn plain_text(doc: &Document, pages: &[usize]) -> String {
    let mut out = String::new();
    for (i, &page) in pages.iter().enumerate() {
        if i > 0 {
            out.push('\u{c}');
        }
        for block in doc.text_blocks(page) {
            out.push_str(block.text.trim_end());
            out.push_str("\n\n");
        }
    }
    out
}

/// The text of `pages` as a Word document, paragraph by paragraph.
pub fn docx(doc: &Document, pages: &[usize]) -> Vec<u8> {
    let mut body = String::new();
    for (i, &page) in pages.iter().enumerate() {
        if i > 0 {
            body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#);
        }
        for block in doc.text_blocks(page) {
            body.push_str(&paragraph(&block));
        }
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#
    );
    const TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    zip(&[
        ("[Content_Types].xml", TYPES.as_bytes()),
        ("_rels/.rels", RELS.as_bytes()),
        ("word/document.xml", document.as_bytes()),
    ])
}

/// One paragraph of the Word file, its lines kept apart where the
/// block's text breaks them.
fn paragraph(block: &TextBlock) -> String {
    let lower = block.font.to_ascii_lowercase();
    let bold = lower.contains("bold") || lower.contains("black") || lower.contains("heavy");
    let italic = lower.contains("italic") || lower.contains("oblique");
    let half_points = (block.size * 2.0).round().max(2.0) as u32;
    let justify = match block.align {
        crate::edit::TextAlign::Left => "left",
        crate::edit::TextAlign::Center => "center",
        crate::edit::TextAlign::Right => "right",
    };
    let mut props = format!(r#"<w:rPr><w:sz w:val="{half_points}"/>"#);
    if bold {
        props.push_str("<w:b/>");
    }
    if italic {
        props.push_str("<w:i/>");
    }
    props.push_str("</w:rPr>");
    let mut runs = String::new();
    for (i, line) in block.text.lines().enumerate() {
        if i > 0 {
            runs.push_str("<w:r><w:br/></w:r>");
        }
        let _ = write!(
            runs,
            r#"<w:r>{props}<w:t xml:space="preserve">{}</w:t></w:r>"#,
            escape(line)
        );
    }
    format!(r#"<w:p><w:pPr><w:jc w:val="{justify}"/></w:pPr>{runs}</w:p>"#)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// A zip file of `entries` (name, bytes), stored as they are: what a
/// Word file is, without a zip library.
fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let offset = out.len() as u32;
        let len = data.len() as u32;
        let name = name.as_bytes();
        // Local file header: stored, no date (DOS epoch), sizes known.
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        central.extend_from_slice(&[
            0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0,
        ]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 8]);
        central.extend_from_slice(&[0; 4]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

/// A PDF whose pages are `pictures`, each filling a page of the size
/// given in points.
pub fn pictures_pdf(pictures: Vec<(Image, (f32, f32))>) -> Result<Vec<u8>, EditError> {
    let sizes: Vec<(f32, f32)> = pictures.iter().map(|(_, s)| *s).collect();
    let mut bytes = blank_pdf(&sizes);
    for (page, (image, (w, h))) in pictures.into_iter().enumerate() {
        let doc = Document::load(bytes).map_err(|_| EditError::NoPage)?;
        bytes = doc.insert(
            page,
            &[Insert::Image {
                rect: [0.0, 0.0, w, h],
                image,
            }],
        )?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_entries_are_findable() {
        let z = zip(&[("a.txt", b"hello"), ("d/b.xml", b"<x/>")]);
        assert!(z.starts_with(&[0x50, 0x4b, 0x03, 0x04]));
        assert!(z.ends_with(&[0, 0]));
        let text = String::from_utf8_lossy(&z);
        assert_eq!(text.matches("a.txt").count(), 2, "local and central");
        assert_eq!(text.matches("d/b.xml").count(), 2);
        assert!(text.contains("hello"));
    }

    #[test]
    fn xml_is_escaped() {
        assert_eq!(escape("a < b & \"c\"\u{1}"), "a &lt; b &amp; &quot;c&quot;");
    }
}
