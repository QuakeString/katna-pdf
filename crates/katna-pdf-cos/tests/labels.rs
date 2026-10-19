// SPDX-License-Identifier: GPL-3.0-or-later

//! Page labels: read, written and worked out.

use std::io::Write as _;

use katna_pdf_cos::{Document, LabelRange, LabelStyle, NavError};

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

/// `n` pages, the catalog given `extra`.
fn pages(n: usize, extra: &str, more: &[&str]) -> Document {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {extra} >>"),
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", kids.join(" ")),
    ];
    objs.extend((0..n).map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".into()));
    objs.extend(more.iter().map(|m| m.to_string()));
    Document::load(file(&objs)).unwrap()
}

#[test]
fn labels_are_read_from_a_tree_with_kids() {
    // Objects 1-2, pages 3-9, then the tree's kids at 10 and 11.
    let doc = pages(
        7,
        "/PageLabels << /Kids [10 0 R 11 0 R] >>",
        &[
            "<< /Limits [0 2] /Nums [0 << /S /r >> 2 << /S /D /St 3 >>] >>",
            "<< /Limits [4 6] /Nums [4 << /S /A /P (App-) >> 6 << /P (Back) >>] >>",
        ],
    );
    assert_eq!(
        doc.page_labels().unwrap(),
        ["i", "ii", "3", "4", "App-A", "App-B", "Back"]
    );
    assert!(pages(2, "", &[]).page_labels().is_none());
}

#[test]
fn labels_are_written_and_taken_out() {
    let doc = pages(30, "", &[]);
    let ranges = vec![
        LabelRange {
            start: 0,
            style: LabelStyle::UpperRoman,
            prefix: String::new(),
            first: 1,
        },
        LabelRange {
            start: 3,
            style: LabelStyle::Decimal,
            prefix: String::new(),
            first: 1,
        },
        LabelRange {
            start: 1,
            style: LabelStyle::LowerLetters,
            prefix: "Annexe ".into(),
            first: 25,
        },
    ];
    let out = Document::load(doc.set_page_labels(&ranges).unwrap()).unwrap();
    let mut sorted = ranges.clone();
    sorted.sort_by_key(|r| r.start);
    assert_eq!(out.page_label_ranges(), sorted);
    let labels = out.page_labels().unwrap();
    assert_eq!(labels[..5], ["I", "Annexe y", "Annexe z", "1", "2"]);
    assert_eq!(labels[29], "27");
    // Starting past page 1, the pages before are numbered plainly.
    let late = Document::load(doc.set_page_labels(&sorted[2..]).unwrap()).unwrap();
    assert_eq!(late.page_labels().unwrap()[..5], ["1", "2", "3", "1", "2"]);
    let gone = Document::load(out.set_page_labels(&[]).unwrap()).unwrap();
    assert!(gone.page_labels().is_none());
    let mut past = sorted.clone();
    past[2].start = 30;
    assert_eq!(doc.set_page_labels(&past), Err(NavError::NoPage(30)));
}

#[test]
fn numbers_are_written_as_people_expect() {
    let range = |style| LabelRange {
        start: 0,
        style,
        prefix: String::new(),
        first: 1,
    };
    let r = range(LabelStyle::UpperRoman);
    assert_eq!(
        [0, 3, 8, 13, 39, 48, 1993].map(|n| r.label(n)),
        ["I", "IV", "IX", "XIV", "XL", "XLIX", "MCMXCIV"]
    );
    let a = range(LabelStyle::LowerLetters);
    assert_eq!(
        [0, 25, 26, 27, 52].map(|n| a.label(n)),
        ["a", "z", "aa", "bb", "aaa"]
    );
    assert_eq!(range(LabelStyle::None).label(4), "");
}
