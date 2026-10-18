// SPDX-License-Identifier: GPL-3.0-or-later

//! The command line, run as people and scripts run it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A PDF of `n` pages, page `i` being `100 + i` points wide.
fn pdf(n: usize) -> Vec<u8> {
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", kids.join(" ")),
    ];
    objs.extend((0..n).map(|i| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} 200] >>",
            100 + i
        )
    }));
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

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("katna-pdf-cli-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_katna-pdf-cli"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn files_are_merged_split_and_described() {
    let d = dir("merge");
    let (a, b) = (d.join("a.pdf"), d.join("b.pdf"));
    std::fs::write(&a, pdf(3)).unwrap();
    std::fs::write(&b, pdf(2)).unwrap();
    let out = d.join("out.pdf");
    let a_pages = format!("{}:3-2", s(&a));
    let (code, text) = run(&["merge", "--json", "-o", s(&out), &a_pages, s(&b)]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("\"pages\":4"), "{text}");
    let (code, text) = run(&["info", "--json", s(&out)]);
    assert_eq!(code, 0);
    // The third page of a, then the second, then all of b.
    assert!(
        text.contains(
            "\"page_sizes\":[{\"width\":102,\"height\":200,\"rotate\":0},{\"width\":101,"
        ),
        "{text}"
    );
    let parts = d.join("parts");
    let (code, text) = run(&["split", s(&out), "-o", s(&parts), "--every", "3"]);
    assert_eq!(code, 0, "{text}");
    assert!(parts.join("out-1.pdf").exists() && parts.join("out-2.pdf").exists());
    let turned = d.join("turned.pdf");
    let (code, _) = run(&["rotate", s(&a), "-o", s(&turned), "--pages", "1"]);
    assert_eq!(code, 0);
    let (_, text) = run(&["info", "--json", s(&turned)]);
    assert!(
        text.contains("{\"width\":200,\"height\":100,\"rotate\":90}"),
        "{text}"
    );
    std::fs::remove_dir_all(d).ok();
}

#[test]
fn mistakes_are_told_apart_from_failures() {
    let (code, text) = run(&["info", "--json", "/no/such.pdf"]);
    assert_eq!(code, 1);
    assert!(text.starts_with("{\"error\":"));
    assert_eq!(run(&["merge", "x.pdf"]).0, 2);
    assert_eq!(run(&["frobnicate"]).0, 2);
}
