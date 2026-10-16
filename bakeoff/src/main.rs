// SPDX-License-Identifier: GPL-3.0-or-later

//! The Phase 0 engine bake-off (docs/PLAN.md §3.1): hayro, PDFium and
//! MuPDF open and draw the same PDFs, and we compare speed, failures and
//! how the pages look.
//!
//! ```sh
//! katna-pdf-bakeoff sweep DIR OUT.jsonl     # every PDF × every engine
//! katna-pdf-bakeoff report OUT.jsonl        # Markdown summary
//! katna-pdf-bakeoff run ENGINE FILE         # one file, one JSON line
//! ```
//!
//! `sweep` runs each file in a child process with a time limit, so an
//! engine that crashes or hangs on a file is counted rather than stopping
//! the run. PDFium is loaded from `$PDFIUM_LIB` (a `libpdfium.so`).

mod engines;
mod report;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

pub const ENGINES: &[&str] = &["hayro", "pdfium", "mupdf"];

/// A screen's worth of page at fit width on a 1080p screen, about.
pub const SCREEN_SCALE: f32 = 1.5;
/// A deep zoom into a drawing: one screen-sized tile at 8×.
pub const DEEP_SCALE: f32 = 8.0;
pub const TILE: u32 = 1024;
/// Side of the small grey picture of page 1 kept to compare engines.
pub const THUMB: usize = 32;

/// One engine's view of one open document.
pub trait Doc {
    fn page_count(&self) -> usize;
    fn page_size(&self, page: usize) -> (f32, f32);
    /// The page as RGB or RGBA rows, with its width, height and channels.
    fn render(&mut self, page: usize, scale: f32) -> Result<Image, String>;
    /// A `TILE`×`TILE` piece of the page at `scale`, from `(x, y)`.
    fn render_tile(&mut self, page: usize, scale: f32, x: u32, y: u32) -> Result<(), String>;
}

pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub stride: usize,
    pub pixels: Vec<u8>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["run", engine, file] => run(engine, Path::new(file)),
        ["sweep", dir, out, rest @ ..] => sweep(Path::new(dir), Path::new(out), rest),
        ["report", file] => match report::report(Path::new(file)) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{err}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!(
                "usage: katna-pdf-bakeoff sweep DIR OUT.jsonl [--timeout SECS] [--engines a,b]\n       katna-pdf-bakeoff report OUT.jsonl\n       katna-pdf-bakeoff run ENGINE FILE"
            );
            ExitCode::FAILURE
        }
    }
}

/// Measures one file with one engine and prints one JSON line.
fn run(engine: &str, file: &Path) -> ExitCode {
    let line = match measure(engine, file) {
        Ok(fields) => format!("{{\"status\":\"ok\",{fields}}}"),
        Err(err) => format!("{{\"status\":\"error\",\"error\":{}}}", json_str(&err)),
    };
    println!("{line}");
    ExitCode::SUCCESS
}

fn measure(engine: &str, file: &Path) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut doc = engines::open(engine, bytes)?;
    let open = started.elapsed();
    let pages = doc.page_count();
    if pages == 0 {
        return Err("no pages".into());
    }
    let started = Instant::now();
    let first = doc.render(0, SCREEN_SCALE)?;
    let first_page = started.elapsed();
    let thumb = thumbnail(&first);

    // Up to 20 pages spread through the file; a page that fails counts.
    let step = (pages / 20).max(1);
    let sample: Vec<usize> = (0..pages).step_by(step).take(20).collect();
    let mut total = Duration::ZERO;
    let mut page_errors = 0;
    for &p in &sample {
        let started = Instant::now();
        if doc.render(p, SCREEN_SCALE).is_err() {
            page_errors += 1;
        }
        total += started.elapsed();
    }
    let mean_page = total / sample.len() as u32;

    let (w, h) = doc.page_size(0);
    let x = ((w * DEEP_SCALE) as u32 / 2).saturating_sub(TILE / 2);
    let y = ((h * DEEP_SCALE) as u32 / 2).saturating_sub(TILE / 2);
    let started = Instant::now();
    let tile_ok = doc.render_tile(0, DEEP_SCALE, x, y).is_ok();
    let deep_tile = started.elapsed();

    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    Ok(format!(
        "\"pages\":{pages},\"open_ms\":{:.3},\"first_page_ms\":{:.3},\"mean_page_ms\":{:.3},\"deep_tile_ms\":{:.3},\"tile_ok\":{tile_ok},\"page_errors\":{page_errors},\"sampled\":{},\"peak_rss_kb\":{},\"thumb\":\"{}\"",
        ms(open),
        ms(first_page),
        ms(mean_page),
        ms(deep_tile),
        sample.len(),
        peak_rss_kb(),
        hex(&thumb),
    ))
}

/// Page 1 shrunk to `THUMB`×`THUMB` grey levels, ignoring its aspect, so
/// two engines' pictures can be compared whatever their exact sizes.
fn thumbnail(image: &Image) -> Vec<u8> {
    let mut sums = vec![0u64; THUMB * THUMB];
    let mut counts = vec![0u64; THUMB * THUMB];
    for y in 0..image.height {
        let row = &image.pixels[y * image.stride..];
        let ty = y * THUMB / image.height.max(1);
        for x in 0..image.width {
            let px = &row[x * image.channels..x * image.channels + 3];
            let grey = (px[0] as u64 * 299 + px[1] as u64 * 587 + px[2] as u64 * 114) / 1000;
            let i = ty * THUMB + x * THUMB / image.width.max(1);
            sums[i] += grey;
            counts[i] += 1;
        }
    }
    sums.iter()
        .zip(&counts)
        .map(|(s, c)| (s / (*c).max(1)) as u8)
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The process's peak memory, from /proc (0 elsewhere).
fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn sweep(dir: &Path, out: &Path, rest: &[&str]) -> ExitCode {
    let mut timeout = Duration::from_secs(30);
    let mut engines: Vec<String> = ENGINES.iter().map(|s| s.to_string()).collect();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match (*arg, it.next()) {
            ("--timeout", Some(v)) => timeout = Duration::from_secs(v.parse().unwrap_or(30)),
            ("--engines", Some(v)) => engines = v.split(',').map(str::to_string).collect(),
            _ => {
                eprintln!("unknown option {arg}");
                return ExitCode::FAILURE;
            }
        }
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
    files.sort();
    let me = std::env::current_exe().expect("own path");
    let mut out = std::fs::File::create(out).expect("output file");
    for (n, file) in files.iter().enumerate() {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
        for engine in &engines {
            let result = child(&me, engine, file, timeout);
            writeln!(
                out,
                "{{\"file\":{},\"bytes\":{bytes},\"engine\":{},{}}}",
                json_str(&name),
                json_str(engine),
                &result[1..result.len() - 1]
            )
            .ok();
        }
        if n % 50 == 49 {
            eprintln!("{} / {}", n + 1, files.len());
        }
    }
    ExitCode::SUCCESS
}

/// Runs `run ENGINE FILE` in a child process; a crash or a hang becomes a
/// status of its own.
fn child(me: &Path, engine: &str, file: &Path, timeout: Duration) -> String {
    let started = Instant::now();
    let mut proc = match Command::new(me)
        .args(["run", engine])
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(p) => p,
        Err(e) => {
            return format!(
                "{{\"status\":\"error\",\"error\":{}}}",
                json_str(&e.to_string())
            );
        }
    };
    loop {
        match proc.try_wait() {
            Ok(Some(status)) => {
                let mut text = String::new();
                if let Some(mut out) = proc.stdout.take() {
                    std::io::Read::read_to_string(&mut out, &mut text).ok();
                }
                let line = text.lines().last().unwrap_or("").trim();
                if status.success() && line.starts_with('{') {
                    return line.to_string();
                }
                return format!(
                    "{{\"status\":\"crash\",\"error\":{}}}",
                    json_str(&status.to_string())
                );
            }
            Ok(None) if started.elapsed() > timeout => {
                proc.kill().ok();
                proc.wait().ok();
                return "{\"status\":\"timeout\"}".into();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                return format!(
                    "{{\"status\":\"error\",\"error\":{}}}",
                    json_str(&e.to_string())
                );
            }
        }
    }
}
