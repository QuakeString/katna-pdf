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
fn words_put_at_the_end_go_on_the_last_line_while_they_fit() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let text = "Katna reads the lines of a paragraph as one block of text to edit. And more \
                words after it.";
    let out = Document::load(doc.edit_block(0, &[0, 1, 2], text).unwrap()).unwrap();
    let got = lines(&out);
    assert!(got[2].0.starts_with("to edit. And"), "{got:?}");
    assert_eq!(out.text_blocks(0)[0].text, text);
}

#[test]
fn a_paragraph_has_its_colour_and_line_spacing() {
    let doc = page(&format!("0 0 1 rg {PARAGRAPH}"), "/F1 5 0 R", &[HELVETICA]);
    let blocks = doc.text_blocks(0);
    assert_eq!(blocks[0].colour, [0, 0, 255]);
    assert!((blocks[0].leading - 14.0).abs() < 0.01, "{blocks:?}");
    assert!((blocks[1].leading - 14.4).abs() < 0.01, "{blocks:?}");
}

#[test]
fn a_paragraph_moves_and_restyles() {
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let out = Document::load(doc.move_block(0, &[0, 1, 2], 10.0, -20.0).unwrap()).unwrap();
    let got = lines(&out);
    assert_eq!(got[0].1, 680.0, "{got:?}");
    assert_eq!(got[2].1, 652.0, "{got:?}");
    assert!((out.text_runs(0)[0].rect[0] - 82.0).abs() < 0.01, "{got:?}");
    // Text after the paragraph stays put.
    assert_eq!(got[3], ("Apart".into(), 600.0));
    assert_eq!(out.text_blocks(0)[0].runs, [0, 1, 2]);

    let out = Document::load(
        doc.style_block(0, &[0, 1, 2], Some(18.0), Some([1.0, 0.0, 0.0]))
            .unwrap(),
    )
    .unwrap();
    let blocks = out.text_blocks(0);
    assert_eq!(blocks[0].size, 18.0, "{}", content(&out));
    assert_eq!(blocks[0].colour, [255, 0, 0]);
    assert_eq!(blocks[0].runs, [0, 1, 2], "{}", content(&out));
    assert!((blocks[0].leading - 21.0).abs() < 0.05, "{blocks:?}");
    assert_eq!(blocks[1].size, 12.0, "{}", content(&out));
    assert_eq!(blocks[1].colour, [0, 0, 0]);
    // Baselines 21 apart (the 14 of 12 pt text, at 18 pt); `lines`
    // takes 12 pt text's descent, so the bigger lines read 1.2 low.
    let got = lines(&out);
    assert!((got[0].1 - 698.8).abs() < 0.05, "{got:?}");
    assert!((got[1].1 - 677.8).abs() < 0.05, "{got:?}");
    assert!((got[2].1 - 656.8).abs() < 0.05, "{got:?}");
    assert_eq!(got[3], ("Apart".into(), 600.0));
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

#[test]
fn a_paragraph_changes_font() {
    use katna_pdf_cos::NewFont;
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let text = doc.text_blocks(0)[0].text.clone();
    // To a standard font: bold, the lines laid out again (wider), the
    // text the same, what follows untouched.
    let bold = NewFont::Standard("Helvetica-Bold".into());
    let out = Document::load(doc.refont_block(0, &[0, 1, 2], &text, &bold).unwrap()).unwrap();
    let blocks = out.text_blocks(0);
    assert_eq!(blocks[0].font, "Helvetica-Bold", "{}", content(&out));
    assert_eq!(blocks[0].text, text, "{}", content(&out));
    assert_eq!(blocks[1].font, "Helvetica");
    let got = lines(&out);
    assert_eq!(got.last().unwrap(), &("Apart".into(), 600.0));
    assert!(
        content(&out).contains("/KatnaF1 12 Tf"),
        "{}",
        content(&out)
    );
    // Bold is wider, so the words wrap again within the old width: no
    // line runs past where the widest did.
    let right = doc.text_blocks(0)[0].rect[2];
    let runs = out.text_runs(0);
    assert_ne!(runs[1].text, "paragraph as one block of text", "{runs:?}");
    for run in &runs[..runs.len() - 1] {
        assert!(run.rect[2] <= right + 0.5, "{right} {runs:?}");
    }

    // To an installed TrueType font, embedded: its glyphs by number,
    // read back as the same text.
    let Ok(data) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") else {
        eprintln!("DejaVu Sans not installed; skipped");
        return;
    };
    let dejavu = NewFont::TrueType { data, face: 0 };
    let out = Document::load(doc.refont_block(0, &[0, 1, 2], &text, &dejavu).unwrap()).unwrap();
    let blocks = out.text_blocks(0);
    assert_eq!(blocks[0].font, "DejaVuSans", "{}", content(&out));
    assert_eq!(blocks[0].text, text, "{}", content(&out));
    assert_eq!(blocks[1].font, "Helvetica");
    // Smaller than the whole font file, which is near 750 KB.
    assert!(out.bytes().len() < 150_000, "{}", out.bytes().len());
    // More typing in it, with a letter the paragraph did not have yet,
    // stays in it.
    let more = format!("{text} Quiz!");
    let again = Document::load(out.edit_block(0, &[0, 1, 2], &more).unwrap()).unwrap();
    let blocks = again.text_blocks(0);
    assert_eq!(blocks[0].text, more, "{}", content(&again));
    assert_eq!(blocks[0].font, "DejaVuSans");
    assert!(!content(&again).contains("/KatnaF"), "{}", content(&again));
    // A font with no TrueType outlines is refused.
    let bad = NewFont::TrueType {
        data: b"OTTO not a font".to_vec(),
        face: 0,
    };
    assert!(matches!(
        doc.refont_block(0, &[0, 1, 2], &text, &bad),
        Err(katna_pdf_cos::EditError::NotTrueType)
    ));
}

#[test]
fn a_paragraph_is_laid_out_again() {
    use katna_pdf_cos::{Layout, TextAlign};
    let doc = page(PARAGRAPH, "/F1 5 0 R", &[HELVETICA]);
    let block = doc.text_blocks(0)[0].clone();
    assert_eq!(block.align, TextAlign::Left);
    assert_eq!(block.char_spacing, 0.0);
    let runs_of = block.runs.clone();

    // Lined up on the right: every line ends where the widest did.
    let right = Layout {
        align: Some(TextAlign::Right),
        ..Layout::default()
    };
    let out = Document::load(doc.layout_block(0, &runs_of, &right).unwrap()).unwrap();
    let blocks = out.text_blocks(0);
    assert_eq!(blocks[0].align, TextAlign::Right, "{}", content(&out));
    let runs = out.text_runs(0);
    for run in &runs[..3] {
        assert!((run.rect[2] - block.rect[2]).abs() < 0.05, "{runs:?}");
    }
    assert_eq!(runs[3].text, "Apart");
    // And in the middle, then on the left again, back where they were.
    let centre = Layout {
        align: Some(TextAlign::Center),
        ..Layout::default()
    };
    let out = Document::load(out.layout_block(0, &runs_of, &centre).unwrap()).unwrap();
    assert_eq!(out.text_blocks(0)[0].align, TextAlign::Center);
    let mid = (block.rect[0] + block.rect[2]) / 2.0;
    for run in &out.text_runs(0)[..3] {
        assert!(
            ((run.rect[0] + run.rect[2]) / 2.0 - mid).abs() < 0.05,
            "{run:?}"
        );
    }
    let left = Layout {
        align: Some(TextAlign::Left),
        ..Layout::default()
    };
    let out = Document::load(out.layout_block(0, &runs_of, &left).unwrap()).unwrap();
    for run in &out.text_runs(0)[..3] {
        assert!((run.rect[0] - 72.0).abs() < 0.05, "{run:?}");
    }

    // Centred, then letters spaced out: still centred. Bigger: still
    // centred.
    let centred = Document::load(doc.layout_block(0, &runs_of, &centre).unwrap()).unwrap();
    let letters = Layout {
        char_spacing: Some(1.0),
        ..Layout::default()
    };
    let out = Document::load(centred.layout_block(0, &runs_of, &letters).unwrap()).unwrap();
    assert_eq!(
        out.text_blocks(0)[0].align,
        TextAlign::Center,
        "{:?}",
        out.text_runs(0)
    );
    for run in &out.text_runs(0)[..3] {
        assert!(
            ((run.rect[0] + run.rect[2]) / 2.0 - mid).abs() < 0.1,
            "{run:?}"
        );
    }
    let out = Document::load(centred.style_block(0, &runs_of, Some(16.0), None).unwrap()).unwrap();
    assert_eq!(
        out.text_blocks(0)[0].align,
        TextAlign::Center,
        "{:?}",
        out.text_runs(0)
    );
    for run in &out.text_runs(0)[..3] {
        assert!(
            ((run.rect[0] + run.rect[2]) / 2.0 - mid).abs() < 0.1,
            "{run:?}"
        );
    }

    // Spaced further apart: 20 pt from one baseline to the next.
    let spaced = Layout {
        leading: Some(20.0),
        ..Layout::default()
    };
    let out = Document::load(doc.layout_block(0, &runs_of, &spaced).unwrap()).unwrap();
    let got = lines(&out);
    assert_eq!(got[0].1, 700.0);
    assert_eq!(got[1].1, 680.0);
    assert_eq!(got[2].1, 660.0);
    assert_eq!(got[3], ("Apart".into(), 600.0));
    assert!((out.text_blocks(0)[0].leading - 20.0).abs() < 0.05);

    // Letters 1 pt apart: the first line ("Katna reads the lines of a",
    // 26 letters) is 26 pt wider, and says so.
    let out = Document::load(doc.layout_block(0, &runs_of, &letters).unwrap()).unwrap();
    let runs = out.text_runs(0);
    let was = doc.text_runs(0)[0].rect[2];
    assert!((runs[0].rect[2] - was - 26.0).abs() < 0.1, "{runs:?}");
    assert!((out.text_blocks(0)[0].char_spacing - 1.0).abs() < 0.01);
    assert_eq!(runs[3].rect, doc.text_runs(0)[3].rect);

    // Words 2 pt further apart: five spaces on the first line.
    let words = Layout {
        word_spacing: Some(2.0),
        ..Layout::default()
    };
    let out = Document::load(doc.layout_block(0, &runs_of, &words).unwrap()).unwrap();
    let runs = out.text_runs(0);
    assert!((runs[0].rect[2] - was - 10.0).abs() < 0.1, "{runs:?}");
    assert!((out.text_blocks(0)[0].word_spacing - 2.0).abs() < 0.01);

    // In a font with two-byte codes `Tw` does nothing, so the spaces are
    // moved past instead; the same width, and 2 pt read back.
    let Ok(data) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") else {
        return;
    };
    let dejavu = katna_pdf_cos::NewFont::TrueType { data, face: 0 };
    let out = Document::load(doc.refont_block(0, &runs_of, &block.text, &dejavu).unwrap()).unwrap();
    let runs_of = out.text_blocks(0)[0].runs.clone();
    let was = out.text_runs(0)[0].rect[2];
    let spaces = out.text_runs(0)[0].text.matches(' ').count() as f32;
    let again = Document::load(out.layout_block(0, &runs_of, &words).unwrap()).unwrap();
    let runs = again.text_runs(0);
    assert_eq!(runs[0].text, out.text_runs(0)[0].text);
    assert!(
        (runs[0].rect[2] - was - 2.0 * spaces).abs() < 0.1,
        "{was} {runs:?}"
    );
    assert!((again.text_blocks(0)[0].word_spacing - 2.0).abs() < 0.01);
    // Set again to 3 pt: 3 pt, not 5.
    let more = Layout {
        word_spacing: Some(3.0),
        ..Layout::default()
    };
    let third = Document::load(again.layout_block(0, &runs_of, &more).unwrap()).unwrap();
    let runs = third.text_runs(0);
    assert!(
        (runs[0].rect[2] - was - 3.0 * spaces).abs() < 0.1,
        "{was} {runs:?}"
    );
}
