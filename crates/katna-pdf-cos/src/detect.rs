// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding the fields of a form that was only printed to PDF (docs/PLAN.md
//! §4.8, "automatic field detection"): blanks drawn as lines of
//! underscores or as rules to write on, empty boxes, rows of letter boxes
//! (PAN, IFSC, PIN code) and small squares to tick. Each becomes a field
//! named after the words beside or above it, ready to add with
//! [`Document::design_form`]. The page's own content is read; boxes drawn
//! inside placed forms (`Do`) are not looked for.

use std::collections::HashSet;

use crate::calc::Scripts;
use crate::design::{FieldSpec, NewField};
use crate::document::Document;
use crate::edit::TextRun;
use crate::objects::{IDENTITY, Matrix, apply, mul};

type Rect = [f32; 4];

/// Lines and boxes a page paints.
#[derive(Default)]
struct Shapes {
    /// Horizontal rules: left, right, height.
    rules: Vec<(f32, f32, f32)>,
    /// Vertical rules: x, bottom, top.
    uprights: Vec<(f32, f32, f32)>,
    boxes: Vec<Rect>,
}

impl Document {
    /// The fields page `page` seems to be asking for and does not have
    /// yet, top to bottom. Their names are made from the page's words
    /// and do not clash with each other or with `taken`.
    pub fn detect_fields(&self, page: usize, taken: &[String]) -> Vec<FieldSpec> {
        let Some(info) = self.page_info(page) else {
            return Vec::new();
        };
        let page_w = info.media[2] - info.media[0];
        let runs = self.text_runs(page);
        let shapes = self.shapes(page);
        let have: Vec<Rect> = self
            .form()
            .iter()
            .flat_map(|f| f.widgets.iter())
            .filter(|w| w.page == page)
            .map(|w| w.rect)
            .collect();
        let mut found: Vec<(Rect, NewField, Option<String>)> = Vec::new();

        // Blanks typed as underscores, and boxes typed as ☐.
        for run in &runs {
            let chars: Vec<char> = run.text.chars().collect();
            if chars.is_empty() {
                continue;
            }
            let step = (run.rect[2] - run.rect[0]) / chars.len() as f32;
            let x = |i: usize| run.rect[0] + step * i as f32;
            let mut i = 0;
            while i < chars.len() {
                if chars[i] == '_' {
                    let start = i;
                    while i < chars.len() && chars[i] == '_' {
                        i += 1;
                    }
                    if i - start >= 3 {
                        let rect = [
                            x(start),
                            run.rect[1] - 1.0,
                            x(i),
                            run.rect[1] + run.size * 1.2,
                        ];
                        let before: String = chars[..start].iter().collect();
                        let label = Some(before).filter(|b| !clean(b).is_empty());
                        found.push((rect, text_field(rect), label));
                    }
                    continue;
                }
                if matches!(chars[i], '☐' | '□' | '❑' | '▢') {
                    let s = run.size * 0.8;
                    let left = x(i) + (step - s) / 2.0;
                    let rect = [left, run.rect[1], left + s, run.rect[1] + s];
                    let after: String = chars[i + 1..].iter().collect();
                    let label = Some(after).filter(|a| !clean(a).is_empty());
                    found.push((rect, NewField::CheckBox, label));
                }
                i += 1;
            }
        }

        // Boxes with a caption in their top corner: the field is the
        // space under it.
        for b in &shapes.boxes {
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            if w >= 30.0
                && w < page_w * 0.9
                && (16.0..=72.0).contains(&h)
                && !shapes.boxes.iter().any(|o| o != b && inside(o, b))
                && let Some((rect, caption)) = captioned(&runs, b)
            {
                found.push((rect, text_field(rect), Some(caption)));
            }
        }

        // Boxes: letter-box rows, squares to tick, and empty boxes.
        let boxes: Vec<Rect> = shapes
            .boxes
            .iter()
            .copied()
            .filter(|b| {
                let (w, h) = (b[2] - b[0], b[3] - b[1]);
                w >= 5.0 && h >= 5.0 && w < page_w * 0.9 && h <= 60.0
            })
            .filter(|b| !holds_text(&runs, b))
            // A box around other boxes is a frame, not a blank.
            .filter(|b| !shapes.boxes.iter().any(|o| o != b && inside(o, b)))
            .collect();
        // Empty boxes side by side that are not letter boxes are a
        // table's cells.
        let cell = |b: &Rect| {
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            !((5.0..=22.0).contains(&w) && (w - h).abs() <= 2.0)
        };
        let boxes: Vec<Rect> = boxes
            .iter()
            .copied()
            .filter(|b| !cell(b) || !boxes.iter().any(|o| o != b && cell(o) && touching(o, b)))
            .collect();
        let mut used = vec![false; boxes.len()];
        let square = |b: &Rect| {
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            (5.0..=22.0).contains(&w) && (w - h).abs() <= 2.0
        };
        let mut order: Vec<usize> = (0..boxes.len()).collect();
        order.sort_by(|&a, &b| {
            boxes[b][1]
                .total_cmp(&boxes[a][1])
                .then(boxes[a][0].total_cmp(&boxes[b][0]))
        });
        for &i in &order {
            if used[i] || !square(&boxes[i]) {
                continue;
            }
            // Squares side by side, the same size, make one row.
            let mut row = vec![i];
            loop {
                let last = boxes[*row.last().unwrap_or(&i)];
                let next = order.iter().copied().find(|&j| {
                    !used[j]
                        && !row.contains(&j)
                        && square(&boxes[j])
                        && (boxes[j][1] - last[1]).abs() < 1.5
                        && (boxes[j][3] - last[3]).abs() < 1.5
                        && (boxes[j][0] - last[2]) > -1.5
                        && (boxes[j][0] - last[2]) < 4.0
                });
                match next {
                    Some(j) => row.push(j),
                    None => break,
                }
            }
            if row.len() >= 3 {
                for &j in &row {
                    used[j] = true;
                }
                let (first, last) = (boxes[row[0]], boxes[row[row.len() - 1]]);
                let rect = [first[0], first[1], last[2], first[3]];
                let field = NewField::Text {
                    multiline: false,
                    password: false,
                    max_len: Some(row.len()),
                };
                found.push((rect, field, None));
            } else {
                used[i] = true;
                // A lone square is a box to tick only beside words.
                if let Some(label) = label_for(&runs, &boxes[i], true) {
                    found.push((boxes[i], NewField::CheckBox, Some(label)));
                }
            }
        }
        for (i, b) in boxes.iter().enumerate() {
            let (w, h) = (b[2] - b[0], b[3] - b[1]);
            if used[i] || w < 30.0 || h < 10.0 {
                continue;
            }
            // An empty box with nothing to say what goes in it is a
            // table's cell or a frame.
            if let Some(label) = label_for(&runs, b, false) {
                found.push((*b, text_field(*b), Some(label)));
            }
        }

        // Cells of a grid drawn with lines, empty and named by the
        // words beside or over them.
        for &(x0, x1, y) in &shapes.rules {
            let mut posts: Vec<(f32, f32)> = shapes
                .uprights
                .iter()
                .filter(|&&(x, b, t)| {
                    x >= x0 - 1.5 && x <= x1 + 1.5 && b - 1.5 <= y && t - y >= 8.0
                })
                .map(|&(x, _, t)| (x, t))
                .collect();
            // A rule with one of the same span over it closes a cell at
            // its ends, drawn or not.
            if shapes.rules.iter().any(|&(r0, r1, ry)| {
                ry > y + 4.0 && ry - y <= 60.0 && (r0 - x0).abs() < 2.0 && (r1 - x1).abs() < 2.0
            }) {
                posts.push((x0, f32::MAX));
                posts.push((x1, f32::MAX));
            }
            posts.sort_by(|a, b| a.0.total_cmp(&b.0));
            posts.dedup_by(|a, b| (a.0 - b.0).abs() < 1.5);
            for pair in posts.windows(2) {
                let ((xa, ta), (xb, tb)) = (pair[0], pair[1]);
                if xb - xa < 20.0 {
                    continue;
                }
                // Up to the rule over the cell, if one is nearer.
                let roof = shapes
                    .rules
                    .iter()
                    .filter(|&&(r0, r1, ry)| ry > y + 4.0 && r0 <= xa + 1.5 && r1 >= xb - 1.5)
                    .map(|r| r.2)
                    .fold(ta.min(tb), f32::min);
                let cell = [xa, y, xb, roof];
                let h = roof - y;
                if !(8.0..=72.0).contains(&h) {
                    continue;
                }
                if holds_text(&runs, &cell) {
                    if let Some((rect, caption)) = captioned(&runs, &cell) {
                        found.push((rect, text_field(rect), Some(caption)));
                    }
                    continue;
                }
                if let Some(label) = label_for(&runs, &cell, false) {
                    found.push((cell, text_field(cell), Some(label)));
                }
            }
        }

        // Rules to write on: long enough, nothing written on them, and
        // not part of a table's grid.
        for &(x0, x1, y) in &shapes.rules {
            if x1 - x0 < 40.0 || x1 - x0 > page_w * 0.9 {
                continue;
            }
            let touches = |x: f32| {
                shapes
                    .uprights
                    .iter()
                    .any(|&(ux, b, t)| (ux - x).abs() < 2.0 && b - 2.0 <= y && y <= t + 2.0)
            };
            if touches(x0) || touches(x1) {
                continue;
            }
            let edge = 36.0;
            if y < info.media[1] + edge || y > info.media[3] - edge {
                continue;
            }
            let mid = y + 6.0;
            let Some(label) = runs
                .iter()
                .filter(|r| short(r) && r.rect[1] - 3.0 <= mid && mid <= r.rect[3] + 3.0)
                .filter(|r| r.rect[2] <= x0 + 2.0 && x0 - r.rect[2] < 60.0)
                .max_by(|a, b| a.rect[2].total_cmp(&b.rect[2]))
                .map(|r| clean(&r.text))
                .filter(|l| l.chars().any(char::is_alphanumeric))
            else {
                continue;
            };
            let rect = [x0, y, x1, y + 16.0];
            let above = runs
                .iter()
                .filter(|r| overlap(r.rect[0], r.rect[2], x0, x1) > 0.0)
                .map(|r| r.rect[1])
                .filter(|&b| b > y - 2.0)
                .fold(f32::MAX, f32::min);
            if above < y + 12.0 {
                continue;
            }
            let top = (y + 16.0).min(above - 1.0);
            found.push(([x0, y + 0.5, x1, top], text_field(rect), Some(label)));
        }

        // Not where a field is, or another found one already.
        let mut out: Vec<(Rect, NewField, Option<String>)> = Vec::new();
        for (rect, kind, label) in found {
            let clash = |r: &Rect| overlap_area(r, &rect) > 0.3 * area(&rect).min(area(r));
            if have.iter().any(clash) || out.iter().any(|(r, ..)| clash(r)) {
                continue;
            }
            out.push((rect, kind, label));
        }
        out.sort_by(|a, b| b.0[3].total_cmp(&a.0[3]).then(a.0[0].total_cmp(&b.0[0])));

        let mut names: HashSet<String> = taken.iter().cloned().collect();
        out.into_iter()
            .map(|(rect, kind, label)| {
                let check = matches!(kind, NewField::CheckBox);
                let base = label
                    .map(|l| clean(&l))
                    .filter(|l| !l.is_empty())
                    .or_else(|| label_for(&runs, &rect, check))
                    .unwrap_or_else(|| if check { "Check" } else { "Field" }.to_owned());
                let mut name = base.clone();
                let mut n = 2;
                while names.contains(&name) {
                    name = format!("{base} {n}");
                    n += 1;
                }
                names.insert(name.clone());
                FieldSpec {
                    page,
                    rect,
                    name,
                    kind,
                    tooltip: String::new(),
                    required: false,
                    read_only: false,
                    value: None,
                    scripts: Scripts::default(),
                }
            })
            .collect()
    }

    /// The rules and boxes page `page`'s content paints, in page space.
    fn shapes(&self, page: usize) -> Shapes {
        let mut out = Shapes::default();
        let Some(read) = self.page_content(page) else {
            return out;
        };
        let mut ctm: Matrix = IDENTITY;
        let mut stack = Vec::new();
        let mut in_text = false;
        // The path being built: finished subpaths and the open one.
        let mut subpaths: Vec<Vec<(f64, f64)>> = Vec::new();
        let mut boxes: Vec<Rect> = Vec::new();
        for op in &read.ops {
            let n = |k: usize| op.number(k).unwrap_or(0.0);
            match op.operator.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => ctm = stack.pop().unwrap_or(ctm),
                b"cm" if op.operands.len() == 6 => {
                    ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &ctm);
                }
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                b"m" => subpaths.push(vec![apply(&ctm, n(0), n(1))]),
                b"l" => match subpaths.last_mut() {
                    Some(sub) => sub.push(apply(&ctm, n(0), n(1))),
                    None => subpaths.push(vec![apply(&ctm, n(0), n(1))]),
                },
                // Curves are not blanks; they end the straight run.
                b"c" | b"v" | b"y" => subpaths.push(Vec::new()),
                b"re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    let corners = [
                        apply(&ctm, x, y),
                        apply(&ctm, x + w, y),
                        apply(&ctm, x + w, y + h),
                        apply(&ctm, x, y + h),
                    ];
                    boxes.push(bounds(&corners));
                }
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if !op.is("n") && !in_text {
                        // Filled shapes are rules when thin, else
                        // backgrounds and bars, not boxes to write in.
                        let stroked = !matches!(op.operator.as_slice(), b"f" | b"F" | b"f*");
                        for sub in subpaths.drain(..) {
                            out.add_path(&sub, stroked);
                        }
                        for b in boxes.drain(..) {
                            out.add_box(b, stroked);
                        }
                    }
                    subpaths.clear();
                    boxes.clear();
                }
                _ => {}
            }
        }
        out
    }
}

impl Shapes {
    /// A thin box is a rule; any other is a box.
    fn add_box(&mut self, b: Rect, stroked: bool) {
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        if h <= 2.0 && w > 2.0 {
            self.rules.push((b[0], b[2], (b[1] + b[3]) / 2.0));
        } else if w <= 2.0 && h > 2.0 {
            self.uprights.push(((b[0] + b[2]) / 2.0, b[1], b[3]));
        } else if w > 2.0 && h > 2.0 && stroked {
            self.boxes.push(b);
        }
    }

    /// A subpath of straight lines: a box if it goes round one, else its
    /// level and upright lines.
    fn add_path(&mut self, points: &[(f64, f64)], stroked: bool) {
        if (4..=5).contains(&points.len()) {
            let b = bounds(points);
            let square = points.iter().all(|&(x, y)| {
                let (x, y) = (x as f32, y as f32);
                ((x - b[0]).abs() < 0.5 || (x - b[2]).abs() < 0.5)
                    && ((y - b[1]).abs() < 0.5 || (y - b[3]).abs() < 0.5)
            });
            if square && b[2] - b[0] > 2.0 && b[3] - b[1] > 2.0 {
                self.add_box(b, stroked);
                return;
            }
        }
        for pair in points.windows(2) {
            let [(x0, y0), (x1, y1)] = [pair[0], pair[1]].map(|(x, y)| (x as f32, y as f32));
            if (y0 - y1).abs() < 0.5 && (x0 - x1).abs() > 2.0 {
                self.rules.push((x0.min(x1), x0.max(x1), y0));
            } else if (x0 - x1).abs() < 0.5 && (y0 - y1).abs() > 2.0 {
                self.uprights.push((x0, y0.min(y1), y0.max(y1)));
            }
        }
    }
}

fn bounds(points: &[(f64, f64)]) -> Rect {
    let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for &(x, y) in points {
        let (x, y) = (x as f32, y as f32);
        r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
    }
    r
}

fn text_field(rect: Rect) -> NewField {
    NewField::Text {
        multiline: rect[3] - rect[1] > 36.0,
        password: false,
        max_len: None,
    }
}

fn overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).max(0.0)
}

fn area(r: &Rect) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn overlap_area(a: &Rect, b: &Rect) -> f32 {
    overlap(a[0], a[2], b[0], b[2]) * overlap(a[1], a[3], b[1], b[3])
}

/// Whether `inner` lies within `outer`.
fn inside(inner: &Rect, outer: &Rect) -> bool {
    inner[0] >= outer[0] - 0.5
        && inner[1] >= outer[1] - 0.5
        && inner[2] <= outer[2] + 0.5
        && inner[3] <= outer[3] + 0.5
}

/// Whether boxes `a` and `b` share part of a side.
fn touching(a: &Rect, b: &Rect) -> bool {
    let near = |p: f32, q: f32| (p - q).abs() < 1.5;
    let across = overlap(a[1], a[3], b[1], b[3]) > 2.0 && (near(a[2], b[0]) || near(b[2], a[0]));
    let along = overlap(a[0], a[2], b[0], b[2]) > 2.0 && (near(a[3], b[1]) || near(b[3], a[1]));
    across || along
}

/// A box whose only words are a caption along its top: the space under
/// the caption, and the caption.
fn captioned(runs: &[TextRun], b: &Rect) -> Option<(Rect, String)> {
    let within: Vec<&TextRun> = runs
        .iter()
        .filter(|r| !r.text.trim().is_empty() && overlap_area(&r.rect, b) > 0.25 * area(&r.rect))
        .collect();
    let floor = within.iter().map(|r| r.rect[1]).fold(f32::MAX, f32::min);
    let h = b[3] - b[1];
    if within.is_empty()
        || floor < b[1] + (h * 0.45).max(10.0)
        || within.iter().any(|r| r.size > 10.0)
    {
        return None;
    }
    let mut words: Vec<&TextRun> = within;
    words.sort_by(|a, b| {
        b.rect[3]
            .total_cmp(&a.rect[3])
            .then(a.rect[0].total_cmp(&b.rect[0]))
    });
    let caption = clean(
        &words
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    let rect = [b[0] + 1.0, b[1] + 1.0, b[2] - 1.0, floor - 1.0];
    (rect[3] - rect[1] >= 8.0 && caption.chars().any(char::is_alphanumeric))
        .then_some((rect, caption))
}

/// Whether `run` reads like a label rather than a line of a paragraph.
fn short(run: &TextRun) -> bool {
    run.text
        .split_whitespace()
        .filter(|w| !w.chars().all(|c| c == '_'))
        .count()
        <= 8
}

/// Whether some of the page's words are written inside `b`.
fn holds_text(runs: &[TextRun], b: &Rect) -> bool {
    runs.iter().any(|r| {
        !r.text.trim().is_empty()
            && !r.text.chars().all(|c| c == '_')
            && overlap_area(&r.rect, b) > 0.25 * area(&r.rect).min(area(b))
    })
}

/// The words that name a field at `rect`: those just left of it on its
/// line, else those over it; for a check box, those just right of it
/// first.
fn label_for(runs: &[TextRun], rect: &Rect, check: bool) -> Option<String> {
    let mid = (rect[1] + rect[3]) / 2.0;
    let on_line = |r: &&TextRun| short(r) && r.rect[1] - 3.0 <= mid && mid <= r.rect[3] + 3.0;
    let left = runs
        .iter()
        .filter(on_line)
        .filter(|r| r.rect[2] <= rect[0] + 2.0 && rect[0] - r.rect[2] < 150.0)
        .max_by(|a, b| a.rect[2].total_cmp(&b.rect[2]));
    let right = runs
        .iter()
        .filter(on_line)
        .filter(|r| r.rect[0] >= rect[2] - 2.0 && r.rect[0] - rect[2] < 40.0)
        .min_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    let above = runs
        .iter()
        .filter(|r| {
            short(r)
                && r.rect[1] >= rect[3] - 2.0
                && r.rect[1] - rect[3] < 20.0
                && overlap(r.rect[0], r.rect[2], rect[0], rect[2]) > 0.0
        })
        .min_by(|a, b| a.rect[1].total_cmp(&b.rect[1]));
    let pick = if check {
        right.or(left).or(above)
    } else {
        left.or(above)
    };
    // For words left of the blank, the last part of the run, after any
    // earlier blank.
    pick.map(|r| clean(r.text.rsplit("___").next().unwrap_or(&r.text)))
        .filter(|l| l.chars().any(char::is_alphanumeric))
}

/// Words as a field's name: no blanks, dots, colons or stars, at most
/// about 40 characters.
fn clean(text: &str) -> String {
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == '_' || c == '.')
        .filter(|w| !w.is_empty())
        .collect();
    let mut name = String::new();
    for word in words {
        if !name.is_empty() && name.len() + word.len() > 40 {
            break;
        }
        if !name.is_empty() {
            name.push(' ');
        }
        name.push_str(word);
    }
    name.trim_matches(|c: char| c == ':' || c == '*' || c.is_whitespace() || c == '☐')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean("Name of applicant:  ____"), "Name of applicant");
        assert_eq!(clean("  Date *"), "Date");
        assert_eq!(clean("PAN No."), "PAN No");
    }
}
