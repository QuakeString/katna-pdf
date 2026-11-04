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
use crate::object::{Dict, Object, Ref};
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

/// How a shape is painted.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeStyle {
    /// Its inside's colour (RGB, 0 to 1), when it is filled.
    pub fill: Option<[f32; 3]>,
    /// Its line's colour, when it is stroked.
    pub stroke: Option<[f32; 3]>,
    /// Its line's width, in points on the page.
    pub width: f32,
    /// Its line's dashes and gaps, in points: none for a solid line.
    pub dash: Vec<f32>,
    /// How its line ends: 0 cut off, 1 round, 2 squared off past the end.
    pub cap: u8,
    /// How its line turns corners: 0 pointed, 1 round, 2 cut off.
    pub join: u8,
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
    /// How it is painted, for a shape.
    pub style: Option<ShapeStyle>,
    /// Whether a clipping path cuts it down.
    pub clipped: bool,
    /// A shape's corners and the points its curves end at, in the page's
    /// own space, in order: the points that can be moved.
    pub points: Vec<[f32; 2]>,
    /// Where each of `points` is written: its operator, and the operand
    /// its x is (`usize::MAX - 1` and `usize::MAX` for a rectangle's
    /// corner and its opposite).
    point_ops: Vec<(usize, usize)>,
}

/// The graphics state read along a page's content, as far as shapes are
/// painted with it.
#[derive(Clone)]
struct Painting {
    fill: [f32; 3],
    stroke: [f32; 3],
    width: f64,
    dash: Vec<f64>,
    cap: u8,
    join: u8,
    clipped: bool,
}

impl Default for Painting {
    fn default() -> Self {
        Painting {
            fill: [0.0; 3],
            stroke: [0.0; 3],
            width: 1.0,
            dash: Vec::new(),
            cap: 0,
            join: 0,
            clipped: false,
        }
    }
}

/// A colour from a colour operator's numbers: gray, RGB or CMYK.
fn colour_of(op: &Op) -> Option<[f32; 3]> {
    let n: Vec<f32> = (0..op.operands.len())
        .filter_map(|k| op.number(k))
        .map(|v| v.clamp(0.0, 1.0) as f32)
        .collect();
    match n[..] {
        [g] => Some([g, g, g]),
        [r, g, b] => Some([r, g, b]),
        [c, m, y, k] => Some([
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        ]),
        _ => None,
    }
}

/// How much a matrix scales lengths.
fn scale_of(m: &Matrix) -> f64 {
    (m[0] * m[3] - m[1] * m[2]).abs().sqrt().max(1e-9)
}

impl PageObject {
    /// How far a picture or placed form is turned from upright, in
    /// degrees anticlockwise (0 for a shape, whose points are its own).
    pub fn angle(&self) -> f32 {
        match self.kind {
            ObjectKind::Path => 0.0,
            _ => {
                let degrees = self.ctm[1].atan2(self.ctm[0]).to_degrees();
                let rounded = (degrees * 10.0).round() / 10.0;
                if rounded == -0.0 { 0.0 } else { rounded as f32 }
            }
        }
    }
}

/// A page's content and operators, read.
pub(crate) struct Content {
    pub(crate) content: Vec<u8>,
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
        let mut paint = Painting::default();
        let mut stack = Vec::new();
        let mut in_text = false;
        // The path being built: where it started and its points.
        let mut path: Option<OpenPath> = None;
        // Its corners and curve ends, and where each is written.
        let mut anchors: Vec<[f32; 2]> = Vec::new();
        let mut anchor_ops: Vec<(usize, usize)> = Vec::new();
        let mut clipping = false;
        for (i, op) in read.ops.iter().enumerate() {
            let n = |k: usize| op.number(k).unwrap_or(0.0);
            let point = |x: f64, y: f64, path: &mut Option<OpenPath>| {
                let (_, m, points) = path.get_or_insert((i, ctm, Vec::new()));
                points.push(apply(m, x, y));
            };
            let mut anchor = |x: f64, y: f64, k: usize| {
                let (px, py) = apply(&ctm, x, y);
                anchors.push([px as f32, py as f32]);
                anchor_ops.push((i, k));
            };
            match op.operator.as_slice() {
                b"q" => stack.push((ctm, paint.clone())),
                b"Q" => (ctm, paint) = stack.pop().unwrap_or((ctm, paint)),
                b"cm" if op.operands.len() == 6 => {
                    ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &ctm);
                }
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                b"rg" | b"g" | b"k" | b"sc" | b"scn" => {
                    if let Some(c) = colour_of(op) {
                        paint.fill = c;
                    }
                }
                b"RG" | b"G" | b"K" | b"SC" | b"SCN" => {
                    if let Some(c) = colour_of(op) {
                        paint.stroke = c;
                    }
                }
                b"w" => paint.width = n(0),
                b"J" => paint.cap = n(0) as u8,
                b"j" => paint.join = n(0) as u8,
                b"d" => {
                    paint.dash = match op.operands.first() {
                        Some(Object::Array(a)) => a.iter().filter_map(Object::as_number).collect(),
                        _ => Vec::new(),
                    };
                }
                b"m" | b"l" => {
                    point(n(0), n(1), &mut path);
                    anchor(n(0), n(1), 0);
                }
                b"c" => {
                    point(n(0), n(1), &mut path);
                    point(n(2), n(3), &mut path);
                    point(n(4), n(5), &mut path);
                    anchor(n(4), n(5), 4);
                }
                b"v" | b"y" => {
                    point(n(0), n(1), &mut path);
                    point(n(2), n(3), &mut path);
                    anchor(n(2), n(3), 2);
                }
                b"re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    point(x, y, &mut path);
                    point(x + w, y + h, &mut path);
                    anchor(x, y, usize::MAX - 1);
                    anchor(x + w, y + h, usize::MAX);
                }
                b"W" | b"W*" => clipping = true,
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if let Some((start, m, points)) = path.take()
                        && !clipping
                        && !op.is("n")
                        && !in_text
                    {
                        let o = op.operator.as_slice();
                        let filled = matches!(o, b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*");
                        let stroked = matches!(o, b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*");
                        let scale = scale_of(&m);
                        out.push(PageObject {
                            kind: ObjectKind::Path,
                            rect: bounds(&points),
                            ops: start..=i,
                            ctm: m,
                            style: Some(ShapeStyle {
                                fill: filled.then_some(paint.fill),
                                stroke: stroked.then_some(paint.stroke),
                                width: (paint.width * scale) as f32,
                                dash: paint.dash.iter().map(|d| (d * scale) as f32).collect(),
                                cap: paint.cap,
                                join: paint.join,
                            }),
                            clipped: paint.clipped,
                            points: std::mem::take(&mut anchors),
                            point_ops: std::mem::take(&mut anchor_ops),
                        });
                    }
                    if clipping {
                        paint.clipped = true;
                    }
                    anchors.clear();
                    anchor_ops.clear();
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
                        style: None,
                        clipped: paint.clipped,
                        points: Vec::new(),
                        point_ops: Vec::new(),
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
                        style: None,
                        clipped: paint.clipped,
                        points: Vec::new(),
                        point_ops: Vec::new(),
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
        self.transform_objects(page, &[(index, matrix)])
    }

    /// The file with each object of `page_objects(page)` in `moves` moved
    /// and sized by its matrix, as [`Document::transform_object`] does,
    /// in one change.
    pub fn transform_objects(
        &self,
        page: usize,
        moves: &[(usize, [f32; 6])],
    ) -> Result<Vec<u8>, EditError> {
        let objects = self.page_objects(page);
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let mut moves: Vec<_> = moves.to_vec();
        moves.sort_by_key(|(i, _)| *i);
        moves.dedup_by_key(|(i, _)| *i);
        let mut edits = Vec::new();
        for (index, matrix) in moves {
            let object = objects.get(index).ok_or(EditError::NoRun)?;
            let m = matrix.map(f64::from);
            // Inside, the object's points go through what is set there,
            // then `x`: so that is the start's matrix, then `m`, then
            // back.
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
            edits.push((first..first, before));
            edits.push((last..last, b" Q".to_vec()));
        }
        let content = splice(&read.content, edits);
        self.write_page_content(page, &content)
    }

    /// A copy of object `index` of `page_objects(page)`, to paste onto a
    /// page of this or another file with [`Document::paste_objects`].
    pub fn copy_object(&self, page: usize, index: usize) -> Result<ObjectCopy, EditError> {
        let object = self
            .page_objects(page)
            .into_iter()
            .nth(index)
            .ok_or(EditError::NoRun)?;
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let page_ref = *self.pages().get(page).ok_or(EditError::NoPage)?;
        let dict = page_dict(self, page_ref);
        let resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        // Its operators with the state they were drawn in, from the
        // page's own space.
        let mut content = Vec::new();
        let mut used = Vec::new();
        let mut taken = Taken::default();
        let mut indices = state_at(&read.ops, *object.ops.start());
        indices.extend(object.ops.clone());
        for i in indices {
            let op = &read.ops[i];
            if let Some((category, at)) = resource_operand(op)
                && let Some(name) = op.operands.get(at).and_then(Object::as_name)
            {
                let name = String::from_utf8_lossy(name).into_owned();
                if !used
                    .iter()
                    .any(|r: &CopiedResource| r.category == category && r.name == name)
                {
                    let value = match self.resolve_key(&resources, category) {
                        Object::Dict(d) => d.get(&name).cloned().unwrap_or(Object::Null),
                        _ => Object::Null,
                    };
                    let value = taken.take(self, &value);
                    used.push(CopiedResource {
                        category: category.to_string(),
                        name,
                        value,
                    });
                }
            }
            content.extend_from_slice(&read.content[op.span.clone()]);
            content.push(b' ');
        }
        Ok(ObjectCopy {
            kind: object.kind,
            rect: object.rect,
            content,
            resources: used,
            objects: taken.objects,
        })
    }

    /// The file with `copies` drawn on page `page` over what it draws,
    /// each moved by `by` (across, up) from where it was copied.
    pub fn paste_objects(
        &self,
        page: usize,
        copies: &[&ObjectCopy],
        by: [f32; 2],
    ) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        let mut add = crate::insert::PageAdd::new(self, page)?;
        let mut drawing = Vec::new();
        for copy in copies {
            // Its objects, brought in: those this file already holds
            // unchanged are used as they are.
            let mut refs = Vec::with_capacity(copy.objects.len());
            for (source, original, _) in &copy.objects {
                let kept = source.filter(|r| self.get(*r).as_ref() == Some(original));
                refs.push(kept.unwrap_or_else(|| update.add(Object::Null)));
            }
            for ((source, original, local), r) in copy.objects.iter().zip(&refs) {
                if source.is_some_and(|s| s == *r) && self.get(*r).as_ref() == Some(original) {
                    continue;
                }
                update.replace(*r, renumber(local, &refs));
            }
            let mut renamed = Vec::new();
            for resource in &copy.resources {
                let value = renumber(&resource.value, &refs);
                let key = add.resource(&resource.category, &resource.name, value);
                if key != resource.name {
                    renamed.push((resource.category.as_str(), resource.name.as_str(), key));
                }
            }
            drawing.extend_from_slice(
                format!(
                    "q 1 0 0 1 {} {} cm ",
                    number(by[0].into()),
                    number(by[1].into())
                )
                .as_bytes(),
            );
            if renamed.is_empty() {
                drawing.extend_from_slice(&copy.content);
            } else {
                for op in ops(&copy.content) {
                    let mut op = op;
                    if let Some((category, at)) = resource_operand(&op)
                        && let Some(name) = op.operands.get(at).and_then(Object::as_name)
                        && let Some((_, _, key)) = renamed
                            .iter()
                            .find(|(c, n, _)| *c == category && n.as_bytes() == name)
                    {
                        op.operands[at] = Object::name(key);
                    }
                    crate::content::write_op(&mut drawing, &op);
                    drawing.push(b' ');
                }
            }
            drawing.extend_from_slice(b"Q\n");
        }
        add.finish(&mut update, &drawing, false);
        Ok(update.finish())
    }

    /// The file with object `index` of `page_objects(page)` changed as
    /// `change` says.
    pub fn change_object(
        &self,
        page: usize,
        index: usize,
        change: &ObjectChange,
    ) -> Result<Vec<u8>, EditError> {
        let object = self
            .page_objects(page)
            .into_iter()
            .nth(index)
            .ok_or(EditError::NoRun)?;
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let first = read.ops[*object.ops.start()].span.start;
        let last = read.ops[*object.ops.end()].span.end;
        let body = &read.content[first..last];
        let mut update = self.update()?;
        let mut add = crate::insert::PageAdd::new(self, page)?;
        // A see-through or colour wrapper this put round the object
        // before is changed rather than wrapped again (the inner one would
        // win): its `q`, and the operators in it.
        let start = *object.ops.start();
        let mut own = Vec::new();
        let mut at = start;
        while at > 0 {
            let op = &read.ops[at - 1];
            let ours = op.is("rg")
                || op.is("RG")
                || op.is("w")
                || op.is("d")
                || op.is("J")
                || op.is("j")
                || (op.is("gs")
                    && op
                        .operands
                        .first()
                        .and_then(Object::as_name)
                        .is_some_and(|n| n.starts_with(b"KatnaG")));
            if !ours {
                break;
            }
            own.push(at - 1);
            at -= 1;
        }
        let wrapped = at > 0
            && read.ops[at - 1].is("q")
            && read
                .ops
                .get(*object.ops.end() + 1)
                .is_some_and(|op| op.is("Q"));
        let wrap_edits = |keep: &dyn Fn(&Op) -> bool, new: String| {
            if wrapped {
                let mut before = b"q ".to_vec();
                for &i in own.iter().rev() {
                    if keep(&read.ops[i]) {
                        before.extend_from_slice(&read.content[read.ops[i].span.clone()]);
                        before.push(b' ');
                    }
                }
                before.extend_from_slice(new.as_bytes());
                let from = read.ops[at - 1].span.start;
                vec![(from..first, before)]
            } else {
                vec![
                    (first..first, format!("q {new}").into_bytes()),
                    (last..last, b" Q".to_vec()),
                ]
            }
        };
        let wrap =
            |keep: &dyn Fn(&Op) -> bool, new: String| splice(&read.content, wrap_edits(keep, new));
        let content = match change {
            ObjectChange::Front | ObjectChange::Back => {
                // Moved, it takes along the state it was drawn in.
                let mut moved = b"q ".to_vec();
                for &i in &state_at(&read.ops, *object.ops.start()) {
                    moved.extend_from_slice(&read.content[read.ops[i].span.clone()]);
                    moved.push(b' ');
                }
                moved.extend_from_slice(body);
                moved.extend_from_slice(b" Q\n");
                let mut kept = Vec::new();
                for op in &read.ops[object.ops.clone()] {
                    if !is_path_or_paint(op) && !op.is("Do") && !op.is("BI") {
                        kept.extend_from_slice(&read.content[op.span.clone()]);
                        kept.push(b' ');
                    }
                }
                let rest = splice(&read.content, vec![(first..last, kept)]);
                if matches!(change, ObjectChange::Front) {
                    let mut out = b"q\n".to_vec();
                    out.extend_from_slice(&rest);
                    out.extend_from_slice(b"\nQ\n");
                    out.extend_from_slice(&moved);
                    out
                } else {
                    let mut out = moved;
                    out.extend_from_slice(&rest);
                    out
                }
            }
            ObjectChange::Forward | ObjectChange::Backward => {
                let objects = self.page_objects(page);
                let forward = matches!(change, ObjectChange::Forward);
                let neighbour = if forward {
                    objects.get(index + 1)
                } else {
                    index.checked_sub(1).and_then(|i| objects.get(i))
                };
                let Some(neighbour) = neighbour else {
                    return self.write_page_content(page, &read.content);
                };
                // Where it goes: past the neighbour, and out of any group
                // (`q` … `Q`, often a clip) the neighbour alone is in.
                let at = if forward {
                    let mut at = *neighbour.ops.end() + 1;
                    while read.ops.get(at).is_some_and(|op| op.is("Q")) {
                        at += 1;
                    }
                    at
                } else {
                    let mut at = *neighbour.ops.start();
                    while at > 0 && sets_up(&read.ops[at - 1]) {
                        at -= 1;
                    }
                    at
                };
                let there = read
                    .ops
                    .get(at)
                    .map_or(read.content.len(), |op| op.span.start);
                // Drawn there, in the page's own space, with the state it
                // was drawn in.
                let undo = invert(&ctm_at(&read.ops, at)).unwrap_or(IDENTITY);
                let mut moved = b"q ".to_vec();
                for v in undo {
                    moved.extend_from_slice(number(v).as_bytes());
                    moved.push(b' ');
                }
                moved.extend_from_slice(b"cm ");
                for &i in &state_at(&read.ops, *object.ops.start()) {
                    moved.extend_from_slice(&read.content[read.ops[i].span.clone()]);
                    moved.push(b' ');
                }
                moved.extend_from_slice(body);
                moved.extend_from_slice(b" Q\n");
                let mut kept = Vec::new();
                for op in &read.ops[object.ops.clone()] {
                    if !is_path_or_paint(op) && !op.is("Do") && !op.is("BI") {
                        kept.extend_from_slice(&read.content[op.span.clone()]);
                        kept.push(b' ');
                    }
                }
                splice(
                    &read.content,
                    vec![(first..last, kept), (there..there, moved)],
                )
            }
            ObjectChange::Opacity(opacity) => {
                let key = add.opacity(&mut update, opacity.clamp(0.0, 1.0));
                wrap(&|op| !op.is("gs"), format!("/{key} gs "))
            }
            ObjectChange::Style {
                fill,
                stroke,
                width,
                dash,
                cap,
                join,
            } => {
                if object.kind != ObjectKind::Path {
                    return Err(EditError::NoRun);
                }
                let n = crate::insert::num;
                // Widths are in the page's points; the shape's own units
                // may be scaled.
                let scale = scale_of(&object.ctm) as f32;
                let mut before = String::new();
                if let Some(w) = width {
                    before.push_str(&format!("{} w ", n(w.max(0.0) / scale)));
                }
                if let Some(d) = dash {
                    let parts: Vec<String> = d.iter().map(|v| n(v.max(0.0) / scale)).collect();
                    before.push_str(&format!("[{}] 0 d ", parts.join(" ")));
                }
                if let Some(c) = cap {
                    before.push_str(&format!("{} J ", (*c).min(2)));
                }
                if let Some(j) = join {
                    before.push_str(&format!("{} j ", (*j).min(2)));
                }
                let mut edits = wrap_edits(
                    &|op| {
                        !(width.is_some() && op.is("w")
                            || dash.is_some() && op.is("d")
                            || cap.is_some() && op.is("J")
                            || join.is_some() && op.is("j"))
                    },
                    before,
                );
                if fill.is_some() || stroke.is_some() {
                    let paint_op = &read.ops[*object.ops.end()];
                    let o = paint_op.operator.as_slice();
                    let style = object.style.as_ref();
                    let filled = fill.unwrap_or(style.is_some_and(|s| s.fill.is_some()));
                    let stroked = stroke.unwrap_or(style.is_some_and(|s| s.stroke.is_some()));
                    let star = if o.ends_with(b"*") { "*" } else { "" };
                    let closes = matches!(o, b"s" | b"b" | b"b*");
                    let new = match (filled, stroked) {
                        (true, true) => format!("{}{star}", if closes { "b" } else { "B" }),
                        (true, false) => format!("f{star}"),
                        (false, true) => (if closes { "s" } else { "S" }).to_string(),
                        (false, false) => "n".to_string(),
                    };
                    edits.push((paint_op.span.clone(), new.into_bytes()));
                }
                splice(&read.content, edits)
            }
            ObjectChange::MovePoint { index, to } => {
                let &(i, k) = object.point_ops.get(*index).ok_or(EditError::NoRun)?;
                let inv = invert(&object.ctm).ok_or(EditError::NoRun)?;
                let (x, y) = apply(&inv, f64::from(to[0]), f64::from(to[1]));
                let op = &read.ops[i];
                let n = |k: usize| op.number(k).unwrap_or(0.0);
                let mut nums: Vec<f64> = (0..op.operands.len()).map(n).collect();
                match k {
                    k if k == usize::MAX - 1 && nums.len() == 4 => {
                        // The corner moves; the opposite one stays.
                        let (fx, fy) = (nums[0] + nums[2], nums[1] + nums[3]);
                        nums = vec![x, y, fx - x, fy - y];
                    }
                    k if k == usize::MAX && nums.len() == 4 => {
                        nums[2] = x - nums[0];
                        nums[3] = y - nums[1];
                    }
                    k if k + 1 < nums.len() => {
                        // A curve's end takes its last control point along,
                        // so the curve keeps its shape there.
                        if op.is("c") && k == 4 {
                            nums[2] += x - nums[4];
                            nums[3] += y - nums[5];
                        }
                        nums[k] = x;
                        nums[k + 1] = y;
                    }
                    _ => return Err(EditError::NoRun),
                }
                let mut text = nums
                    .iter()
                    .map(|v| number(*v))
                    .collect::<Vec<_>>()
                    .join(" ");
                text.push(' ');
                text.push_str(&String::from_utf8_lossy(&op.operator));
                splice(&read.content, vec![(op.span.clone(), text.into_bytes())])
            }
            ObjectChange::Unclip => {
                let Some(level) = clip_level(&read.ops, start) else {
                    return Ok(self.update()?.finish());
                };
                // Past the `Q` that ends the outermost clip in force; a
                // clip set at the top level never ends.
                let mut there = read.content.len();
                if level > 0 {
                    let mut depth = read.ops[..start].iter().fold(0usize, |d, op| {
                        if op.is("q") {
                            d + 1
                        } else if op.is("Q") {
                            d.saturating_sub(1)
                        } else {
                            d
                        }
                    });
                    for op in &read.ops[*object.ops.end() + 1..] {
                        if op.is("q") {
                            depth += 1;
                        } else if op.is("Q") {
                            depth = depth.saturating_sub(1);
                            if depth < level {
                                there = op.span.end;
                                break;
                            }
                        }
                    }
                }
                let at = read
                    .ops
                    .iter()
                    .position(|op| op.span.start >= there)
                    .unwrap_or(read.ops.len());
                let undo = invert(&ctm_at(&read.ops, at)).unwrap_or(IDENTITY);
                let mut moved = b"\nq ".to_vec();
                for v in undo {
                    moved.extend_from_slice(number(v).as_bytes());
                    moved.push(b' ');
                }
                moved.extend_from_slice(b"cm ");
                for &i in &state_at_with(&read.ops, start, false) {
                    moved.extend_from_slice(&read.content[read.ops[i].span.clone()]);
                    moved.push(b' ');
                }
                moved.extend_from_slice(body);
                moved.extend_from_slice(b" Q\n");
                let mut kept = Vec::new();
                for op in &read.ops[object.ops.clone()] {
                    if !is_path_or_paint(op) && !op.is("Do") && !op.is("BI") {
                        kept.extend_from_slice(&read.content[op.span.clone()]);
                        kept.push(b' ');
                    }
                }
                splice(
                    &read.content,
                    vec![(first..last, kept), (there..there, moved)],
                )
            }
            ObjectChange::Colours { fill, stroke } => {
                let mut before = String::new();
                let n = crate::insert::num;
                if let Some([r, g, b]) = fill {
                    before.push_str(&format!("{} {} {} rg ", n(*r), n(*g), n(*b)));
                }
                if let Some([r, g, b]) = stroke {
                    before.push_str(&format!("{} {} {} RG ", n(*r), n(*g), n(*b)));
                }
                let (f, st) = (fill.is_some(), stroke.is_some());
                wrap(&|op| !(f && op.is("rg") || st && op.is("RG")), before)
            }
            ObjectChange::Picture(image) => {
                if object.kind != ObjectKind::Picture {
                    return Err(EditError::NoRun);
                }
                let r = crate::insert::image_object(&mut update, image);
                let key = add.xobject(r);
                splice(
                    &read.content,
                    vec![(first..last, format!("/{key} Do").into_bytes())],
                )
            }
        };
        add.finish_replacing(&mut update, &content);
        Ok(update.finish())
    }

    /// The file with object `index` of `page_objects(page)` taken out.
    pub fn delete_object(&self, page: usize, index: usize) -> Result<Vec<u8>, EditError> {
        self.delete_objects(page, &[index])
    }

    /// The file with the objects of `page_objects(page)` at `indices`
    /// taken out, in one change.
    pub fn delete_objects(&self, page: usize, indices: &[usize]) -> Result<Vec<u8>, EditError> {
        let objects = self.page_objects(page);
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        let mut edits = Vec::new();
        for index in indices {
            let object = objects.get(index).ok_or(EditError::NoRun)?;
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
            edits.push((first..last, kept));
        }
        let content = splice(&read.content, edits);
        self.write_page_content(page, &content)
    }

    /// Picture `index` of `page_objects(page)` as a file: a JPEG's bytes
    /// as they are, or its samples. None for a drawing, a picture in the
    /// page's own content, or one in a form this cannot read (JPEG 2000,
    /// JBIG2, fax).
    pub fn picture_file(&self, page: usize, index: usize) -> Option<PictureFile> {
        let object = self.page_objects(page).into_iter().nth(index)?;
        if object.kind != ObjectKind::Picture {
            return None;
        }
        let read = self.page_content(page)?;
        let op = &read.ops[*object.ops.start()];
        if !op.is("Do") {
            return None;
        }
        let name = op.operands.first()?.as_name()?;
        let page_ref = *self.pages().get(page)?;
        let dict = page_dict(self, page_ref);
        let resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let Object::Dict(xobjects) = self.resolve_key(&resources, "XObject") else {
            return None;
        };
        let Object::Stream(image) = self.resolve(xobjects.get(&String::from_utf8_lossy(name))?)
        else {
            return None;
        };
        let d = &image.dict;
        let width = u32::try_from(self.resolve_key(d, "Width").as_int()?).ok()?;
        let height = u32::try_from(self.resolve_key(d, "Height").as_int()?).ok()?;
        let filters: Vec<Vec<u8>> = match self.resolve_key(d, "Filter") {
            Object::Name(n) => vec![n.as_bytes().to_vec()],
            Object::Array(a) => a
                .iter()
                .filter_map(|f| self.resolve(f).as_name().map(<[u8]>::to_vec))
                .collect(),
            _ => Vec::new(),
        };
        match filters.last().map(Vec::as_slice) {
            Some(b"DCTDecode" | b"DCT") => {
                let last = filters.len() - 1;
                if last == 0 {
                    return Some(PictureFile::Jpeg(image.data.clone()));
                }
                let mut before = Dict::new();
                before.set(
                    "Filter",
                    Object::Array(
                        filters[..last]
                            .iter()
                            .map(|f| Object::name(&String::from_utf8_lossy(f)))
                            .collect(),
                    ),
                );
                if let Object::Array(params) = self.resolve_key(d, "DecodeParms")
                    && params.len() == filters.len()
                {
                    before.set("DecodeParms", Object::Array(params[..last].to_vec()));
                }
                Some(PictureFile::Jpeg(decode(&before, &image.data).ok()?))
            }
            Some(b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"JPX" | b"CCF") => None,
            _ => {
                let mask = matches!(self.resolve_key(d, "ImageMask"), Object::Bool(true));
                let (components, bits) = if mask {
                    (1, 1)
                } else {
                    let cs = self.resolve_key(d, "ColorSpace");
                    let components =
                        u8::try_from(crate::redact::components(self, &cs, Some(&resources))?)
                            .ok()?;
                    let bits =
                        u8::try_from(self.resolve_key(d, "BitsPerComponent").as_int()?).ok()?;
                    (components, bits)
                };
                if !matches!(bits, 1 | 2 | 4 | 8 | 16) || !matches!(components, 1 | 3 | 4) {
                    return None;
                }
                let data = decode(d, &image.data).ok()?;
                let stride =
                    (usize::from(components) * usize::from(bits) * width as usize).div_ceil(8);
                if data.len() < stride * height as usize {
                    return None;
                }
                Some(PictureFile::Samples {
                    width,
                    height,
                    components,
                    bits,
                    data,
                })
            }
        }
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
    pub(crate) fn write_page_content(
        &self,
        page: usize,
        content: &[u8],
    ) -> Result<Vec<u8>, EditError> {
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

/// What can be changed about an object besides where it is.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectChange {
    /// Drawn last, over everything else the page draws.
    Front,
    /// Drawn first, under everything else.
    Back,
    /// Drawn one step later: over the object drawn right after it.
    Forward,
    /// Drawn one step earlier: under the object drawn right before it.
    Backward,
    /// Drawn see-through: 0 is not at all, 1 fully.
    Opacity(f32),
    /// A shape's fill and line colours (RGB, 0 to 1), each if given.
    Colours {
        fill: Option<[f32; 3]>,
        stroke: Option<[f32; 3]>,
    },
    /// A picture shown in place of this one, in the same box.
    Picture(crate::insert::Image),
    /// A shape painted another way, each part where given: filled or
    /// not, its line drawn or not, so wide (in points), dashed so (none
    /// for solid), its ends and corners as [`ShapeStyle`] says.
    Style {
        fill: Option<bool>,
        stroke: Option<bool>,
        width: Option<f32>,
        dash: Option<Vec<f32>>,
        cap: Option<u8>,
        join: Option<u8>,
    },
    /// One of a shape's [`PageObject::points`] moved to a place in the
    /// page's own space.
    MovePoint { index: usize, to: [f32; 2] },
    /// Drawn whole, out from under the clipping path that cuts it down:
    /// it is drawn again where that clip ends, so may come over what
    /// was drawn there since.
    Unclip,
}

/// The bytes of a picture as they can be saved to a file.
#[derive(Debug, Clone, PartialEq)]
pub enum PictureFile {
    /// A JPEG file's bytes as they are.
    Jpeg(Vec<u8>),
    /// Rows of samples, `components` per pixel (1 gray, 3 RGB, 4 CMYK)
    /// of `bits` bits each, rows padded to whole bytes.
    Samples {
        width: u32,
        height: u32,
        components: u8,
        bits: u8,
        data: Vec<u8>,
    },
}

/// The operators that set the graphics state in force at operator `at`:
/// the matrix, colours, line style, graphics states and clipping, as
/// `q` and `Q` leave them.
fn state_at(ops: &[Op], at: usize) -> Vec<usize> {
    state_at_with(ops, at, true)
}

/// The `q` level the outermost clipping path in force at operator `at`
/// was set at (0 for the top level), if any.
fn clip_level(ops: &[Op], at: usize) -> Option<usize> {
    let mut base = false;
    let mut stack: Vec<bool> = Vec::new();
    let mut clipping = false;
    for op in &ops[..at.min(ops.len())] {
        match op.operator.as_slice() {
            b"q" => stack.push(false),
            b"Q" => {
                stack.pop();
            }
            b"W" | b"W*" => clipping = true,
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                if clipping {
                    match stack.last_mut() {
                        Some(level) => *level = true,
                        None => base = true,
                    }
                }
                clipping = false;
            }
            _ => {}
        }
    }
    if base {
        return Some(0);
    }
    stack.iter().position(|set| *set).map(|i| i + 1)
}

/// As [`state_at`], the clipping paths left out when `clips` is false.
fn state_at_with(ops: &[Op], at: usize, clips: bool) -> Vec<usize> {
    let mut stack: Vec<Vec<usize>> = Vec::new();
    let mut now: Vec<usize> = Vec::new();
    let mut path: Vec<usize> = Vec::new();
    let mut clipping = false;
    for (i, op) in ops[..at].iter().enumerate() {
        match op.operator.as_slice() {
            b"q" => stack.push(now.clone()),
            b"Q" => now = stack.pop().unwrap_or_default(),
            b"cm" | b"w" | b"J" | b"j" | b"M" | b"d" | b"ri" | b"i" | b"gs" | b"CS" | b"cs"
            | b"SC" | b"SCN" | b"sc" | b"scn" | b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" => {
                now.push(i)
            }
            b"W" | b"W*" => {
                clipping = true;
                path.push(i);
            }
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => path.push(i),
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                if clipping && clips {
                    now.append(&mut path);
                    now.push(i);
                }
                path.clear();
                clipping = false;
            }
            _ => {}
        }
    }
    now
}

/// A picture or shape copied off a page, with everything it needs to be
/// drawn on another: its operators, drawn from the page's own space, and
/// the resources they name, with those objects.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectCopy {
    pub kind: ObjectKind,
    /// Where it was, in the page's own space: left, bottom, right, top.
    pub rect: [f32; 4],
    content: Vec<u8>,
    resources: Vec<CopiedResource>,
    /// The objects the resources reach: where each was in its file, as it
    /// was there, and as it is here (its references numbered into this
    /// list from 1).
    objects: Vec<(Option<Ref>, Object, Object)>,
}

#[derive(Debug, Clone, PartialEq)]
struct CopiedResource {
    /// `XObject`, `ExtGState`, `ColorSpace`, `Pattern` or `Shading`.
    category: String,
    name: String,
    /// What the page's resources had under the name, references
    /// numbered into [`ObjectCopy::objects`].
    value: Object,
}

/// Objects taken out of a file into an [`ObjectCopy`].
#[derive(Default)]
struct Taken {
    objects: Vec<(Option<Ref>, Object, Object)>,
    map: std::collections::HashMap<Ref, u32>,
}

impl Taken {
    /// `obj` with the objects it references taken too, numbered into the
    /// list.
    fn take(&mut self, doc: &Document, obj: &Object) -> Object {
        match obj {
            Object::Ref(r) => {
                if let Some(num) = self.map.get(r) {
                    return Object::Ref(Ref::new(*num, 0));
                }
                let num = self.objects.len() as u32 + 1;
                self.map.insert(*r, num);
                let original = doc.get(*r).unwrap_or(Object::Null);
                self.objects
                    .push((Some(*r), original.clone(), Object::Null));
                let local = self.take(doc, &original);
                self.objects[num as usize - 1].2 = local;
                Object::Ref(Ref::new(num, 0))
            }
            Object::Array(items) => {
                Object::Array(items.iter().map(|i| self.take(doc, i)).collect())
            }
            Object::Dict(dict) => Object::Dict(self.take_dict(doc, dict)),
            Object::Stream(s) => {
                let mut s = s.clone();
                s.dict = self.take_dict(doc, &s.dict);
                Object::Stream(s)
            }
            other => other.clone(),
        }
    }

    fn take_dict(&mut self, doc: &Document, dict: &Dict) -> Dict {
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            // A stream's length is written from its data; a page tree is
            // not something a resource should reach, but a form's
            // resources can point back at a page's, so keep clear of it.
            if key.as_bytes() == b"Length" && matches!(value, Object::Ref(_)) {
                continue;
            }
            if key.as_bytes() == b"Parent" || key.as_bytes() == b"P" {
                continue;
            }
            out.0.push((key.clone(), self.take(doc, value)));
        }
        out
    }
}

/// `obj` with references into an [`ObjectCopy`]'s list pointing at
/// `refs` instead.
fn renumber(obj: &Object, refs: &[Ref]) -> Object {
    match obj {
        Object::Ref(r) => refs
            .get(r.num as usize - 1)
            .map_or(Object::Null, |r| Object::Ref(*r)),
        Object::Array(items) => Object::Array(items.iter().map(|i| renumber(i, refs)).collect()),
        Object::Dict(dict) => Object::Dict(renumber_dict(dict, refs)),
        Object::Stream(s) => {
            let mut s = s.clone();
            s.dict = renumber_dict(&s.dict, refs);
            Object::Stream(s)
        }
        other => other.clone(),
    }
}

fn renumber_dict(dict: &Dict, refs: &[Ref]) -> Dict {
    Dict(
        dict.iter()
            .map(|(k, v)| (k.clone(), renumber(v, refs)))
            .collect(),
    )
}

/// The resource category an operator names, and which operand names it:
/// `/Im1 Do`, `/GS0 gs`, `/CS0 cs`, `/P1 scn`, `/Sh0 sh`.
fn resource_operand(op: &Op) -> Option<(&'static str, usize)> {
    let last = op.operands.len().checked_sub(1)?;
    match op.operator.as_slice() {
        b"Do" => Some(("XObject", 0)),
        b"gs" => Some(("ExtGState", 0)),
        b"sh" => Some(("Shading", 0)),
        b"cs" | b"CS" => {
            let name = op.operands.first()?.as_name()?;
            let device = matches!(
                name,
                b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"Pattern"
            );
            (!device).then_some(("ColorSpace", 0))
        }
        b"scn" | b"SCN" => op.operands[last].as_name().map(|_| ("Pattern", last)),
        _ => None,
    }
}

/// Whether `op` sets up what is drawn next rather than drawing: the
/// state, a `q`, or a clipping path.
fn sets_up(op: &Op) -> bool {
    matches!(
        op.operator.as_slice(),
        b"q" | b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"W"
            | b"W*"
            | b"n"
            | b"m"
            | b"l"
            | b"c"
            | b"v"
            | b"y"
            | b"h"
            | b"re"
    )
}

/// The matrix in force as operator `at` starts.
fn ctm_at(ops: &[Op], at: usize) -> Matrix {
    let mut ctm = IDENTITY;
    let mut stack = Vec::new();
    for op in &ops[..at.min(ops.len())] {
        let n = |k: usize| op.number(k).unwrap_or(0.0);
        match op.operator.as_slice() {
            b"q" => stack.push(ctm),
            b"Q" => ctm = stack.pop().unwrap_or(ctm),
            b"cm" if op.operands.len() == 6 => {
                ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &ctm);
            }
            _ => {}
        }
    }
    ctm
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
