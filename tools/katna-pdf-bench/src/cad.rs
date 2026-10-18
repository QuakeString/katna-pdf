// SPDX-License-Identifier: GPL-3.0-or-later

//! `katna-pdf-bench --cad FILE…`: zoom times on vector-heavy drawings,
//! against the budgets in docs/PLAN.md §2.5, drawing page 1 from a scene
//! and the usual way side by side.
//!
//! Each step draws a 1920×1080 view as 512-pixel tiles on every core, as
//! the app does: the whole sheet fitted to the view, then 100 %, 400 % and
//! 3200 % around the sheet's middle. The scene's first view includes
//! reading the page into the scene; later views reuse it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use katna_pdf_render::scene::{page_scene, quick_reader_agrees};
use katna_pdf_render::{Bitmap, Document, Engine, Hayro, HayroPlain, Region};

const VIEW: (u32, u32) = (1920, 1080);
const TILE: u32 = 512;
/// "Open to first full-sheet view" for a vector-heavy page.
const FIRST_VIEW_BUDGET: Duration = Duration::from_millis(600);
/// "Any zoom step: sharp visible tiles".
const SHARP_BUDGET: Duration = Duration::from_millis(250);

pub fn run(files: &[String]) -> bool {
    let mut ok = true;
    for file in files {
        if let Err(err) = measure(file) {
            eprintln!("{file}: {err}");
            ok = false;
        }
    }
    ok
}

/// `--scene-check`: draws page 1 of each file from a scene, vector-heavy
/// or not, and the usual way, and prints how much the two differ. A check
/// that scenes draw what hayro draws, on ordinary files too. The last
/// column says whether our quick reader read the page (`quick`), and
/// `QUICK-DIFFERS` if it read it unlike hayro.
pub fn check(files: &[String]) -> bool {
    for file in files {
        let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let bytes = Arc::new(bytes);
        let scene = match page_scene(bytes.clone(), 0) {
            Some(Ok(scene)) => scene,
            Some(Err(why)) => {
                println!("{name}\tskip\t{why:?}");
                continue;
            }
            None => continue,
        };
        let Ok(plain) = HayroPlain.open(bytes.clone(), "") else {
            continue;
        };
        let (w, h) = scene.size();
        let size = ((w * 1.5) as u32, (h * 1.5) as u32);
        let region = Region {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        };
        let a = scene.render(1.5, (0.0, 0.0), size);
        let b = plain.render_region(0, 1.5, region);
        // Whether our quick reader read the page, and into what hayro's
        // interpreter reads.
        let quick = match quick_reader_agrees(bytes.clone(), 0) {
            Some(true) => "quick",
            Some(false) => "QUICK-DIFFERS",
            None => "hayro",
        };
        println!(
            "{name}\t{}\t{:.3}\t{quick}",
            scene.len(),
            differs(&[a], &[b])
        );
    }
    true
}

fn measure(file: &str) -> Result<(), String> {
    let bytes = Arc::new(std::fs::read(file).map_err(|err| err.to_string())?);
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    println!("{name}");
    match page_scene(bytes.clone(), 0) {
        Some(Ok(scene)) => println!(
            "  scene: {} items, {:.0} MB",
            scene.len(),
            scene.memory() as f64 / 1_048_576.0
        ),
        Some(Err(why)) => println!("  no scene: the page has {why:?}"),
        None => return Err("no first page".into()),
    }
    let open = |engine: &dyn Engine| {
        let started = Instant::now();
        let doc = engine
            .open(bytes.clone(), "")
            .map_err(|err| err.to_string());
        doc.map(|doc| (doc, started.elapsed()))
    };
    let (scene, scene_open) = open(&Hayro)?;
    let (plain, plain_open) = open(&HayroPlain)?;
    let (w, h) = scene.page_size(0).ok_or("no first page")?;
    let fit = (VIEW.0 as f32 / w).min(VIEW.1 as f32 / h);
    println!(
        "  {:<14} {:>12} {:>12} {:>9} {:>9}",
        "view", "scene ms", "usual ms", "faster", "differs"
    );
    let steps = [
        ("sheet", fit),
        ("100 %", 1.0),
        ("400 %", 4.0),
        ("3200 %", 32.0),
    ];
    for (i, (label, scale)) in steps.into_iter().enumerate() {
        let (scene_ms, a) = view(scene.as_ref(), w, h, scale);
        let (plain_ms, b) = view(plain.as_ref(), w, h, scale);
        // Opening counts toward the first view, as in the budget.
        let (scene_ms, plain_ms) = if i == 0 {
            (scene_ms + scene_open, plain_ms + plain_open)
        } else {
            (scene_ms, plain_ms)
        };
        let budget = if i == 0 {
            FIRST_VIEW_BUDGET
        } else {
            SHARP_BUDGET
        };
        let mark = |d: Duration| if d <= budget { ' ' } else { '!' };
        println!(
            "  {:<14} {:>11.0}{} {:>11.0}{} {:>8.1}× {:>8.2}%",
            label,
            ms(scene_ms),
            mark(scene_ms),
            ms(plain_ms),
            mark(plain_ms),
            ms(plain_ms) / ms(scene_ms).max(0.001),
            differs(&a, &b)
        );
    }
    // The same views again, now the scene is built: a second visit.
    let again: Duration = steps
        .iter()
        .map(|&(_, scale)| view(scene.as_ref(), w, h, scale).0)
        .max()
        .unwrap_or_default();
    println!(
        "  slowest view once the scene is built: {:.0} ms",
        ms(again)
    );
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Draws the [`VIEW`]-sized view of page 1 at `scale` centred on the
/// page, as tiles on every core. Returns the time and the tiles.
fn view(doc: &dyn Document, w: f32, h: f32, scale: f32) -> (Duration, Vec<Option<Bitmap>>) {
    let (pw, ph) = ((w * scale) as u32, (h * scale) as u32);
    let (vw, vh) = (VIEW.0.min(pw), VIEW.1.min(ph));
    let (x0, y0) = ((pw - vw) / 2, (ph - vh) / 2);
    let mut tiles = Vec::new();
    for ty in (0..vh).step_by(TILE as usize) {
        for tx in (0..vw).step_by(TILE as usize) {
            tiles.push(Region {
                x: x0 + tx,
                y: y0 + ty,
                width: TILE.min(vw - tx),
                height: TILE.min(vh - ty),
            });
        }
    }
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let started = Instant::now();
    let mut done: Vec<(usize, Option<Bitmap>)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&region) = tiles.get(i) else {
                            return out;
                        };
                        out.push((i, doc.render_region(0, scale, region)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_default())
            .collect()
    });
    let elapsed = started.elapsed();
    done.sort_by_key(|(i, _)| *i);
    (elapsed, done.into_iter().map(|(_, b)| b).collect())
}

/// The share of pixels whose colour differs by more than a quarter of the
/// range in some channel between two drawings of the same tiles.
fn differs(a: &[Option<Bitmap>], b: &[Option<Bitmap>]) -> f64 {
    let (mut differ, mut total) = (0usize, 0usize);
    for (a, b) in a.iter().zip(b) {
        let (Some(a), Some(b)) = (a, b) else {
            continue;
        };
        for (p, q) in a
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b.rgba.as_chunks::<4>().0)
        {
            total += 1;
            if p.iter().zip(q).any(|(x, y)| x.abs_diff(*y) > 64) {
                differ += 1;
            }
        }
    }
    100.0 * differ as f64 / total.max(1) as f64
}
