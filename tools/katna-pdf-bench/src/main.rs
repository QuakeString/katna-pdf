// SPDX-License-Identifier: GPL-3.0-or-later

//! `katna-pdf-bench FILE…`: how long each PDF takes to open and draw,
//! against the budgets in docs/PLAN.md §2.4. It prints one line per file;
//! `--json` prints JSON lines instead, for CI to keep. `--cad` measures
//! zooming into drawings instead (`cad.rs`). `--plain` never draws pages
//! from scenes, for comparison. `--workers` draws in worker processes,
//! as the app does (`katna-pdf-render-worker`, built beside this).

mod cad;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use katna_pdf_render::{Document, Engine, Hayro, HayroPlain, Region, Workers};

/// "Open a 1,000-page PDF to first page visible": open plus first page.
const FIRST_PAGE_BUDGET: Duration = Duration::from_millis(300);
/// "Zoom change: sharp", for one visible page.
const SHARP_BUDGET: Duration = Duration::from_millis(150);
/// A screen's worth of page at fit-width on a 1080p screen, about.
const SCREEN_SCALE: f32 = 1.5;
/// A deep zoom into a drawing: one screen-sized tile at 8×.
const DEEP_SCALE: f32 = 8.0;
const TILE: u32 = 1024;

struct Timing {
    file: String,
    bytes: usize,
    pages: usize,
    open: Duration,
    first_page: Duration,
    mean_page: Duration,
    deep_tile: Duration,
}

fn main() -> ExitCode {
    let mut json = false;
    let mut drawings = false;
    let mut scene_check = false;
    let mut plain = false;
    let mut workers = false;
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--json" => json = true,
            "--cad" => drawings = true,
            "--scene-check" => scene_check = true,
            "--plain" => plain = true,
            "--workers" => workers = true,
            "-h" | "--help" => {
                println!("usage: katna-pdf-bench [--json | --cad] [--plain | --workers] FILE.pdf…");
                return ExitCode::SUCCESS;
            }
            _ => files.push(arg),
        }
    }
    if files.is_empty() {
        eprintln!("usage: katna-pdf-bench [--json | --cad] [--plain | --workers] FILE.pdf…");
        return ExitCode::FAILURE;
    }
    if scene_check {
        cad::check(&files);
        return ExitCode::SUCCESS;
    }
    if drawings {
        return if cad::run(&files) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let pool;
    let engine: &dyn Engine = if plain {
        &HayroPlain
    } else if workers {
        let program = std::env::current_exe()
            .unwrap_or_default()
            .with_file_name(format!(
                "katna-pdf-render-worker{}",
                std::env::consts::EXE_SUFFIX
            ));
        pool = Workers::new(move || program.clone(), Vec::new(), 4);
        &pool
    } else {
        &Hayro
    };
    let mut failed = false;
    if !json {
        println!(
            "{:<32} {:>8} {:>6} {:>9} {:>11} {:>10} {:>10}",
            "file", "size", "pages", "open", "1st page", "mean page", "8× tile"
        );
    }
    for file in files {
        match measure(engine, &file) {
            Ok(t) => print(&t, json),
            Err(err) => {
                eprintln!("{file}: {err}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn measure(engine: &dyn Engine, file: &str) -> Result<Timing, String> {
    let bytes = std::fs::read(file).map_err(|err| err.to_string())?;
    let size = bytes.len();
    let started = Instant::now();
    let doc = engine
        .open(Arc::new(bytes), "")
        .map_err(|err| err.to_string())?;
    let open = started.elapsed();
    let first_page = time(|| doc.render(0, SCREEN_SCALE));
    // Up to 20 pages spread through the file, for a mean.
    let pages = doc.page_count();
    let step = (pages / 20).max(1);
    let sample: Vec<usize> = (0..pages).step_by(step).take(20).collect();
    let total: Duration = sample
        .iter()
        .map(|&p| time(|| doc.render(p, SCREEN_SCALE)))
        .sum();
    let mean_page = total / sample.len().max(1) as u32;
    Ok(Timing {
        file: file.rsplit(['/', '\\']).next().unwrap_or(file).to_string(),
        bytes: size,
        pages,
        open,
        first_page,
        mean_page,
        deep_tile: deep_tile(doc.as_ref()),
    })
}

fn time<T>(f: impl FnOnce() -> T) -> Duration {
    let started = Instant::now();
    std::hint::black_box(f());
    started.elapsed()
}

/// One tile from the middle of page 1 at [`DEEP_SCALE`].
fn deep_tile(doc: &dyn Document) -> Duration {
    let (w, h) = doc.page_size(0).unwrap_or((612.0, 792.0));
    let (cx, cy) = (w * DEEP_SCALE / 2.0, h * DEEP_SCALE / 2.0);
    let region = Region {
        x: (cx as u32).saturating_sub(TILE / 2),
        y: (cy as u32).saturating_sub(TILE / 2),
        width: TILE,
        height: TILE,
    };
    time(|| doc.render_region(0, DEEP_SCALE, region))
}

fn print(t: &Timing, json: bool) {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    if json {
        println!(
            r#"{{"file":{:?},"bytes":{},"pages":{},"open_ms":{:.2},"first_page_ms":{:.2},"mean_page_ms":{:.2},"deep_tile_ms":{:.2}}}"#,
            t.file,
            t.bytes,
            t.pages,
            ms(t.open),
            ms(t.first_page),
            ms(t.mean_page),
            ms(t.deep_tile)
        );
        return;
    }
    let mark = |d: Duration, budget: Duration| if d <= budget { " " } else { "!" };
    println!(
        "{:<32} {:>7.1}M {:>6} {:>7.1}ms {:>8.1}ms{} {:>8.1}ms{} {:>8.1}ms",
        t.file.chars().take(32).collect::<String>(),
        t.bytes as f64 / 1_048_576.0,
        t.pages,
        ms(t.open),
        ms(t.open + t.first_page),
        mark(t.open + t.first_page, FIRST_PAGE_BUDGET),
        ms(t.mean_page),
        mark(t.mean_page, SHARP_BUDGET),
        ms(t.deep_tile)
    );
}
