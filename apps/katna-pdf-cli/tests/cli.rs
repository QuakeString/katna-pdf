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
fn pages_are_cropped_and_uncropped() {
    let d = dir("crop");
    let a = d.join("a.pdf");
    std::fs::write(&a, pdf(2)).unwrap();
    let boxed = d.join("boxed.pdf");
    let (code, text) = run(&["crop", s(&a), "-o", s(&boxed), "--box", "10,20,60,120"]);
    assert_eq!(code, 0, "{text}");
    let (_, text) = run(&["info", "--json", s(&boxed)]);
    assert!(text.contains("{\"width\":50,\"height\":100,"), "{text}");
    // Margins come off what is shown, on the second page only.
    let cut = d.join("cut.pdf");
    let (code, _) = run(&[
        "crop",
        s(&boxed),
        "-o",
        s(&cut),
        "--margins",
        "10,0,0,5",
        "--pages",
        "2",
    ]);
    assert_eq!(code, 0);
    let (_, text) = run(&["info", "--json", s(&cut)]);
    assert!(text.contains("{\"width\":45,\"height\":90,"), "{text}");
    let back = d.join("back.pdf");
    assert_eq!(run(&["crop", s(&cut), "-o", s(&back), "--uncrop"]).0, 0);
    let (_, text) = run(&["info", "--json", s(&back)]);
    assert!(text.contains("{\"width\":100,\"height\":200,"), "{text}");
    assert!(text.contains("{\"width\":101,\"height\":200,"), "{text}");
    assert_eq!(run(&["crop", s(&a), "-o", s(&back)]).0, 2);
    assert_eq!(
        run(&["crop", s(&a), "-o", s(&back), "--box", "5,5,1,1"]).0,
        2
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

#[test]
fn files_are_signed_and_verified() {
    let d = dir("sign");
    let input = d.join("in.pdf");
    std::fs::write(&input, pdf(2)).unwrap();
    let p12 = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/katna-pdf-sign/tests/data/rsa-aes.p12"
    );
    let signed = d.join("signed.pdf");
    let (code, _) = run(&[
        "sign",
        s(&input),
        "-o",
        s(&signed),
        "--p12",
        p12,
        "--password",
        "katna",
        "--page",
        "2",
        "--rect",
        "10,10,90,60",
        "--reason",
        "Checked",
    ]);
    assert_eq!(code, 0);
    let (code, out) = run(&["verify", "--json", s(&signed)]);
    assert_eq!(code, 0);
    assert!(out.contains("\"intact\":true"), "{out}");
    assert!(out.contains("\"signer\":\"Asha Verma\""), "{out}");
    assert!(out.contains("\"page\":2"), "{out}");
    assert!(out.contains("\"reason\":\"Checked\""), "{out}");
    assert!(out.contains("\"covers\":\"whole\""), "{out}");
    assert!(
        out.contains(
            "\"trust\":{\"state\":\"untrusted-root\",\"certificate\":\"Katna Test Root\"}"
        ),
        "{out}"
    );
    let ca = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/katna-pdf-sign/tests/data/ca.pem"
    );
    let (code, out) = run(&["verify", s(&signed), "--trust", ca]);
    assert_eq!(code, 0);
    assert!(
        out.contains("identity: vouched for by Katna Test Root"),
        "{out}"
    );
    // The same, as a PDF to keep or send on.
    let report = d.join("report.pdf");
    let (code, out) = run(&[
        "verify",
        "--json",
        s(&signed),
        "--trust",
        ca,
        "--report",
        s(&report),
    ]);
    assert_eq!(code, 0);
    assert!(out.starts_with("{\"signatures\":"), "{out}");
    let bytes = std::fs::read(&report).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("(Signature report) Tj"));
    assert!(text.contains("(signed by: Asha Verma) Tj"), "{text}");
    assert!(text.contains("(identity: vouched for by Katna Test Root) Tj"));
    assert_eq!(
        katna_pdf_cos::Document::load(bytes.clone())
            .unwrap()
            .pages()
            .len(),
        1
    );

    let (code, _) = run(&[
        "sign",
        s(&input),
        "-o",
        s(&signed),
        "--p12",
        p12,
        "--password",
        "nope",
    ]);
    assert_eq!(code, 1);
    let (code, _) = run(&["sign", s(&input), "-o", s(&signed), "--p12", p12]);
    assert_eq!(code, 2);
    let (code, out) = run(&["verify", s(&input)]);
    assert_eq!((code, out.as_str()), (0, "no signatures\n"));

    // Many files at once, into a folder; one that is not a PDF does not
    // stop the others.
    let bad = d.join("bad.pdf");
    std::fs::write(&bad, "nope").unwrap();
    let folder = d.join("signed");
    let (code, _) = run(&[
        "sign",
        s(&input),
        s(&signed),
        s(&bad),
        "-o",
        s(&folder),
        "--p12",
        p12,
        "--password",
        "katna",
    ]);
    assert_eq!(code, 1);
    for name in ["in.pdf", "signed.pdf"] {
        let (code, out) = run(&["verify", "--json", s(&folder.join(name))]);
        assert_eq!(code, 0);
        assert!(out.contains("\"intact\":true"), "{name}: {out}");
    }
    assert!(!folder.join("bad.pdf").exists());
    // Not over the files themselves.
    let (code, _) = run(&[
        "sign",
        s(&input),
        s(&signed),
        "-o",
        s(&d),
        "--p12",
        p12,
        "--password",
        "katna",
    ]);
    assert_eq!(code, 2);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn files_are_certified_with_fields_for_others() {
    let d = dir("certify");
    let input = d.join("in.pdf");
    std::fs::write(&input, pdf(2)).unwrap();
    let data = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/katna-pdf-sign/tests/data/"
    );
    let (rsa, ec) = (format!("{data}rsa-aes.p12"), format!("{data}ec-p256.p12"));
    let (form, certified, signed) = (d.join("form.pdf"), d.join("cert.pdf"), d.join("both.pdf"));
    let sign = |input: &Path, out: &Path, p12: &str, more: &[&str]| {
        let mut args = vec![
            "sign",
            s(input),
            "-o",
            s(out),
            "--p12",
            p12,
            "--password",
            "katna",
        ];
        args.extend(more);
        run(&args).0
    };
    let (code, _) = run(&[
        "add-field",
        s(&input),
        "-o",
        s(&form),
        "--page",
        "2",
        "--rect",
        "10,10,90,60",
        "--name",
        "Approver",
    ]);
    assert_eq!(code, 0);
    assert_eq!(sign(&form, &certified, &rsa, &["--certify", "forms"]), 0);
    // Not in a box of one's own: in the field left for it.
    assert_eq!(sign(&certified, &signed, &ec, &[]), 1);
    assert_eq!(sign(&certified, &signed, &ec, &["--field", "Approver"]), 0);
    let (code, out) = run(&["verify", "--json", s(&signed)]);
    assert_eq!(code, 0);
    assert!(
        out.contains("\"certifies\":{\"allows\":\"forms\",\"forbidden_changes\":[]}"),
        "{out}"
    );
    assert!(!out.contains("\"intact\":false"), "{out}");
    assert_eq!(sign(&form, &certified, &rsa, &["--certify", "all"]), 2);
    let _ = std::fs::remove_dir_all(d);
}

/// SoftHSM, a token in software, where it is installed (CI installs
/// it on Linux): signing with a token goes through it as through an
/// ePass2003's driver.
const SOFTHSM: &str = "/usr/lib/softhsm/libsofthsm2.so";

#[test]
fn files_are_signed_with_a_token() {
    let tools = ["softhsm2-util", "pkcs11-tool"];
    if !Path::new(SOFTHSM).is_file()
        || tools
            .iter()
            .any(|t| Command::new(t).arg("--help").output().is_err())
    {
        eprintln!("SoftHSM is not installed; token signing not tried");
        return;
    }
    let d = dir("token");
    std::fs::create_dir_all(d.join("tokens")).unwrap();
    let conf = d.join("softhsm2.conf");
    std::fs::write(
        &conf,
        format!(
            "directories.tokendir = {}\nobjectstore.backend = file\n",
            d.join("tokens").display()
        ),
    )
    .unwrap();
    let data = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/katna-pdf-sign/tests/data"
    );
    let ok = |c: &mut Command| assert!(c.env("SOFTHSM2_CONF", &conf).status().unwrap().success());
    ok(Command::new("softhsm2-util").args([
        "--init-token",
        "--free",
        "--label",
        "Katna Test Token",
        "--so-pin",
        "87654321",
        "--pin",
        "123456",
    ]));
    for (file, kind, id) in [
        ("asha-key.p8", "privkey", "01"),
        ("asha.der", "cert", "01"),
        ("ca.der", "cert", "02"),
    ] {
        ok(Command::new("pkcs11-tool").args([
            "--module",
            SOFTHSM,
            "--login",
            "--pin",
            "123456",
            "--write-object",
            &format!("{data}/{file}"),
            "--type",
            kind,
            "--id",
            id,
        ]));
    }
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_katna-pdf-cli"))
            .args(args)
            .env("SOFTHSM2_CONF", &conf)
            .output()
            .unwrap();
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        )
    };
    let (code, out) = run(&["tokens", "--json", "--module", SOFTHSM]);
    assert_eq!(code, 0);
    assert!(out.contains("\"label\":\"Katna Test Token\""), "{out}");
    // The signing certificate, not the authority's beside it.
    assert!(
        out.contains("\"certificates\":[{\"name\":\"Asha Verma\",\"issuer\":\"Katna Test Root\""),
        "{out}"
    );
    assert_eq!(out.matches("\"name\":").count(), 1, "{out}");
    // The setup, step by step: the driver sees the token.
    let out = Command::new(env!("CARGO_BIN_EXE_katna-pdf-cli"))
        .args(["token-check", "--json"])
        .env("SOFTHSM2_CONF", &conf)
        .env("KATNA_PDF_PKCS11", SOFTHSM)
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.contains("\"tokens\":[{\"label\":\"Katna Test Token\",\"certificates\":1}]"),
        "{out}"
    );

    let input = d.join("in.pdf");
    std::fs::write(&input, pdf(1)).unwrap();
    let signed = d.join("signed.pdf");
    let sign = |pin: &str| {
        run(&[
            "sign",
            s(&input),
            "-o",
            s(&signed),
            "--token",
            "Katna Test Token",
            "--module",
            SOFTHSM,
            "--password",
            pin,
        ])
    };
    assert_eq!(sign("000000").0, 1);
    assert_eq!(sign("123456").0, 0);
    let (code, out) = run(&["verify", "--json", s(&signed)]);
    assert_eq!(code, 0);
    assert!(out.contains("\"intact\":true"), "{out}");
    assert!(out.contains("\"signer\":\"Asha Verma\""), "{out}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn text_is_listed_and_changed() {
    let d = dir("text");
    let file = d.join("letter.pdf");
    let lines = [
        "Dear Asha,".to_string(),
        "  The total due is 500 rupees.".to_string(),
    ];
    std::fs::write(&file, katna_pdf_cos::text_pdf("Letter", &lines)).unwrap();
    let (code, out) = run(&["text", s(&file)]);
    assert_eq!(code, 0);
    assert!(
        out.contains("1:2      Helvetica-Bold 10pt: Dear Asha,"),
        "{out}"
    );
    let (code, out) = run(&["text", "--json", s(&file)]);
    assert_eq!(code, 0);
    assert!(
        out.contains("\"run\":\"1:3\",\"font\":\"Helvetica\""),
        "{out}"
    );

    let changed = d.join("changed.pdf");
    let (code, out) = run(&[
        "edit-text",
        s(&file),
        "-o",
        s(&changed),
        "--replace",
        "500",
        "--to",
        "750",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("1 run changed"), "{out}");
    let (_, out) = run(&["text", s(&changed)]);
    assert!(out.contains("The total due is 750 rupees."), "{out}");

    let again = d.join("again.pdf");
    let (code, _) = run(&[
        "edit-text",
        s(&changed),
        "-o",
        s(&again),
        "--run",
        "1:2",
        "--to",
        "Dear Ravi,",
    ]);
    assert_eq!(code, 0);
    let (_, out) = run(&["text", s(&again)]);
    assert!(out.contains("Dear Ravi,") && out.contains("750"), "{out}");

    let (code, _) = run(&[
        "edit-text",
        s(&file),
        "-o",
        s(&again),
        "--run",
        "9:1",
        "--to",
        "x",
    ]);
    assert_eq!(code, 2);
    let (code, _) = run(&[
        "edit-text",
        s(&file),
        "-o",
        s(&again),
        "--replace",
        "nowhere",
        "--to",
        "x",
    ]);
    assert_eq!(code, 1);
}

#[test]
fn a_paragraph_is_changed_and_wrapped() {
    let d = dir("block");
    let file = d.join("note.pdf");
    let lines = [
        "A note".to_string(),
        "  The first line of a short note".to_string(),
        "  and its second line.".to_string(),
    ];
    std::fs::write(&file, katna_pdf_cos::text_pdf("Note", &lines)).unwrap();
    let (code, out) = run(&["text", "--blocks", s(&file)]);
    assert_eq!(code, 0);
    assert!(
        out.contains(
            "1:3      Helvetica 10pt, 2 lines: The first line of a short note and its second line."
        ),
        "{out}"
    );
    let changed = d.join("changed.pdf");
    let (code, out) = run(&[
        "edit-text",
        s(&file),
        "-o",
        s(&changed),
        "--block",
        "1:3",
        "--to",
        "The first line of a short note, now longer, wraps on to its second line\nand a third.",
    ]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["text", s(&changed)]);
    // Wrapped to the width of the longest line it had.
    assert!(
        out.contains("1:4      Helvetica 10pt: note, now longer, wraps on"),
        "{out}"
    );
    assert!(
        out.contains("1:6      Helvetica 10pt: and a third."),
        "{out}"
    );
}

#[test]
fn text_and_shapes_are_drawn_into_a_page() {
    let d = dir("insert");
    let file = d.join("note.pdf");
    std::fs::write(
        &file,
        katna_pdf_cos::text_pdf("Note", &["  Hello".to_string()]),
    )
    .unwrap();
    let with_text = d.join("text.pdf");
    let (code, out) = run(&[
        "insert",
        s(&file),
        "-o",
        s(&with_text),
        "--text",
        "Paid\\nThank you",
        "--at",
        "72,300",
        "--font",
        "Times-Roman",
        "--colour",
        "#cc0000",
    ]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["text", s(&with_text)]);
    assert!(out.contains("Times-Roman 12pt: Paid"), "{out}");
    assert!(out.contains("Times-Roman 12pt: Thank you"), "{out}");
    let with_shape = d.join("shape.pdf");
    let (code, out) = run(&[
        "insert",
        s(&with_text),
        "-o",
        s(&with_shape),
        "--shape",
        "ellipse",
        "--rect",
        "60,280,200,320",
    ]);
    assert_eq!(code, 0, "{out}");
    let (code, _) = run(&[
        "insert",
        s(&file),
        "-o",
        s(&with_shape),
        "--shape",
        "circle",
        "--rect",
        "0,0,1,1",
    ]);
    assert_ne!(code, 0);
}

#[test]
fn objects_are_listed_moved_and_taken_out() {
    let d = dir("objects");
    let file = d.join("note.pdf");
    std::fs::write(
        &file,
        katna_pdf_cos::text_pdf("Note", &["  Hello".to_string()]),
    )
    .unwrap();
    let boxed = d.join("boxed.pdf");
    let (code, out) = run(&[
        "insert",
        s(&file),
        "-o",
        s(&boxed),
        "--shape",
        "rectangle",
        "--rect",
        "100,100,200,150",
        "--fill",
    ]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = run(&["objects", s(&boxed), "--json"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(r#"{"object":"1:1","kind":"shape","rect":[100,100,200,150]}"#),
        "{out}"
    );
    let moved = d.join("moved.pdf");
    let (code, out) = run(&[
        "object",
        s(&boxed),
        "1:1",
        "-o",
        s(&moved),
        "--rect",
        "300,400,500,500",
    ]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["objects", s(&moved)]);
    assert!(out.contains("1:1      shape    300,400,500,500"), "{out}");
    let gone = d.join("gone.pdf");
    let (code, out) = run(&["object", s(&moved), "1:1", "-o", s(&gone), "--delete"]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["objects", s(&gone)]);
    assert_eq!(out, "");
    let (_, out) = run(&["text", s(&gone)]);
    assert!(out.contains("Hello"), "{out}");
    let (code, _) = run(&["object", s(&gone), "1:1", "-o", s(&moved), "--delete"]);
    assert_ne!(code, 0);
}

#[test]
fn footers_watermarks_and_bates_numbers_go_on_pages() {
    let d = dir("stamp");
    let one = d.join("one.pdf");
    std::fs::write(
        &one,
        katna_pdf_cos::text_pdf("Note", &["  Hello".to_string()]),
    )
    .unwrap();
    let file = d.join("two.pdf");
    let (code, out) = run(&["merge", s(&one), s(&one), "-o", s(&file)]);
    assert_eq!(code, 0, "{out}");
    let footed = d.join("footed.pdf");
    let (code, out) = run(&[
        "stamp",
        s(&file),
        "-o",
        s(&footed),
        "--text",
        "Page {page} of {pages}",
    ]);
    assert_eq!(code, 0, "{out}");
    let marked = d.join("marked.pdf");
    let (code, out) = run(&[
        "stamp",
        s(&footed),
        "-o",
        s(&marked),
        "--text",
        "DRAFT",
        "--place",
        "across",
    ]);
    assert_eq!(code, 0, "{out}");
    let numbered = d.join("numbered.pdf");
    let (code, out) = run(&[
        "stamp",
        s(&marked),
        "-o",
        s(&numbered),
        "--text",
        "{bates}",
        "--place",
        "bottom-right",
        "--pages",
        "2",
        "--bates-prefix",
        "X-",
        "--bates-start",
        "41",
        "--bates-digits",
        "3",
    ]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["text", s(&numbered)]);
    assert!(out.contains("Helvetica 10pt: Page 1 of 2"), "{out}");
    assert!(out.contains("Helvetica 10pt: Page 2 of 2"), "{out}");
    assert!(out.contains("DRAFT"), "{out}");
    assert!(out.contains("Helvetica 10pt: X-041"), "{out}");
    assert!(!out.contains("X-042"), "{out}");
    let (code, _) = run(&[
        "stamp",
        s(&file),
        "-o",
        s(&numbered),
        "--text",
        "x",
        "--place",
        "sideways",
    ]);
    assert_ne!(code, 0);
}

#[test]
fn properties_are_changed() {
    let d = dir("meta");
    let file = d.join("note.pdf");
    std::fs::write(
        &file,
        katna_pdf_cos::text_pdf("Note", &["  Hello".to_string()]),
    )
    .unwrap();
    let out = d.join("out.pdf");
    let (code, text) = run(&[
        "meta",
        s(&file),
        "-o",
        s(&out),
        "--title",
        "Minutes – Monday",
        "--author",
        "Asha",
    ]);
    assert_eq!(code, 0, "{text}");
    let (_, text) = run(&["info", s(&out)]);
    assert!(text.contains("Minutes – Monday"), "{text}");
    assert!(text.contains("Asha"), "{text}");
    let again = d.join("again.pdf");
    let (code, text) = run(&["meta", s(&out), "-o", s(&again), "--author", ""]);
    assert_eq!(code, 0, "{text}");
    let (_, text) = run(&["info", s(&again)]);
    assert!(!text.contains("Asha"), "{text}");
    assert!(text.contains("Minutes – Monday"), "{text}");
}

#[test]
fn a_password_is_put_on_and_taken_off() {
    let d = dir("lock");
    let file = d.join("note.pdf");
    std::fs::write(
        &file,
        katna_pdf_cos::text_pdf("Note", &["  Hello".to_string()]),
    )
    .unwrap();
    let locked = d.join("locked.pdf");
    let (code, out) = run(&[
        "lock",
        s(&file),
        "-o",
        s(&locked),
        "--user",
        "asha",
        "--owner",
        "boss",
        "--allow",
        "print,copy",
    ]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["text", s(&locked)]);
    assert!(!out.contains("Hello"), "{out}");
    let open = d.join("open.pdf");
    let (code, out) = run(&["unlock", s(&locked), "-o", s(&open), "--owner", "asha"]);
    assert_ne!(code, 0, "{out}");
    let (code, out) = run(&["unlock", s(&locked), "-o", s(&open), "--owner", "boss"]);
    assert_eq!(code, 0, "{out}");
    let (_, out) = run(&["text", s(&open)]);
    assert!(out.contains("Hello"), "{out}");
    let (code, _) = run(&["lock", s(&file), "-o", s(&open), "--allow", "fly"]);
    assert_ne!(code, 0);
}

#[test]
fn numbers_and_boxes_are_redacted() {
    let d = dir("redact");
    let file = d.join("form.pdf");
    std::fs::write(
        &file,
        katna_pdf_cos::text_pdf(
            "Form",
            &[
                "  PAN: ABCDE1234F".to_string(),
                "  Mail asha@example.in".to_string(),
                "  Kept".to_string(),
            ],
        ),
    )
    .unwrap();
    let out = d.join("out.pdf");
    let (code, text) = run(&[
        "redact",
        s(&file),
        "-o",
        s(&out),
        "--find",
        "pan,email",
        "--json",
    ]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(text.matches("\"page\":1").count(), 2, "{text}");
    let (_, text) = run(&["text", s(&out)]);
    assert!(!text.contains("ABCDE") && !text.contains("asha@"), "{text}");
    assert!(text.contains("PAN:") && text.contains("Kept"), "{text}");
    let (code, _) = run(&["redact", s(&file), "-o", s(&out), "--find", "dates"]);
    assert_eq!(code, 2);
    let (code, _) = run(&["redact", s(&file), "-o", s(&out), "--area", "9:0,0,1,1"]);
    assert_eq!(code, 2);
}

#[test]
fn forms_are_designed() {
    let d = dir("form-design");
    let file = d.join("blank.pdf");
    std::fs::write(&file, katna_pdf_cos::text_pdf("Form", &[])).unwrap();
    let out = d.join("out.pdf");
    let (code, text) = run(&[
        "form",
        s(&file),
        "-o",
        s(&out),
        "--add",
        "text:1:100,700,300,720:Name",
        "--add",
        "radio=Yes:1:100,660,114,674:Answer",
        "--add",
        "radio=No:1:140,660,154,674:Answer",
        "--add",
        "combo=Delhi;Pune:1:100,620,300,640:City",
        "--required",
        "Name",
        "--tip",
        "City=Where you live",
    ]);
    assert_eq!(code, 0, "{text}");
    let doc = katna_pdf_cos::Document::load(std::fs::read(&out).unwrap()).unwrap();
    let form = doc.form();
    let names: Vec<&str> = form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Name", "Answer", "City"]);
    assert!(form[0].required);
    assert_eq!(form[1].widgets.len(), 2);
    assert_eq!(form[2].tooltip.as_deref(), Some("Where you live"));
    let again = d.join("again.pdf");
    let (code, text) = run(&[
        "form",
        s(&out),
        "-o",
        s(&again),
        "--remove",
        "Answer",
        "--rename",
        "City=Town",
        "--move",
        "Name:100,500,300,520",
    ]);
    assert_eq!(code, 0, "{text}");
    let doc = katna_pdf_cos::Document::load(std::fs::read(&again).unwrap()).unwrap();
    let form = doc.form();
    let names: Vec<&str> = form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Name", "Town"]);
    assert_eq!(form[0].widgets[0].rect, [100.0, 500.0, 300.0, 520.0]);
    let (code, _) = run(&[
        "form",
        s(&out),
        "-o",
        s(&again),
        "--add",
        "wheel:1:0,0,1,1:X",
    ]);
    assert_eq!(code, 2);
    let (code, _) = run(&["form", s(&out), "-o", s(&again), "--remove", "Nobody"]);
    assert_eq!(code, 1);
}
