// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing the text a page draws.

use std::io::Write as _;

use katna_pdf_cos::{Document, Object, decode};

/// A file of `objs`, object 1 first, with a classic table.
fn file(objs: &[String]) -> Vec<u8> {
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
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out
}

/// One page drawing `content` with fonts `fonts` (objects 5 on).
fn page(content: &str, fonts: &str, more: &[&str]) -> Document {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 600 800] >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << {fonts} >> >> >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
    ];
    objs.extend(more.iter().map(|s| s.to_string()));
    Document::load(file(&objs)).unwrap()
}

const HELVETICA: &str =
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

fn texts(doc: &Document) -> Vec<String> {
    doc.text_runs(0).into_iter().map(|r| r.text).collect()
}

/// The page's content, decoded.
fn content(doc: &Document) -> String {
    let page = doc.get(doc.pages()[0]).unwrap();
    let r = page.as_dict().unwrap().get("Contents").unwrap().clone();
    let Object::Stream(s) = doc.resolve(&r) else {
        panic!("no content stream");
    };
    String::from_utf8_lossy(&decode(&s.dict, &s.data).unwrap()).into_owned()
}

#[test]
fn reads_lines_of_text() {
    let doc = page(
        "BT /F1 12 Tf 72 700 Td (Hello) Tj [( W) 20 (orld)] TJ 0 -14 Td (Second line) Tj ET \
         BT 3 Tr /F1 12 Tf 72 600 Td (hidden) Tj ET",
        "/F1 5 0 R",
        &[HELVETICA],
    );
    let runs = doc.text_runs(0);
    assert_eq!(texts(&doc), ["Hello World", "Second line"]);
    assert_eq!(runs[0].font, "Helvetica");
    assert_eq!(runs[0].size, 12.0);
    let r = runs[0].rect;
    assert!(
        (r[0] - 72.0).abs() < 0.01 && (r[1] - (700.0 - 2.4)).abs() < 0.01,
        "{r:?}"
    );
    // Helvetica's widths: "Hello World" is 59.34 em-thousandths wide at
    // 12 pt, less the kerning of 20.
    assert!((r[2] - (72.0 + 62.004 - 0.24)).abs() < 0.05, "{r:?}");
}

#[test]
fn changes_only_what_changed() {
    let doc = page(
        "BT /F1 12 Tf 72 700 Td [(Hel) 30 (lo W) 50 (orld)] TJ ET",
        "/F1 5 0 R",
        &[HELVETICA],
    );
    let out = Document::load(doc.edit_text(0, &[(0, "Hello Katna".into())]).unwrap()).unwrap();
    assert_eq!(texts(&out), ["Hello Katna"]);
    // The kerning before the change is kept.
    assert!(
        content(&out).contains("[(Hel) 30 (lo Katna)] TJ"),
        "{}",
        content(&out)
    );
    // The original stays in the file, as an incremental update keeps it.
    assert!(out.bytes().starts_with(doc.bytes()));
}

#[test]
fn text_further_along_the_line_moves() {
    let doc = page(
        "BT /F1 12 Tf 72 700 Td (Total:) Tj ET BT /F2 12 Tf 120 700 Td (500) Tj ET",
        "/F1 5 0 R /F2 6 0 R",
        &[
            HELVETICA,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
        ],
    );
    let before = doc.text_runs(0)[1].rect;
    let out = Document::load(doc.edit_text(0, &[(0, "Grand total:".into())]).unwrap()).unwrap();
    assert_eq!(texts(&out), ["Grand total:", "500"]);
    let after = out.text_runs(0)[1].rect;
    // "Grand " in Helvetica, and "t" for "T": 2.889 + 2.889 ... at 12 pt.
    let grown = out.text_runs(0)[0].rect[2] - doc.text_runs(0)[0].rect[2];
    assert!(grown > 30.0);
    assert!(
        (after[0] - before[0] - grown).abs() < 0.01,
        "{before:?} {after:?}"
    );
}

#[test]
fn letters_a_subset_lacks_come_from_a_standard_font() {
    // A subset with only H, e, l and o, as its character set says.
    let doc = page(
        "BT /F1 20 Tf 72 700 Td (Hello) Tj ET",
        "/F1 5 0 R",
        &[
            "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+Times-Bold /FirstChar 72 \
             /LastChar 111 /Widths 6 0 R /FontDescriptor 7 0 R /Encoding /WinAnsiEncoding >>",
            &format!(
                "[{}]",
                (72..=111).map(|_| "500").collect::<Vec<_>>().join(" ")
            ),
            "<< /Type /FontDescriptor /FontName /ABCDEF+Times-Bold /CharSet (/H/e/l/o) \
             /FontFile3 8 0 R >>",
            "<< /Length 0 >>\nstream\n\nendstream",
        ],
    );
    let out = Document::load(doc.edit_text(0, &[(0, "Hello Ollie".into())]).unwrap()).unwrap();
    let all: String = out
        .text_runs(0)
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("|");
    // "O" and "i" are not in the subset: a stand-in writes them, and the
    // space, with no glyph either, is a gap.
    assert_eq!(all.replace('|', ""), "Hello Ollie");
    let c = content(&out);
    assert!(
        c.contains("[(Hello) -250] TJ /KatnaF1 20 Tf [(O)] TJ /F1 20 Tf [(ll)] TJ"),
        "{c}"
    );
    let page = out.get(out.pages()[0]).unwrap();
    let res = page
        .as_dict()
        .unwrap()
        .get("Resources")
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
    let fonts = res.get("Font").unwrap().as_dict().unwrap().clone();
    let stand_in = out.resolve(fonts.get("KatnaF1").unwrap());
    assert_eq!(
        stand_in.as_dict().unwrap().name("BaseFont"),
        Some(&b"Times-Bold"[..])
    );
}

#[test]
fn composite_fonts_are_written_in_their_codes() {
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
        1 begincodespacerange <0000> <FFFF> endcodespacerange \
        2 beginbfchar <0001> <0041> <0002> <0042> endbfchar \
        endcmap CMapName currentdict /CMap defineresource pop end end";
    let doc = page(
        "BT /F1 10 Tf 72 700 Td <00010002> Tj ET",
        "/F1 5 0 R",
        &[
            "<< /Type /Font /Subtype /Type0 /BaseFont /Noto /Encoding /Identity-H \
             /DescendantFonts [6 0 R] /ToUnicode 7 0 R >>",
            "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Noto /DW 600 \
             /W [1 [700 800]] >>",
            &format!(
                "<< /Length {} >>\nstream\n{cmap}\nendstream",
                cmap.len() + 1
            ),
        ],
    );
    assert_eq!(texts(&doc), ["AB"]);
    assert!((doc.text_runs(0)[0].rect[2] - (72.0 + 15.0)).abs() < 0.01);
    let out = Document::load(doc.edit_text(0, &[(0, "BAB".into())]).unwrap()).unwrap();
    assert_eq!(texts(&out), ["BAB"]);
    assert!(
        content(&out).contains("[<000200010002>] TJ"),
        "{}",
        content(&out)
    );
}

#[test]
fn characters_no_font_has_are_refused() {
    let doc = page(
        "BT /F1 12 Tf 72 700 Td (Hi) Tj ET",
        "/F1 5 0 R",
        &[HELVETICA],
    );
    let err = doc.edit_text(0, &[(0, "Hi नमस्ते".into())]).unwrap_err();
    assert!(err.to_string().contains("न"), "{err}");
}

const PARAGRAPH: &str = "BT /F1 12 Tf 14 TL 72 700 Td (Katna reads the lines of a) Tj \
     T* (paragraph as one block of text) Tj T* (to edit.) Tj ET \
     BT /F1 12 Tf 72 600 Td (Apart) Tj ET";

/// Each run's text and baseline.
fn lines(doc: &Document) -> Vec<(String, f32)> {
    doc.text_runs(0)
        .into_iter()
        .map(|r| (r.text, ((r.rect[1] + 2.4) * 10.0).round() / 10.0))
        .collect()
}

#[test]
fn lines_under_one_another_are_a_paragraph() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let blocks = doc.text_blocks(0);
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0].runs, [0, 1, 2]);
    assert_eq!(
        blocks[0].text,
        "Katna reads the lines of a paragraph as one block of text to edit."
    );
    assert_eq!(blocks[1].text, "Apart");
}

#[test]
fn a_paragraph_rewraps_to_its_width() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let text = "Katna reads the lines of a paragraph as one block of text, so a word \
                put in wraps on to the lines below it.";
    let out = Document::load(doc.edit_block(0, &[0, 1, 2], text).unwrap()).unwrap();
    let got = lines(&out);
    // The first line is as it was; the rest wrap, a line more under it.
    assert_eq!(got[0], ("Katna reads the lines of a".into(), 700.0));
    let joined: Vec<&str> = got[..got.len() - 1].iter().map(|l| l.0.as_str()).collect();
    assert_eq!(joined.join(" "), text);
    for (j, line) in got[..got.len() - 1].iter().enumerate() {
        assert!((line.1 - (700.0 - 14.0 * j as f32)).abs() < 0.05, "{got:?}");
    }
    assert!(got.len() > 4, "{got:?}");
    let right = doc.text_runs(0)[1].rect[2].max(doc.text_runs(0)[0].rect[2]);
    for run in &out.text_runs(0) {
        assert!(run.rect[2] <= right + 1.0, "{run:?}");
    }
    // The text after the paragraph is where it was.
    assert_eq!(got.last().unwrap(), &("Apart".into(), 600.0));
    assert_eq!(out.text_blocks(0)[0].text, text);
}

#[test]
fn a_shorter_paragraph_empties_lines() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let out = Document::load(doc.edit_block(0, &[0, 1, 2], "Short now.").unwrap()).unwrap();
    assert_eq!(
        lines(&out),
        [("Short now.".into(), 700.0), ("Apart".into(), 600.0)]
    );
}

#[test]
fn a_line_break_starts_a_new_line() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let out = Document::load(doc.edit_block(0, &[3], "Apart\nand below").unwrap()).unwrap();
    let got = lines(&out);
    assert_eq!(
        got[3..],
        [("Apart".into(), 600.0), ("and below".into(), 585.6)]
    );
    // Text drawn after it in the same text object is where it was.
    let doc = page(
        "BT /F1 12 Tf 72 600 Td (One) Tj 0 -20 Td (Two) Tj ET",
        "/F1 5 0 R",
        &[HELVETICA],
    );
    let out = Document::load(doc.edit_block(0, &[0], "One\nmore").unwrap()).unwrap();
    assert_eq!(
        lines(&out),
        [
            ("One".into(), 600.0),
            ("more".into(), 585.6),
            ("Two".into(), 580.0)
        ]
    );
}

#[test]
fn lines_lined_up_on_the_right_stay_so() {
    // Each line ends at 400: "Hi there" and "you" in Helvetica at 10 pt.
    let doc = page(
        "BT /F1 10 Tf 364.99 700 Td (Hi there) Tj ET BT /F1 10 Tf 383.88 688 Td (you) Tj ET",
        "/F1 5 0 R",
        &[HELVETICA],
    );
    let blocks = doc.text_blocks(0);
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    let out = Document::load(doc.edit_block(0, &blocks[0].runs, "Hi you there").unwrap()).unwrap();
    let runs = out.text_runs(0);
    let texts: Vec<&str> = runs.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(texts, ["Hi you", "there"]);
    for run in &runs {
        assert!((run.rect[2] - 400.0).abs() < 0.05, "{runs:?}");
    }
}
