// SPDX-License-Identifier: GPL-3.0-or-later

//! The pictures and shapes a page draws, as objects to pick, move,
//! resize and delete (docs/PLAN.md §4.3, "move, resize and delete
//! objects"): each picture (`Do` of an image, an inline image), each
//! form a page places (`Do` of a form), and each path it paints, with
//! where it is on the page. Moving or resizing wraps the object's
//! operators in `q` … `cm` … `Q`; deleting takes them out. Everything
//! else in the content stays byte for byte, and the change is an
//! incremental update. Text is edited as text (`edit.rs`).

use std::ops::RangeInclusive;

use crate::content::{Op, ops, splice};
use crate::document::Document;
use crate::edit::EditError;
use crate::filter::decode;
use crate::object::{Dict, Object};
use crate::pages::page_dict;

/// Pages with more objects than this (drawings of many thousand strokes)
/// list none: picking one stroke among them is not what moving is for.
const MOST: usize = 20_000;

pub(crate) type Matrix = [f64; 6];
/// A path being built: its first operator, the matrix then, and its points.
type OpenPath = (usize, Matrix, Vec<(f64, f64)>);
pub(crate) const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b`.
pub(crate) fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub(crate) fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let a = m[3] / det;
    let b = -m[1] / det;
    let c = -m[2] / det;
    let d = m[0] / det;
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Picture,
    /// A form XObject: a drawing of its own the page places, often a
    /// logo or a stamp.
    Form,
    Path,
}

/// A picture or shape on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageObject {
    pub kind: ObjectKind,
    /// Where it is, in the page's own space: left, bottom, right, top.
    pub rect: [f32; 4],
    /// Its operators, by index.
    ops: RangeInclusive<usize>,
    /// The graphics state's matrix as it starts.
    ctm: Matrix,
}

/// A page's content and operators, read.
pub(crate) struct Content {
    content: Vec<u8>,
    pub(crate) ops: Vec<Op>,
}

impl Document {
    /// The pictures, placed forms and painted shapes page `page` draws in
    /// its own content, in drawing order (so the last is on top).
    pub fn page_objects(&self, page: usize) -> Vec<PageObject> {
        let Some(read) = self.page_content(page) else {
            return Vec::new();
        };
        let forms = self.form_boxes(page);
        let mut out = Vec::new();
        let mut ctm = IDENTITY;
        let mut stack = Vec::new();
        let mut in_text = false;
        // The path being built: where it started and its points.
        let mut path: Option<OpenPath> = None;
        let mut clipping = false;
        for (i, op) in read.ops.iter().enumerate() {
            let n = |k: usize| op.number(k).unwrap_or(0.0);
            let point = |x: f64, y: f64, path: &mut Option<OpenPath>| {
                let (_, m, points) = path.get_or_insert((i, ctm, Vec::new()));
                points.push(apply(m, x, y));
            };
            match op.operator.as_slice() {
                b"q" => stack.push(ctm),
                b"Q" => ctm = stack.pop().unwrap_or(ctm),
                b"cm" if op.operands.len() == 6 => {
                    ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &ctm);
                }
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                b"m" | b"l" => point(n(0), n(1), &mut path),
                b"c" => {
                    point(n(0), n(1), &mut path);
                    point(n(2), n(3), &mut path);
                    point(n(4), n(5), &mut path);
                }
                b"v" | b"y" => {
                    point(n(0), n(1), &mut path);
                    point(n(2), n(3), &mut path);
                }
                b"re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    point(x, y, &mut path);
                    point(x + w, y + h, &mut path);
                }
                b"W" | b"W*" => clipping = true,
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if let Some((start, m, points)) = path.take()
                        && !clipping
                        && !op.is("n")
                        && !in_text
                    {
                        out.push(PageObject {
                            kind: ObjectKind::Path,
                            rect: bounds(&points),
                            ops: start..=i,
                            ctm: m,
                        });
                    }
                    clipping = false;
                }
                b"Do" if !in_text => {
                    let name = op.operands.first().and_then(Object::as_name);
                    let form = name.and_then(|n| forms.iter().find(|(k, _)| k == n));
                    let (kind, rect) = match form {
                        Some((_, Some(b))) => (ObjectKind::Form, *b),
                        Some((_, None)) => continue,
                        None => (ObjectKind::Picture, [0.0, 0.0, 1.0, 1.0]),
                    };
                    let corners = [
                        apply(&ctm, rect[0], rect[1]),
                        apply(&ctm, rect[2], rect[1]),
                        apply(&ctm, rect[0], rect[3]),
                        apply(&ctm, rect[2], rect[3]),
                    ];
                    out.push(PageObject {
                        kind,
                        rect: bounds(&corners),
                        ops: i..=i,
                        ctm,
                    });
                }
                b"BI" if !in_text => {
                    let corners = [
                        apply(&ctm, 0.0, 0.0),
                        apply(&ctm, 1.0, 0.0),
                        apply(&ctm, 0.0, 1.0),
                        apply(&ctm, 1.0, 1.0),
                    ];
                    out.push(PageObject {
                        kind: ObjectKind::Picture,
                        rect: bounds(&corners),
                        ops: i..=i,
                        ctm,
                    });
                }
                _ => {}
            }
            if out.len() > MOST {
                return Vec::new();
            }
        }
        out
    }

    /// The file with object `index` of `page_objects(page)` moved and
    /// sized by `matrix`, in the page's own space (its points p go to
    /// p × matrix).
    pub fn transform_object(
        &self,
        page: usize,
        index: usize,
        matrix: [f32; 6],
    ) -> Result<Vec<u8>, EditError> {
        let object = self
            .page_objects(page)
            .into_iter()
            .nth(index)
            .ok_or(EditError::NoRun)?;
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let m = matrix.map(f64::from);
        // Inside, the object's points go through what is set there, then
        // `x`: so that is the start's matrix, then `m`, then back.
        let back = invert(&object.ctm).ok_or(EditError::NoRun)?;
        let x = mul(&mul(&object.ctm, &m), &back);
        let first = read.ops[*object.ops.start()].span.start;
        let last = read.ops[*object.ops.end()].span.end;
        let mut before = b"q ".to_vec();
        for v in x {
            before.extend_from_slice(number(v).as_bytes());
            before.push(b' ');
        }
        before.extend_from_slice(b"cm ");
        let content = splice(
            &read.content,
            vec![(first..first, before), (last..last, b" Q".to_vec())],
        );
        self.write_page_content(page, &content)
    }

    /// The file with object `index` of `page_objects(page)` taken out.
    pub fn delete_object(&self, page: usize, index: usize) -> Result<Vec<u8>, EditError> {
        let object = self
            .page_objects(page)
            .into_iter()
            .nth(index)
            .ok_or(EditError::NoRun)?;
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let first = read.ops[*object.ops.start()].span.start;
        let last = read.ops[*object.ops.end()].span.end;
        // What set the state for it (colours, line widths) is kept, as
        // what follows may count on it.
        let mut kept = Vec::new();
        for op in &read.ops[object.ops.clone()] {
            if !is_path_or_paint(op) && !op.is("Do") && !op.is("BI") {
                kept.extend_from_slice(&read.content[op.span.clone()]);
                kept.push(b' ');
            }
        }
        let content = splice(&read.content, vec![(first..last, kept)]);
        self.write_page_content(page, &content)
    }

    pub(crate) fn page_content(&self, page: usize) -> Option<Content> {
        let dict = page_dict(self, *self.pages().get(page)?);
        let streams = match dict.get("Contents").map(|c| self.resolve(c)) {
            Some(Object::Array(a)) => a.iter().map(|s| self.resolve(s)).collect(),
            Some(s @ Object::Stream(_)) => vec![s],
            _ => Vec::new(),
        };
        let mut content = Vec::new();
        for s in streams {
            let Object::Stream(s) = s else {
                continue;
            };
            content.extend(decode(&s.dict, &s.data).ok()?);
            content.push(b'\n');
        }
        let ops = ops(&content);
        Some(Content { content, ops })
    }

    /// The page's XObjects by name: forms with their boxes in form space,
    /// other XObjects not listed (pictures). A form without a box is
    /// listed with none.
    fn form_boxes(&self, page: usize) -> Vec<(Vec<u8>, Option<[f64; 4]>)> {
        let Some(page_ref) = self.pages().get(page).copied() else {
            return Vec::new();
        };
        let dict = page_dict(self, page_ref);
        let resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let Object::Dict(xobjects) = self.resolve_key(&resources, "XObject") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (name, value) in xobjects.iter() {
            let Object::Stream(s) = self.resolve(value) else {
                continue;
            };
            if s.dict.name("Subtype") != Some(b"Form") {
                continue;
            }
            let bbox = match self.resolve_key(&s.dict, "BBox") {
                Object::Array(a) => {
                    let n: Vec<f64> = a
                        .iter()
                        .filter_map(|v| self.resolve(v).as_number())
                        .collect();
                    match n[..] {
                        [a, b, c, d] => {
                            // The form's own matrix places its box.
                            let m = match self.resolve_key(&s.dict, "Matrix") {
                                Object::Array(m) => {
                                    let m: Vec<f64> = m
                                        .iter()
                                        .filter_map(|v| self.resolve(v).as_number())
                                        .collect();
                                    <[f64; 6]>::try_from(m).unwrap_or(IDENTITY)
                                }
                                _ => IDENTITY,
                            };
                            let r = bounds(&[
                                apply(&m, a, b),
                                apply(&m, c, b),
                                apply(&m, a, d),
                                apply(&m, c, d),
                            ]);
                            Some(r.map(f64::from))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            out.push((name.as_bytes().to_vec(), bbox));
        }
        out
    }

    /// The file with page `page`'s content replaced by `content`.
    fn write_page_content(&self, page: usize, content: &[u8]) -> Result<Vec<u8>, EditError> {
        let page_ref = *self.pages().get(page).ok_or(EditError::NoPage)?;
        let mut update = self.update()?;
        let stream = update.add(Object::Stream(crate::dss::compressed(content)));
        let mut dict = self
            .get(page_ref)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        dict.set("Contents", Object::Ref(stream));
        update.replace(page_ref, Object::Dict(dict));
        Ok(update.finish())
    }
}

fn is_path_or_paint(op: &Op) -> bool {
    matches!(
        op.operator.as_slice(),
        b"m" | b"l"
            | b"c"
            | b"v"
            | b"y"
            | b"h"
            | b"re"
            | b"S"
            | b"s"
            | b"f"
            | b"F"
            | b"f*"
            | b"B"
            | b"B*"
            | b"b"
            | b"b*"
            | b"n"
            | b"W"
            | b"W*"
    )
}

fn bounds(points: &[(f64, f64)]) -> [f32; 4] {
    let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &(x, y) in points {
        r[0] = r[0].min(x);
        r[1] = r[1].min(y);
        r[2] = r[2].max(x);
        r[3] = r[3].max(y);
    }
    if points.is_empty() {
        r = [0.0; 4];
    }
    r.map(|v| v as f32)
}

fn number(v: f64) -> String {
    let rounded = (v * 10_000.0).round() / 10_000.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}
