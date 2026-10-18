// SPDX-License-Identifier: GPL-3.0-or-later

//! Editing the text a page draws (docs/PLAN.md §4.3, "edit text in
//! place"). A page's text-showing operators are read into runs, a line's
//! worth of text in one font each, with where they sit. An edit rewrites
//! only the part of a run that changed: the glyphs before and after it
//! keep their codes and kerning, the new letters are written in the run's
//! own font where it holds the glyphs and in the nearest standard font
//! where it does not, and text placed further along the line moves by as
//! much as the change grew or shrank. Everything else in the page's
//! content stays byte for byte, and the change is an incremental update.

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::UpdateError;
use crate::content::{Op, ops, splice, write_op};
use crate::document::Document;
use crate::filter::decode;
use crate::fonts::{Font, stand_in, win_ansi_code, win_ansi_width};
use crate::object::{Dict, Name, Object, Ref};
use crate::pages::page_dict;

/// The start of the names given to standard fonts brought in to stand in
/// for letters a font lacks.
const STAND_IN: &[u8] = b"KatnaF";
/// A gap wider than this share of the text size is a space.
const SPACE: f64 = 0.15;
/// How far above and below its baseline a line of text reaches, as a
/// share of its size.
const ASCENT: f64 = 0.8;
const DESCENT: f64 = 0.2;
/// The space left for a space a font has no glyph for, in thousandths of
/// the text size, when the text had none to measure.
const SPACE_WIDTH: f64 = 250.0;

/// A line's worth of text in one font, as the page draws it.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    /// The font's name, without a subset's tag.
    pub font: String,
    /// The text's size on the page, in points.
    pub size: f32,
    /// Where it is, in the page's own space (PDF's, from the bottom
    /// left): left, bottom, right, top.
    pub rect: [f32; 4],
    /// Its pieces, and where each one's text starts in `text`, in chars.
    ops: Vec<usize>,
    starts: Vec<usize>,
    /// `font` is a standard font standing in, not the run's own.
    stood_in: bool,
    /// The spaces in `text` that are gaps on the page rather than
    /// glyphs, and the width of the narrowest, in thousandths of the size.
    space: f64,
}

#[derive(Debug)]
pub enum EditError {
    NoPage,
    NoRun,
    /// The content could not be read (a filter not supported).
    Content,
    /// A character neither the font nor a standard font has.
    Unsupported(char),
    Update(UpdateError),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::NoPage => write!(f, "no such page"),
            EditError::NoRun => write!(f, "no such text on the page"),
            EditError::Content => write!(f, "the page's content could not be read"),
            EditError::Unsupported(c) => {
                write!(
                    f,
                    "the font has no “{c}”, and no standard font has it either"
                )
            }
            EditError::Update(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<UpdateError> for EditError {
    fn from(e: UpdateError) -> Self {
        EditError::Update(e)
    }
}

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b`.
fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
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

fn translate(x: f64, y: f64) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// One character code or move of a text-showing operator.
struct Atom {
    kind: AtomKind,
    /// How many chars of the run's text it stands for.
    chars: usize,
    /// How far it moves the pen, in its piece's text space.
    advance: f64,
}

enum AtomKind {
    /// A code's bytes.
    Code(Vec<u8>),
    /// A `TJ` number.
    Move(Object),
}

/// One text-showing operator, read.
struct Piece {
    atoms: Vec<Atom>,
    op: usize,
    font: Vec<u8>,
    /// The font size operand in force (`Tf`).
    size: f64,
    /// Text space to the page's at the operator's start, the move to
    /// the next line of `'` and `"` made.
    m: Matrix,
    /// How far it moves along its line, in text space.
    advance: f64,
    /// Where along it its glyphs start and end (moves before and after
    /// them left out).
    ink: (f64, f64),
    rise: f64,
    text: String,
    /// The narrowest gap shown as a space, in thousandths of the size.
    gap: Option<f64>,
    /// Placed by an operator of its own (`Td`, `Tm` and the like) rather
    /// than going on where the text before it ended.
    placed: bool,
    char_spacing: f64,
    word_spacing: f64,
    /// Horizontal scaling (`Tz`), as a factor.
    stretch: f64,
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    font: Option<Vec<u8>>,
    size: f64,
    rise: f64,
    render: i64,
}

/// A page's content, its fonts and its text, read.
struct Page {
    content: Vec<u8>,
    ops: Vec<Op>,
    fonts: HashMap<Vec<u8>, Font>,
    pieces: Vec<Piece>,
    /// Each font's codes the page shows.
    used: HashMap<Vec<u8>, HashSet<u32>>,
}

impl Document {
    /// The text page `page` draws in its own content, in runs: the text
    /// of forms it places (`Do`) and invisible text are left out.
    pub fn text_runs(&self, page: usize) -> Vec<TextRun> {
        self.read_page(page).map(|p| runs(&p)).unwrap_or_default()
    }

    /// The file with the text of runs on page `page` changed: each
    /// `(index into text_runs, new text)`, as an incremental update.
    pub fn edit_text(&self, page: usize, edits: &[(usize, String)]) -> Result<Vec<u8>, EditError> {
        let page_ref = *self.pages().get(page).ok_or(EditError::NoPage)?;
        let read = self.read_page(page).ok_or(EditError::Content)?;
        let all = runs(&read);
        let mut update = self.update()?;
        // Standard fonts brought in for glyphs a font lacks, by name.
        let mut stand_ins: Vec<(&'static str, Vec<u8>)> = Vec::new();
        let full = page_dict(self, page_ref);
        let resources = match full.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let mut font_dict = match self.resolve_key(&resources, "Font") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        // New bytes for operators, and how far to move text that follows
        // an edited run along its line, by piece, in the piece's units.
        let mut replaced: HashMap<usize, Vec<u8>> = HashMap::new();
        let mut shifts: Vec<(usize, f64)> = Vec::new();
        for (index, text) in edits {
            let run = all.get(*index).ok_or(EditError::NoRun)?;
            let old: Vec<char> = run.text.chars().collect();
            let new: Vec<char> = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
            if old == new {
                continue;
            }
            // The part that changed: what lies between the text the old
            // and new have in common at either end, widened to whole codes
            // (a ligature's glyph stands for several letters).
            let mut from = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            let most = old.len().min(new.len()) - from;
            let common_end = old
                .iter()
                .rev()
                .zip(new.iter().rev())
                .take(most)
                .take_while(|(a, b)| a == b)
                .count();
            let mut to = old.len() - common_end;
            // Each atom of the run: (piece in the run, atom, its chars).
            let mut flat: Vec<(usize, usize, usize, usize)> = Vec::new();
            for (k, i) in run.ops.iter().enumerate() {
                let mut at = run.starts[k];
                for (a, atom) in read.pieces[*i].atoms.iter().enumerate() {
                    flat.push((k, a, at, at + atom.chars));
                    at += atom.chars;
                }
            }
            for &(_, _, s, e) in &flat {
                if s < from && from < e {
                    from = s;
                }
                if s < to && to < e {
                    to = e;
                }
            }
            let middle: String = new[from..new.len() - (old.len() - to)].iter().collect();
            let i1 = flat
                .iter()
                .position(|&(_, _, s, e)| s > from || (s == from && e > s))
                .unwrap_or(flat.len());
            let i2 = i1
                + flat[i1..]
                    .iter()
                    .position(|&(_, _, s, _)| s >= to)
                    .unwrap_or(flat.len() - i1);
            // The pieces it starts and ends in; new text put in where two
            // pieces meet goes at the end of the first.
            let (f, l) = if i1 < i2 {
                (flat[i1].0, flat[i2 - 1].0)
            } else if i1 > 0 && (i1 == flat.len() || flat[i1].0 != flat[i1 - 1].0) {
                (flat[i1 - 1].0, flat[i1 - 1].0)
            } else {
                let k = flat.get(i1).map_or(0, |a| a.0);
                (k, k)
            };
            let fp = &read.pieces[run.ops[f]];
            let lp = &read.pieces[run.ops[l]];
            let Some(inv) = invert(&fp.m) else {
                return Err(EditError::NoRun);
            };
            let along = |piece: &Piece, x: f64| {
                let (px, py) = apply(&piece.m, x, 0.0);
                apply(&inv, px, py).0
            };
            // The fonts to write the new letters in: the piece's own, then
            // the run's others, then the page's of the same name, as one
            // font's glyphs often go on in another (a subset split in two).
            let mut keys: Vec<&Vec<u8>> = vec![&fp.font];
            for i in &run.ops {
                let key = &read.pieces[*i].font;
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
            let base = &read.fonts[&fp.font].base;
            let mut more: Vec<&Vec<u8>> = read
                .fonts
                .iter()
                .filter(|(k, f)| f.base == *base && !keys.contains(k))
                .map(|(k, _)| k)
                .collect();
            more.sort();
            keys.extend(more);
            let encoders: Vec<HashMap<char, u32>> = keys
                .iter()
                .map(|k| {
                    let used = read.used.get(*k).cloned().unwrap_or_default();
                    read.fonts[*k].encoder(&used)
                })
                .collect();
            let (name, metrics) = stand_in(base);
            let size = fp.size;
            let advance = |w: f64, space: bool| {
                (w * size + fp.char_spacing + if space { fp.word_spacing } else { 0.0 })
                    * fp.stretch
            };
            let mut w = Writer {
                out: lead_in(&read.ops[fp.op]),
                items: Vec::new(),
                current: fp.font.clone(),
                size,
            };
            // What comes before the change in its first piece, as it was.
            let mut x = 0.0;
            for &(k, a, _, _) in &flat[..i1] {
                if k == f {
                    let atom = &read.pieces[run.ops[k]].atoms[a];
                    w.atom(atom, &read.fonts[&fp.font]);
                    x += atom.advance;
                }
            }
            // The new text.
            for segment in segments(&middle, &encoders, run.space)? {
                match segment {
                    Segment::Own(k, parts) => {
                        w.font(keys[k]);
                        let font = &read.fonts[keys[k]];
                        for part in parts {
                            match part {
                                Part::Codes(codes) => {
                                    let mut bytes = Vec::new();
                                    for code in codes {
                                        font.code_bytes(code, &mut bytes);
                                        x += advance(
                                            font.width(code),
                                            font.single_byte() && code == 32,
                                        );
                                    }
                                    w.string(bytes, !font.single_byte());
                                }
                                Part::Gap(gap) => {
                                    x += gap / 1000.0 * size * fp.stretch;
                                    w.items.push(number(-gap));
                                }
                            }
                        }
                    }
                    Segment::Standard(bytes) => {
                        let key = match stand_ins.iter().find(|(n, _)| *n == name) {
                            Some((_, key)) => key.clone(),
                            None => {
                                let key = free_name(&font_dict, &stand_ins);
                                stand_ins.push((name, key.clone()));
                                key
                            }
                        };
                        w.font(&key);
                        for b in &bytes {
                            x += advance(win_ansi_width(metrics, *b) / 1000.0, *b == 32);
                        }
                        w.string(bytes, false);
                    }
                }
            }
            let new_end = x;
            // Where the change ended before, along the line.
            let mut before = 0.0;
            for &(k, a, _, _) in &flat[..i2] {
                if k == l {
                    before += read.pieces[run.ops[k]].atoms[a].advance;
                }
            }
            let old_end = along(lp, before);
            let delta = new_end - old_end;
            // What comes after the change in its last piece, as it was.
            w.font(&lp.font);
            for &(k, a, _, _) in &flat[i2..] {
                if k == l {
                    w.atom(&read.pieces[run.ops[k]].atoms[a], &read.fonts[&lp.font]);
                }
            }
            w.font(&fp.font);
            replaced.insert(fp.op, w.finish());
            // The pieces after the first that the change reached are
            // emptied. Where one placed the pen afresh, it is left where
            // that piece began, so the last one moves it on to where the
            // text it ended with now ends, for what goes on from there.
            let target = along(lp, lp.advance) + delta;
            let mut pen = target;
            for k in f + 1..=l {
                let piece = &read.pieces[run.ops[k]];
                let op = &read.ops[piece.op];
                let mut out = lead_in(op);
                if piece.placed {
                    pen = along(piece, 0.0);
                }
                if k == l && (pen - target).abs() > 1e-3 {
                    let units = (target - pen) * scale_between(fp, piece);
                    let n = -units * 1000.0 / (piece.size * piece.stretch);
                    write_op(
                        &mut out,
                        &Op::new("TJ", vec![Object::Array(vec![number(n)])]),
                    );
                }
                while out.last() == Some(&b' ') {
                    out.pop();
                }
                replaced.insert(piece.op, out);
            }
            // Text placed afresh further along the same line moves with it.
            if delta.abs() > 1e-3 {
                for (i, piece) in read.pieces.iter().enumerate() {
                    if piece.op <= lp.op || !piece.placed {
                        continue;
                    }
                    let (x, y) = apply(&inv, piece.m[4], piece.m[5]);
                    let same_way = (piece.m[0] * fp.m[1] - piece.m[1] * fp.m[0]).abs()
                        < 1e-3 * fp.m[0].hypot(fp.m[1]) * piece.m[0].hypot(piece.m[1]);
                    if same_way && y.abs() < 0.3 * size.abs() && x > old_end - 0.05 * size.abs() {
                        shifts.push((i, delta * scale_between(fp, piece)));
                    }
                }
            }
        }
        for (i, units) in shifts {
            let piece = &read.pieces[i];
            if replaced.contains_key(&piece.op) {
                continue;
            }
            let op = &read.ops[piece.op];
            let mut out = lead_in(op);
            let mut items = vec![number(-units * 1000.0 / (piece.size * piece.stretch))];
            match op.operands.last() {
                Some(Object::Array(a)) if op.is("TJ") => items.extend(a.iter().cloned()),
                Some(s) => items.push(s.clone()),
                None => continue,
            }
            write_op(&mut out, &Op::new("TJ", vec![Object::Array(items)]));
            replaced.insert(piece.op, out);
        }
        let splices = replaced
            .into_iter()
            .map(|(op, bytes)| (read.ops[op].span.clone(), bytes))
            .collect();
        let content = splice(&read.content, splices);
        let stream = update.add(Object::Stream(crate::dss::compressed(&content)));
        let mut dict = self
            .get(page_ref)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        dict.set("Contents", Object::Ref(stream));
        if !stand_ins.is_empty() {
            for (name, key) in &stand_ins {
                let font = Dict(vec![
                    (Name::new("Type"), Object::name("Font")),
                    (Name::new("Subtype"), Object::name("Type1")),
                    (Name::new("BaseFont"), Object::name(name)),
                    (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
                ]);
                let r = update.add(Object::Dict(font));
                font_dict.0.push((Name(key.clone()), Object::Ref(r)));
            }
            let mut resources = resources;
            resources.set("Font", font_dict);
            dict.set("Resources", resources);
        }
        update.replace(page_ref, Object::Dict(dict));
        Ok(update.finish())
    }

    fn read_page(&self, page: usize) -> Option<Page> {
        let page_ref: Ref = *self.pages().get(page)?;
        let dict = page_dict(self, page_ref);
        let mut content = Vec::new();
        let streams = match dict.get("Contents").map(|c| self.resolve(c)) {
            Some(Object::Array(a)) => a.iter().map(|s| self.resolve(s)).collect(),
            Some(s @ Object::Stream(_)) => vec![s],
            _ => Vec::new(),
        };
        for s in streams {
            let Object::Stream(s) = s else {
                continue;
            };
            content.extend(decode(&s.dict, &s.data).ok()?);
            content.push(b'\n');
        }
        let resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let font_dict = match self.resolve_key(&resources, "Font") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        let mut fonts = HashMap::new();
        for (name, font) in font_dict.iter() {
            if let Object::Dict(d) = self.resolve(font) {
                fonts.insert(name.as_bytes().to_vec(), Font::load(self, &d));
            }
        }
        let ops = ops(&content);
        let mut page = Page {
            content: Vec::new(),
            ops,
            fonts,
            pieces: Vec::new(),
            used: HashMap::new(),
        };
        page.read_text();
        page.content = content;
        Some(page)
    }
}

impl Page {
    fn read_text(&mut self) {
        let mut state = State {
            ctm: IDENTITY,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            font: None,
            size: 0.0,
            rise: 0.0,
            render: 0,
        };
        let mut stack = Vec::new();
        let mut tm = IDENTITY;
        let mut tlm = IDENTITY;
        let mut placed = true;
        let all = std::mem::take(&mut self.ops);
        for (i, op) in all.iter().enumerate() {
            let n = |k: usize| op.number(k).unwrap_or(0.0);
            match op.operator.as_slice() {
                b"q" => stack.push(state.clone()),
                b"Q" => {
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                }
                b"cm" if op.operands.len() == 6 => {
                    state.ctm = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &state.ctm);
                }
                b"BT" => {
                    tm = IDENTITY;
                    tlm = IDENTITY;
                    placed = true;
                }
                b"Tc" => state.char_spacing = n(0),
                b"Tw" => state.word_spacing = n(0),
                b"Tz" => state.scale = n(0) / 100.0,
                b"TL" => state.leading = n(0),
                b"Ts" => state.rise = n(0),
                b"Tr" => state.render = op.operands.first().and_then(Object::as_int).unwrap_or(0),
                b"Tf" => {
                    state.font = op
                        .operands
                        .first()
                        .and_then(Object::as_name)
                        .map(<[u8]>::to_vec);
                    state.size = n(1);
                }
                b"Td" => {
                    tlm = mul(&translate(n(0), n(1)), &tlm);
                    tm = tlm;
                    placed = true;
                }
                b"TD" => {
                    state.leading = -n(1);
                    tlm = mul(&translate(n(0), n(1)), &tlm);
                    tm = tlm;
                    placed = true;
                }
                b"Tm" if op.operands.len() == 6 => {
                    tlm = [n(0), n(1), n(2), n(3), n(4), n(5)];
                    tm = tlm;
                    placed = true;
                }
                b"T*" => {
                    tlm = mul(&translate(0.0, -state.leading), &tlm);
                    tm = tlm;
                    placed = true;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if op.is("\"") {
                        state.word_spacing = n(0);
                        state.char_spacing = n(1);
                    }
                    if op.is("'") || op.is("\"") {
                        tlm = mul(&translate(0.0, -state.leading), &tlm);
                        tm = tlm;
                        placed = true;
                    }
                    let items: Vec<Object> = match op.operands.last() {
                        Some(Object::Array(a)) if op.is("TJ") => a.clone(),
                        Some(s @ Object::String { .. }) => vec![s.clone()],
                        _ => continue,
                    };
                    let start = tm;
                    let advance = self.show(&state, &items, i, start, placed);
                    placed = false;
                    tm = mul(&translate(advance, 0.0), &tm);
                }
                _ => {}
            }
        }
        self.ops = all;
    }

    /// Reads one text-showing operator's strings and spacings, drawn
    /// from text matrix `tm`; returns how far they move the pen.
    fn show(
        &mut self,
        state: &State,
        items: &[Object],
        op: usize,
        tm: Matrix,
        placed: bool,
    ) -> f64 {
        let Some(key) = state.font.clone() else {
            return 0.0;
        };
        let Some(font) = self.fonts.get(&key) else {
            return 0.0;
        };
        let size = state.size;
        let mut advance = 0.0;
        let mut text = String::new();
        let mut gap: Option<f64> = None;
        let mut atoms = Vec::new();
        let mut ink = (f64::INFINITY, 0.0);
        let used = self.used.entry(key.clone()).or_default();
        for item in items {
            match item {
                Object::String { bytes, .. } => {
                    let mut at = 0;
                    for (code, len) in font.codes(bytes) {
                        used.insert(code);
                        // Ligatures as their letters, so text is found and
                        // edited as it is typed.
                        let unicode: String = font
                            .unicode(code)
                            .unwrap_or_else(|| "\u{FFFD}".into())
                            .chars()
                            .map(|c| unligature(c).map_or_else(|| c.to_string(), str::to_string))
                            .collect();
                        text.push_str(&unicode);
                        let mut w = font.width(code) * size + state.char_spacing;
                        if len == 1 && code == 32 {
                            w += state.word_spacing;
                        }
                        ink.0 = ink.0.min(advance);
                        advance += w * state.scale;
                        ink.1 = advance;
                        atoms.push(Atom {
                            kind: AtomKind::Code(bytes[at..(at + len).min(bytes.len())].to_vec()),
                            chars: unicode.chars().count(),
                            advance: w * state.scale,
                        });
                        at += len;
                    }
                }
                other => {
                    let Some(adjust) = other.as_number() else {
                        continue;
                    };
                    let mut chars = 0;
                    // A move right wider than a space shows one.
                    if -adjust > SPACE * 1000.0 && !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                        chars = 1;
                        gap = Some(gap.map_or(-adjust, |g: f64| g.min(-adjust)));
                    }
                    let moved = -adjust / 1000.0 * size * state.scale;
                    advance += moved;
                    atoms.push(Atom {
                        kind: AtomKind::Move(other.clone()),
                        chars,
                        advance: moved,
                    });
                }
            }
        }
        // Clipping-only and invisible text is no text to edit.
        if !matches!(state.render, 3 | 7) && !text.is_empty() {
            self.pieces.push(Piece {
                ink: if ink.0.is_finite() { ink } else { (0.0, 0.0) },
                atoms,
                op,
                font: key,
                size,
                m: mul(&tm, &state.ctm),
                advance,
                rise: state.rise,
                text,
                gap,
                placed,
                char_spacing: state.char_spacing,
                word_spacing: state.word_spacing,
                stretch: state.scale,
            });
        }
        advance
    }
}

/// The page's pieces of text joined into runs: a piece continues the
/// run before it when it follows it along the same line, in the same
/// font and size.
fn runs(page: &Page) -> Vec<TextRun> {
    let mut out: Vec<TextRun> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, piece) in page.pieces.iter().enumerate() {
        let joined = last.and_then(|l| {
            let prev = &page.pieces[l];
            // Letters an edit wrote in a standard font standing in belong
            // to the text around them.
            let same_font = prev.font == piece.font
                || page.fonts[&prev.font].base == page.fonts[&piece.font].base
                || prev.font.starts_with(STAND_IN)
                || piece.font.starts_with(STAND_IN);
            if !same_font || (prev.size - piece.size).abs() > 1e-6 {
                return None;
            }
            // Same direction, and the start on the previous one's line.
            let same_way = (prev.m[0] - piece.m[0]).abs() < 1e-3 * prev.m[0].abs().max(1.0)
                && (prev.m[1] - piece.m[1]).abs() < 1e-3 * prev.m[0].abs().max(1.0);
            if !same_way {
                return None;
            }
            let inv = invert(&prev.m)?;
            let (px, py) = apply(&piece.m, piece.ink.0, 0.0);
            let (x, y) = apply(&inv, px, py);
            let size = prev.size.abs().max(1e-6);
            let off = x - prev.ink.1;
            (y.abs() < 0.3 * size && off > -0.5 * size && off < 3.0 * size)
                .then_some(off / size * 1000.0)
        });
        match (joined, out.last_mut()) {
            (Some(gap), Some(run)) => {
                if gap > SPACE * 1000.0 && !run.text.ends_with(' ') && !piece.text.starts_with(' ')
                {
                    run.text.push(' ');
                    run.space = run.space.min(gap);
                }
                if run.stood_in && !piece.font.starts_with(STAND_IN) {
                    run.font = page.fonts[&piece.font].base.clone();
                    run.stood_in = false;
                }
                run.starts.push(run.text.chars().count());
                run.text.push_str(&piece.text);
                if let Some(g) = piece.gap {
                    run.space = run.space.min(g);
                }
                run.ops.push(i);
                let r = piece_rect(piece);
                run.rect = [
                    run.rect[0].min(r[0]),
                    run.rect[1].min(r[1]),
                    run.rect[2].max(r[2]),
                    run.rect[3].max(r[3]),
                ];
            }
            _ => {
                let font = &page.fonts[&piece.font];
                let size = piece.size * piece.m[2].hypot(piece.m[3]);
                out.push(TextRun {
                    text: piece.text.clone(),
                    font: font.base.clone(),
                    size: size.abs() as f32,
                    rect: piece_rect(piece),
                    ops: vec![i],
                    starts: vec![0],
                    stood_in: piece.font.starts_with(STAND_IN),
                    space: piece.gap.unwrap_or(f64::INFINITY),
                });
            }
        }
        last = Some(i);
    }
    for run in &mut out {
        if !run.space.is_finite() {
            run.space = SPACE_WIDTH;
        }
        let trimmed = run.text.trim_end().len();
        run.text.truncate(trimmed);
    }
    out.retain(|r| !r.text.trim().is_empty());
    out
}

/// Writes a text-showing operator's worth of strings and moves, with
/// font changes between.
struct Writer {
    out: Vec<u8>,
    /// The `TJ` being gathered.
    items: Vec<Object>,
    current: Vec<u8>,
    size: f64,
}

impl Writer {
    fn flush(&mut self) {
        if !self.items.is_empty() {
            let items = std::mem::take(&mut self.items);
            write_op(&mut self.out, &Op::new("TJ", vec![Object::Array(items)]));
            self.out.push(b' ');
        }
    }

    fn font(&mut self, key: &[u8]) {
        if self.current != key {
            self.flush();
            write_op(
                &mut self.out,
                &Op::new(
                    "Tf",
                    vec![Object::Name(Name(key.to_vec())), number(self.size)],
                ),
            );
            self.out.push(b' ');
            self.current = key.to_vec();
        }
    }

    fn string(&mut self, bytes: Vec<u8>, hex: bool) {
        if let Some(Object::String { bytes: last, .. }) = self.items.last_mut() {
            last.extend(bytes);
        } else {
            self.items.push(Object::String { bytes, hex });
        }
    }

    fn atom(&mut self, atom: &Atom, font: &Font) {
        match &atom.kind {
            AtomKind::Code(bytes) => self.string(bytes.clone(), !font.single_byte()),
            AtomKind::Move(n) => self.items.push(n.clone()),
        }
    }

    fn finish(mut self) -> Vec<u8> {
        self.flush();
        while self.out.last() == Some(&b' ') {
            self.out.pop();
        }
        self.out
    }
}

/// Lengths along the line in `from`'s text space to `to`'s.
fn scale_between(from: &Piece, to: &Piece) -> f64 {
    let a = from.m[0].hypot(from.m[1]);
    let b = to.m[0].hypot(to.m[1]);
    if b > 1e-12 { a / b } else { 1.0 }
}

fn piece_rect(piece: &Piece) -> [f32; 4] {
    let size = piece.size.abs();
    let low = piece.rise - DESCENT * size;
    let high = piece.rise + ASCENT * size;
    let (start, end) = piece.ink;
    let corners = [
        apply(&piece.m, start, low),
        apply(&piece.m, end, low),
        apply(&piece.m, end, high),
        apply(&piece.m, start, high),
    ];
    let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (x, y) in corners {
        r[0] = r[0].min(x as f32);
        r[1] = r[1].min(y as f32);
        r[2] = r[2].max(x as f32);
        r[3] = r[3].max(y as f32);
    }
    r
}

/// What a text-showing operator does besides showing: `'` and `"` move
/// to the next line first, and `"` sets the spacing.
fn lead_in(op: &Op) -> Vec<u8> {
    let mut out = Vec::new();
    if op.is("\"") && op.operands.len() == 3 {
        write_op(&mut out, &Op::new("Tw", vec![op.operands[0].clone()]));
        out.push(b' ');
        write_op(&mut out, &Op::new("Tc", vec![op.operands[1].clone()]));
        out.push(b' ');
    }
    if op.is("'") || op.is("\"") {
        out.extend_from_slice(b"T* ");
    }
    out
}

enum Part {
    Codes(Vec<u32>),
    /// A space as a move right, in thousandths of the size.
    Gap(f64),
}

enum Segment {
    /// In the run's `n`th font.
    Own(usize, Vec<Part>),
    /// In the standard font standing in, WinAnsi-encoded.
    Standard(Vec<u8>),
}

/// The letters a ligature stands for, to write it with when no font has
/// the ligature itself.
fn unligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{FB00}' => "ff",
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

/// `text` split by the font each part is written in: the first of the
/// run's fonts (`encoders`) that has the glyph, else a standard font.
/// Spaces no font has a glyph for become gaps.
fn segments(
    text: &str,
    encoders: &[HashMap<char, u32>],
    space: f64,
) -> Result<Vec<Segment>, EditError> {
    let mut out: Vec<Segment> = Vec::new();
    let mut chars: Vec<char> = Vec::new();
    for c in text.chars() {
        let c = if c == '\u{a0}' || c == '\t' { ' ' } else { c };
        if c == '\n' || c == '\r' {
            continue;
        }
        let held = encoders.iter().any(|e| e.contains_key(&c));
        match unligature(c) {
            Some(letters) if !held => chars.extend(letters.chars()),
            _ => chars.push(c),
        }
    }
    // Letters a font has only as a ligature are written with it.
    let mut i = 0;
    let mut joined = Vec::with_capacity(chars.len());
    while i < chars.len() {
        let found = ['\u{FB03}', '\u{FB04}', '\u{FB00}', '\u{FB01}', '\u{FB02}']
            .into_iter()
            .find(|lig| {
                let letters: Vec<char> = unligature(*lig).unwrap_or_default().chars().collect();
                chars[i..].starts_with(&letters)
                    && encoders.iter().any(|e| e.contains_key(lig))
                    && !letters
                        .iter()
                        .all(|c| encoders.iter().any(|e| e.contains_key(c)))
            });
        match found {
            Some(lig) => {
                i += unligature(lig).unwrap_or_default().chars().count();
                joined.push(lig);
            }
            None => {
                joined.push(chars[i]);
                i += 1;
            }
        }
    }
    for c in joined {
        let own = encoders
            .iter()
            .enumerate()
            .find_map(|(k, e)| e.get(&c).map(|code| (k, *code)));
        if own.is_none() && c == ' ' {
            match out.last_mut() {
                Some(Segment::Standard(bytes)) => bytes.push(b' '),
                Some(Segment::Own(_, parts)) => parts.push(Part::Gap(space)),
                None => out.push(Segment::Own(0, vec![Part::Gap(space)])),
            }
            continue;
        }
        if let Some((k, code)) = own {
            match out.last_mut() {
                Some(Segment::Own(last, parts)) if *last == k => match parts.last_mut() {
                    Some(Part::Codes(codes)) => codes.push(code),
                    _ => parts.push(Part::Codes(vec![code])),
                },
                _ => out.push(Segment::Own(k, vec![Part::Codes(vec![code])])),
            }
            continue;
        }
        let (code, _) = win_ansi_code(c).ok_or(EditError::Unsupported(c))?;
        match out.last_mut() {
            Some(Segment::Standard(bytes)) => bytes.push(code),
            _ => out.push(Segment::Standard(vec![code])),
        }
    }
    Ok(out)
}

/// A name for a font brought in that neither the page's fonts nor those
/// brought in already use.
fn free_name(fonts: &Dict, taken: &[(&'static str, Vec<u8>)]) -> Vec<u8> {
    let mut k = 1;
    loop {
        let name = format!("{}{k}", String::from_utf8_lossy(STAND_IN));
        if fonts.get(&name).is_none() && !taken.iter().any(|(_, t)| *t == name.as_bytes()) {
            return name.into_bytes();
        }
        k += 1;
    }
}

fn number(v: f64) -> Object {
    if v.fract() == 0.0 && v.abs() < 1e9 {
        Object::Int(v as i64)
    } else {
        Object::Real(v)
    }
}
