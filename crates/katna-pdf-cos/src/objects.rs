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
        let wrap = |keep: &dyn Fn(&Op) -> bool, new: String| {
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
                splice(&read.content, vec![(from..first, before)])
            } else {
                splice(
                    &read.content,
                    vec![
                        (first..first, format!("q {new}").into_bytes()),
                        (last..last, b" Q".to_vec()),
                    ],
                )
            }
        };
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
}

/// The operators that set the graphics state in force at operator `at`:
/// the matrix, colours, line style, graphics states and clipping, as
/// `q` and `Q` leave them.
fn state_at(ops: &[Op], at: usize) -> Vec<usize> {
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
                if clipping {
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
