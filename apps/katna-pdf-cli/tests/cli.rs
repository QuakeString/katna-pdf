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
