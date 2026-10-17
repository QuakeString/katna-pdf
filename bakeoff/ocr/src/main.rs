// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs one pure-Rust OCR engine over images and prints, per image, one
//! JSON line: the file, the milliseconds it took and the text it read,
//! lines top to bottom. `score.py` compares that text with the truth.
//!
//! ```text
//! katna-pdf-ocr-bakeoff ocrs  DETECTION.rten RECOGNITION.rten IMAGE…
//! katna-pdf-ocr-bakeoff ppocr DET.onnx REC.onnx KEYS.txt IMAGE…
//! ```
//!
//! `ocrs` is the ocrs crate as it ships. `ppocr` is PaddleOCR's detector
//! (DB) and recogniser (CTC) run on rten, with the usual PaddleOCR
//! pre- and post-processing written out here.

use std::time::Instant;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use rten_imageproc::{RetrievalMode, RotatedRect, find_contours, min_area_rect, simplify_polygon};
use rten_tensor::prelude::*;
use rten_tensor::{NdTensor, Tensor};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ocrs") if args.len() > 3 => run_ocrs(&args[1], &args[2], &args[3..]),
        Some("ppocr") if args.len() > 4 => run_ppocr(&args[1], &args[2], &args[3], &args[4..]),
        _ => Err("usage: ocrs DET REC IMAGE… | ppocr DET REC KEYS IMAGE…".into()),
    }
}

fn report(file: &str, started: Instant, lines: &[String]) {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{}",
        serde_json::json!({ "file": file, "ms": ms, "text": lines.join("\n") })
    );
}

fn run_ocrs(det: &str, rec: &str, images: &[String]) -> Result<()> {
    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(Model::load_file(det)?),
        recognition_model: Some(Model::load_file(rec)?),
        ..Default::default()
    })?;
    for file in images {
        let img = image::open(file)?.into_rgb8();
        let started = Instant::now();
        let source = ImageSource::from_bytes(img.as_raw(), img.dimensions())?;
        let input = engine.prepare_input(source)?;
        let words = engine.detect_words(&input)?;
        let lines = engine.find_text_lines(&input, &words);
        let text = engine.recognize_text(&input, &lines)?;
        let lines: Vec<String> = text.into_iter().flatten().map(|l| l.to_string()).collect();
        report(file, started, &lines);
    }
    Ok(())
}

/// The longest side the detector sees (PaddleOCR's `max_side_len`).
const DET_MAX_SIDE: usize = 2000;
/// A pixel is text when the detector's probability is above this.
const DET_THRESHOLD: f32 = 0.3;
/// How much DB's shrunk text regions are grown back.
const UNCLIP_RATIO: f32 = 1.5;
/// Height of the recogniser's input strip.
const REC_HEIGHT: usize = 48;

fn run_ppocr(det: &str, rec: &str, keys: &str, images: &[String]) -> Result<()> {
    let det = Model::load_file(det)?;
    let rec = Model::load_file(rec)?;
    // Class 0 is CTC's blank; then the dictionary; then a space.
    let mut chars: Vec<String> = vec![String::new()];
    chars.extend(
        std::fs::read_to_string(keys)?
            .split('\n')
            .map(str::to_owned),
    );
    chars.push(" ".into());
    for file in images {
        let img = image::open(file)?.into_luma8();
        let started = Instant::now();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let grey = NdTensor::from_data(
            [h, w],
            img.as_raw()
                .iter()
                .map(|&v| f32::from(v))
                .collect::<Vec<_>>(),
        );
        let boxes = detect(&det, &grey)?;
        let mut found = Vec::new();
        for rect in boxes {
            let strip = crop(&grey, &rect);
            let text = recognise(&rec, &strip, &chars)?;
            if !text.trim().is_empty() {
                found.push((rect, text));
            }
        }
        report(file, started, &reading_order(found));
    }
    Ok(())
}

/// Text regions in `grey`, in image coordinates.
fn detect(model: &Model, grey: &NdTensor<f32, 2>) -> Result<Vec<RotatedRect>> {
    let [h, w] = grey.shape();
    let scale = (DET_MAX_SIDE as f32 / h.max(w) as f32).min(1.0);
    let round = |v: usize| (((v as f32 * scale) / 32.0).round() as usize).max(1) * 32;
    let (dh, dw) = (round(h), round(w));
    let mean = [0.485f32, 0.456, 0.406];
    let std = [0.229f32, 0.224, 0.225];
    let mut input = NdTensor::zeros([1, 3, dh, dw]);
    for y in 0..dh {
        for x in 0..dw {
            let v = bilinear(
                grey,
                (x as f32 + 0.5) * w as f32 / dw as f32 - 0.5,
                (y as f32 + 0.5) * h as f32 / dh as f32 - 0.5,
            ) / 255.0;
            for c in 0..3 {
                input[[0, c, y, x]] = (v - mean[c]) / std[c];
            }
        }
    }
    let out: Tensor<f32> = model.run_one(input.view().into(), None)?.try_into()?;
    let prob = out.reshaped([dh, dw]).to_tensor();
    let mask = prob.map(|&p| p > DET_THRESHOLD);
    let (sx, sy) = (w as f32 / dw as f32, h as f32 / dh as f32);
    let mut rects = Vec::new();
    for poly in find_contours(mask.view(), RetrievalMode::External).iter() {
        let points: Vec<_> = poly.iter().map(|p| p.to_f32()).collect();
        let points = simplify_polygon(&points, 1.0);
        let Some(rect) = min_area_rect(&points) else {
            continue;
        };
        let (rw, rh) = (rect.width(), rect.height());
        if rw.min(rh) < 3.0 {
            continue;
        }
        // DB's unclip: grow by area × ratio / perimeter on every side.
        let grow = rw * rh * UNCLIP_RATIO / (2.0 * (rw + rh));
        let c = rect.center();
        let up = rect.up_axis();
        rects.push(RotatedRect::new(
            rten_imageproc::PointF::from_yx(c.y * sy, c.x * sx),
            rten_imageproc::Vec2::from_yx(up.y * sy, up.x * sx),
            (rw + 2.0 * grow) * axis_scale(up, sx, sy, false),
            (rh + 2.0 * grow) * axis_scale(up, sx, sy, true),
        ));
    }
    Ok(rects)
}

/// How a length along the rect's width (or, with `along_up`, its height)
/// scales from detector to image pixels.
fn axis_scale(up: rten_imageproc::Vec2, sx: f32, sy: f32, along_up: bool) -> f32 {
    let (dx, dy) = if along_up {
        (up.x, up.y)
    } else {
        (-up.y, up.x)
    };
    ((dx * sx).powi(2) + (dy * sy).powi(2)).sqrt() / (dx * dx + dy * dy).sqrt()
}

/// The region `rect` cut out of `grey` as a level strip `REC_HEIGHT` high.
fn crop(grey: &NdTensor<f32, 2>, rect: &RotatedRect) -> NdTensor<f32, 2> {
    let c = rect.center();
    let up = rect.up_axis();
    let side = (-up.y, up.x);
    // The long side runs along the text.
    let (mut along, length, mut across, thickness) = if rect.width() >= rect.height() {
        (side, rect.width(), (up.x, up.y), rect.height())
    } else {
        ((up.x, up.y), rect.height(), side, rect.width())
    };
    if along.0 < 0.0 {
        along = (-along.0, -along.1);
    }
    if across.1 < 0.0 {
        across = (-across.0, -across.1);
    }
    let out_w = ((REC_HEIGHT as f32 * length / thickness).round() as usize).max(8);
    let mut out = NdTensor::zeros([REC_HEIGHT, out_w]);
    for y in 0..REC_HEIGHT {
        let t = ((y as f32 + 0.5) / REC_HEIGHT as f32 - 0.5) * thickness;
        for x in 0..out_w {
            let s = ((x as f32 + 0.5) / out_w as f32 - 0.5) * length;
            out[[y, x]] = bilinear(
                grey,
                c.x + along.0 * s + across.0 * t,
                c.y + along.1 * s + across.1 * t,
            );
        }
    }
    out
}

fn recognise(model: &Model, strip: &NdTensor<f32, 2>, chars: &[String]) -> Result<String> {
    let [h, w] = strip.shape();
    let mut input = NdTensor::zeros([1, 3, h, w]);
    for y in 0..h {
        for x in 0..w {
            let v = (strip[[y, x]] / 255.0 - 0.5) / 0.5;
            for c in 0..3 {
                input[[0, c, y, x]] = v;
            }
        }
    }
    let out: NdTensor<f32, 3> = model.run_one(input.view().into(), None)?.try_into()?;
    let [_, steps, classes] = out.shape();
    let mut text = String::new();
    let mut last = 0;
    for t in 0..steps {
        let mut best = 0;
        for k in 1..classes {
            if out[[0, t, k]] > out[[0, t, best]] {
                best = k;
            }
        }
        if best != 0 && best != last {
            text.push_str(chars.get(best).map_or("", String::as_str));
        }
        last = best;
    }
    Ok(text)
}

/// Joins regions on the same line left to right, lines top to bottom.
fn reading_order(mut found: Vec<(RotatedRect, String)>) -> Vec<String> {
    found.sort_by(|a, b| a.0.center().y.total_cmp(&b.0.center().y));
    // Each line: its centre's y, its thickness, and its regions by x.
    type Line = (f32, f32, Vec<(f32, String)>);
    let mut lines: Vec<Line> = Vec::new();
    for (rect, text) in found {
        let c = rect.center();
        let thickness = rect.width().min(rect.height());
        match lines.last_mut() {
            Some((y, th, items)) if (c.y - *y).abs() < 0.5 * th.min(thickness) => {
                items.push((c.x, text))
            }
            _ => lines.push((c.y, thickness, vec![(c.x, text)])),
        }
    }
    lines
        .into_iter()
        .map(|(_, _, mut items)| {
            items.sort_by(|a, b| a.0.total_cmp(&b.0));
            items
                .into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn bilinear(img: &NdTensor<f32, 2>, x: f32, y: f32) -> f32 {
    let [h, w] = img.shape();
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let top = img[[y0, x0]] * (1.0 - fx) + img[[y0, x1]] * fx;
    let bottom = img[[y1, x0]] * (1.0 - fx) + img[[y1, x1]] * fx;
    top * (1.0 - fy) + bottom * fy
}
