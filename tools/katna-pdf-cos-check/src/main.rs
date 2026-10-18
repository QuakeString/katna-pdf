// SPDX-License-Identifier: GPL-3.0-or-later

//! `katna-pdf-cos-check DIR`: runs Katna PDF's object layer over every PDF
//! in a folder and prints one line per file and a summary. For each file:
//!
//! - does it load, and did its cross-reference data need repair;
//! - does it find the same number of pages as hayro (the renderer);
//! - does an incremental update (a new /Info and a changed catalog) load
//!   again, in both katna-pdf-cos and hayro, with the same pages;
//! - how many form fields it has, and whether filling its text fields in
//!   reads back.
//!
//! Each file runs in a child process with a time limit, so a hang or a
//! crash is counted, not fatal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use katna_pdf_cos::{Dict, Document, Object};
use katna_pdf_render::{Engine, Hayro};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [one, file] if one == "--one" => {
            println!("{}", check(Path::new(file)));
            ExitCode::SUCCESS
        }
        [dir] => sweep(Path::new(dir)),
        _ => {
            eprintln!("usage: katna-pdf-cos-check DIR");
            ExitCode::FAILURE
        }
    }
}

/// One file's verdicts as `key=value` words.
fn check(file: &Path) -> String {
    let Ok(bytes) = std::fs::read(file) else {
        return "load=unreadable".into();
    };
    let hayro_pages = Hayro
        .open(Arc::new(bytes.clone()), "")
        .ok()
        .map(|d| d.page_count());
    let started = Instant::now();
    let doc = match Document::load(bytes) {
        Ok(d) => d,
        Err(e) => {
            return format!(
                "load=failed hayro={} why={}",
                hayro_pages.map_or("failed".into(), |n| n.to_string()),
                e.to_string().replace(' ', "_")
            );
        }
    };
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    let pages = doc.pages().len();
    let outline = doc.outline();
    let mut out = format!(
        "load=ok repaired={} encrypted={} pages={pages} hayro={} load_ms={load_ms:.2} outline={} \
         links={}",
        doc.was_repaired(),
        doc.is_encrypted(),
        hayro_pages.map_or("failed".into(), |n| n.to_string()),
        outline_summary(&outline, pages),
        doc.links().iter().map(Vec::len).sum::<usize>(),
    );
    let update = match doc.update() {
        Err(e) => format!(
            "skipped_{}",
            if e == katna_pdf_cos::UpdateError::Encrypted {
                "encrypted"
            } else {
                "repaired"
            }
        ),
        Ok(mut up) => {
            let mut info = Dict::new();
            info.set("Producer", Object::string("Katna PDF"));
            let r = up.add(Object::Dict(info));
            up.trailer_mut().set("Info", r);
            if let (Some(root), Some(mut catalog)) = (
                doc.trailer().get("Root").and_then(Object::as_ref),
                doc.catalog(),
            ) {
                catalog.set("PageMode", Object::name("UseNone"));
                up.replace(root, Object::Dict(catalog));
            }
            let updated = up.finish();
            let hayro_after = Hayro
                .open(Arc::new(updated.clone()), "")
                .ok()
                .map(|d| d.page_count());
            match Document::load(updated) {
                Err(_) => "reload_failed".into(),
                Ok(again) if again.was_repaired() => "reload_repaired".into(),
                Ok(again) if again.pages().len() != pages => "pages_changed".into(),
                Ok(_) if hayro_after != hayro_pages => "hayro_changed".into(),
                Ok(_) => "ok".into(),
            }
        }
    };
    out.push_str(&format!(" update={update}"));
    out.push_str(&format!(" form={}", form_check(&doc, pages)));
    out
}

/// How many form fields there are, and whether filling the text fields
/// in gives a file both readers open: e.g. `12/fill=ok`, `none`.
fn form_check(doc: &Document, pages: usize) -> String {
    let fields = doc.form();
    if fields.is_empty() {
        return "none".into();
    }
    let changes: Vec<_> = fields
        .iter()
        .filter(|f| matches!(f.kind, katna_pdf_cos::FieldKind::Text { .. }) && !f.read_only)
        .map(|f| (f.id, katna_pdf_cos::FieldValue::Text("Katna PDF".into())))
        .collect();
    let fill: String = match doc.fill(&changes) {
        Err(_) => "skipped".into(),
        Ok(filled) => {
            let hayro = Hayro
                .open(Arc::new(filled.clone()), "")
                .ok()
                .map(|d| d.page_count());
            match Document::load(filled) {
                Ok(again) if again.pages().len() == pages && hayro == Some(pages) => {
                    let ok = again
                        .form()
                        .iter()
                        .filter(|f| changes.iter().any(|(id, _)| *id == f.id))
                        .all(|f| f.value.as_deref() == Some("Katna PDF"));
                    if ok { "ok" } else { "value_lost" }.into()
                }
                _ => "reload_failed".into(),
            }
        }
    };
    format!("{}/fill={fill}", fields.len())
}

/// How many outline items there are, and how many go to a page that is
/// not in the file (`none`, or e.g. `12/1bad`).
fn outline_summary(outline: &[katna_pdf_cos::OutlineItem], pages: usize) -> String {
    fn count(items: &[katna_pdf_cos::OutlineItem], pages: usize, n: &mut usize, bad: &mut usize) {
        for item in items {
            *n += 1;
            if item.dest.is_some_and(|d| d.page >= pages) {
                *bad += 1;
            }
            count(&item.children, pages, n, bad);
        }
    }
    let (mut n, mut bad) = (0, 0);
    count(outline, pages, &mut n, &mut bad);
    match (n, bad) {
        (0, _) => "none".into(),
        (n, 0) => n.to_string(),
        (n, bad) => format!("{n}/{bad}bad"),
    }
}

fn sweep(dir: &Path) -> ExitCode {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
    files.sort();
    let me = std::env::current_exe().expect("own path");
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        let line = child(&me, file, Duration::from_secs(30));
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        println!("{name}\t{line}");
        let words: BTreeMap<&str, &str> =
            line.split(' ').filter_map(|w| w.split_once('=')).collect();
        let get = |k: &str| words.get(k).copied().unwrap_or("");
        let mut count = |k: String| *tally.entry(k).or_default() += 1;
        count(format!("load {}", get("load")));
        if get("load") == "ok" {
            count(format!("repaired {}", get("repaired")));
            let agree = match get("hayro") {
                "failed" => "hayro failed",
                h if h == get("pages") => "same",
                _ => "different",
            };
            count(format!("pages vs hayro: {agree}"));
            count(format!("update {}", get("update")));
        } else if get("load") == "failed" {
            count(format!(
                "load failed, hayro {}",
                if get("hayro") == "failed" {
                    "failed too"
                } else {
                    "opened it"
                }
            ));
        }
    }
    eprintln!("\n{} files", files.len());
    for (k, v) in &tally {
        eprintln!("{v:>6}  {k}");
    }
    ExitCode::SUCCESS
}

fn child(me: &Path, file: &Path, timeout: Duration) -> String {
    let started = Instant::now();
    let Ok(mut proc) = Command::new(me)
        .arg("--one")
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return "load=spawn_failed".into();
    };
    loop {
        match proc.try_wait() {
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut out) = proc.stdout.take() {
                    std::io::Read::read_to_string(&mut out, &mut text).ok();
                }
                return match text.lines().last() {
                    Some(l) if status.success() => l.to_string(),
                    _ => "load=crash".into(),
                };
            }
            Ok(None) if started.elapsed() > timeout => {
                proc.kill().ok();
                proc.wait().ok();
                return "load=timeout".into();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => return "load=crash".into(),
        }
    }
}
