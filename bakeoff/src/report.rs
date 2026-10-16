// SPDX-License-Identifier: GPL-3.0-or-later

//! Turns a sweep's JSON lines into a Markdown summary.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::{ENGINES, THUMB};

/// Mean grey-level difference (0–255) above which two engines' page 1
/// pictures are taken to disagree.
const DISAGREE: f64 = 6.0;

struct Row {
    status: String,
    error: String,
    open_first: f64,
    mean_page: f64,
    deep_tile: f64,
    tile_ok: bool,
    page_errors: u64,
    rss_mb: f64,
    thumb: Vec<u8>,
}

pub fn report(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    // file → engine → row
    let mut files: BTreeMap<String, BTreeMap<String, Row>> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("{e}: {line}"))?;
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let row = Row {
            status: v["status"].as_str().unwrap_or("?").to_string(),
            error: v["error"].as_str().unwrap_or("").to_string(),
            open_first: f("open_ms") + f("first_page_ms"),
            mean_page: f("mean_page_ms"),
            deep_tile: f("deep_tile_ms"),
            tile_ok: v["tile_ok"].as_bool().unwrap_or(false),
            page_errors: v["page_errors"].as_u64().unwrap_or(0),
            rss_mb: f("peak_rss_kb") / 1024.0,
            thumb: unhex(v["thumb"].as_str().unwrap_or("")),
        };
        files
            .entry(v["file"].as_str().unwrap_or("?").to_string())
            .or_default()
            .insert(v["engine"].as_str().unwrap_or("?").to_string(), row);
    }
    let engines: Vec<&str> = ENGINES
        .iter()
        .copied()
        .filter(|e| files.values().any(|m| m.contains_key(*e)))
        .collect();

    let mut out = String::new();
    writeln!(out, "Files: {}\n", files.len()).ok();

    writeln!(out, "### Opens and draws\n").ok();
    writeln!(
        out,
        "| Engine | Drew page 1 | Error | Crash | Hang (timeout) | Sampled pages failed | 8× tile failed |"
    )
    .ok();
    writeln!(out, "|---|---|---|---|---|---|---|").ok();
    for e in &engines {
        let rows: Vec<&Row> = files.values().filter_map(|m| m.get(*e)).collect();
        let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
        let ok: Vec<&&Row> = rows.iter().filter(|r| r.status == "ok").collect();
        writeln!(
            out,
            "| {e} | {} ({:.1}%) | {} | {} | {} | {} files | {} |",
            ok.len(),
            100.0 * ok.len() as f64 / rows.len().max(1) as f64,
            count("error"),
            count("crash"),
            count("timeout"),
            ok.iter().filter(|r| r.page_errors > 0).count(),
            ok.iter().filter(|r| !r.tile_ok).count(),
        )
        .ok();
    }

    // Speed, on the files every engine drew, so the numbers compare.
    let common: Vec<&BTreeMap<String, Row>> = files
        .values()
        .filter(|m| {
            engines
                .iter()
                .all(|e| m.get(*e).is_some_and(|r| r.status == "ok"))
        })
        .collect();
    writeln!(
        out,
        "\n### Speed on the {} files all engines drew (ms; median / 95th percentile / worst)\n",
        common.len()
    )
    .ok();
    writeln!(
        out,
        "| Engine | Open + page 1 at 150% | Within 300 ms | Page at 150% | Within 150 ms | 1024² tile at 800% | Peak memory (MB, median) |"
    )
    .ok();
    writeln!(out, "|---|---|---|---|---|---|---|").ok();
    for e in &engines {
        let pick = |f: fn(&Row) -> f64| -> Vec<f64> { common.iter().map(|m| f(&m[*e])).collect() };
        let first = pick(|r| r.open_first);
        let page = pick(|r| r.mean_page);
        let tile = pick(|r| r.deep_tile);
        let rss = pick(|r| r.rss_mb);
        let within = |v: &[f64], b: f64| {
            100.0 * v.iter().filter(|x| **x <= b).count() as f64 / v.len().max(1) as f64
        };
        writeln!(
            out,
            "| {e} | {} | {:.1}% | {} | {:.1}% | {} | {:.0} |",
            spread(&first),
            within(&first, 300.0),
            spread(&page),
            within(&page, 150.0),
            spread(&tile),
            quantile(&rss, 0.5),
        )
        .ok();
    }
    if engines.contains(&"hayro") {
        writeln!(out, "\nhayro's time as a multiple of the others' (median over the same files; below 1 is faster):\n").ok();
        for other in engines.iter().filter(|e| **e != "hayro") {
            let ratio = |f: fn(&Row) -> f64| -> f64 {
                let v: Vec<f64> = common
                    .iter()
                    .map(|m| f(&m["hayro"]) / f(&m[*other]).max(0.01))
                    .collect();
                quantile(&v, 0.5)
            };
            writeln!(
                out,
                "- vs {other}: open + page 1 ×{:.2}, page ×{:.2}, deep tile ×{:.2}",
                ratio(|r| r.open_first),
                ratio(|r| r.mean_page),
                ratio(|r| r.deep_tile)
            )
            .ok();
        }
    }

    // Looks: page 1 pictures where one engine disagrees with the other two
    // while those two agree, which usually means that one drew it wrong.
    if engines.len() == 3 {
        writeln!(
            out,
            "\n### Odd one out on page 1 (the other two engines agree, this one differs by more than {DISAGREE} grey levels on average)\n"
        )
        .ok();
        let mut odd: BTreeMap<&str, Vec<(String, f64)>> = BTreeMap::new();
        for (name, m) in &files {
            if !engines
                .iter()
                .all(|e| m.get(*e).is_some_and(|r| r.status == "ok"))
            {
                continue;
            }
            for (i, e) in engines.iter().enumerate() {
                let a = engines[(i + 1) % 3];
                let b = engines[(i + 2) % 3];
                let others = diff(&m[a].thumb, &m[b].thumb);
                let mine = diff(&m[*e].thumb, &m[a].thumb).min(diff(&m[*e].thumb, &m[b].thumb));
                if others <= DISAGREE / 2.0 && mine > DISAGREE {
                    odd.entry(e).or_default().push((name.clone(), mine));
                }
            }
        }
        for e in &engines {
            let list = odd.get(e).cloned().unwrap_or_default();
            writeln!(out, "- **{e}**: {} files", list.len()).ok();
        }
        for e in &engines {
            let mut list = odd.get(e).cloned().unwrap_or_default();
            if list.is_empty() {
                continue;
            }
            list.sort_by(|a, b| b.1.total_cmp(&a.1));
            let names: Vec<String> = list
                .iter()
                .take(12)
                .map(|(n, d)| format!("{n} ({d:.0})"))
                .collect();
            writeln!(out, "\n{e}, worst first: {}", names.join(", ")).ok();
        }
    }

    // Files one engine cannot draw at all while every other can.
    writeln!(out, "\n### Files only this engine fails on\n").ok();
    for e in &engines {
        let only: Vec<String> = files
            .iter()
            .filter(|(_, m)| {
                m.get(*e).is_some_and(|r| r.status != "ok")
                    && engines
                        .iter()
                        .filter(|o| *o != e)
                        .all(|o| m.get(*o).is_some_and(|r| r.status == "ok"))
            })
            .map(|(n, m)| {
                let r = &m[*e];
                let why = if r.error.is_empty() {
                    r.status.clone()
                } else {
                    format!("{}: {}", r.status, r.error)
                };
                format!("{n} ({})", why.chars().take(60).collect::<String>())
            })
            .collect();
        writeln!(
            out,
            "- **{e}**: {} files{}",
            only.len(),
            if only.is_empty() {
                String::new()
            } else {
                format!(
                    ". {}",
                    only.iter().take(15).cloned().collect::<Vec<_>>().join("; ")
                )
            }
        )
        .ok();
    }
    Ok(out)
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .filter_map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn diff(a: &[u8], b: &[u8]) -> f64 {
    if a.len() != THUMB * THUMB || b.len() != THUMB * THUMB {
        return 255.0;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| (*x as f64 - *y as f64).abs())
        .sum::<f64>()
        / a.len() as f64
}

fn quantile(v: &[f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn spread(v: &[f64]) -> String {
    format!(
        "{:.1} / {:.1} / {:.0}",
        quantile(v, 0.5),
        quantile(v, 0.95),
        quantile(v, 1.0)
    )
}
