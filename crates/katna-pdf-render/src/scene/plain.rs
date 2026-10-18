// SPDX-License-Identifier: GPL-3.0-or-later

//! A quick reader for pages that only draw plain paths, as CAD exports do
//! (docs/PLAN.md §2.5).
//!
//! On such a page most of a scene's reading time goes to hayro's general
//! tokenizer and interpreter. This reads the content stream itself:
//! numbers, the path and painting operators, the graphics state
//! operators that take only numbers, and grey and RGB colours. It feeds
//! the scene's [`Builder`] exactly what hayro's interpreter would, with
//! the same rules for rectangles, zero-sized fills, clips and `q`/`Q`.
//! Anything else (text, images, names, strings, CMYK, inline images, a
//! malformed number) makes it give up, and the page is read by hayro.

use hayro::hayro_interpret::{ClipPath, Device, FillRule};
use hayro::kurbo::{Affine, BezPath, Cap, Join, PathEl, Point, Rect, Shape, Stroke};

use super::Builder;

/// Reads the decoded content stream `content` into `builder`, starting
/// from `initial`, with `bbox` as the page's drawing area. False when the
/// stream holds something this reader leaves to hayro; `builder` is then
/// half-filled and must be thrown away.
pub(super) fn read(content: &[u8], initial: Affine, bbox: Rect, builder: &mut Builder) -> bool {
    let mut reader = Reader {
        builder,
        // hayro's context starts with one state and saves another before
        // the page's own operators.
        states: vec![State::new(initial), State::new(initial)],
        bboxes: vec![bbox],
        path: BezPath::new(),
        last_point: Point::ZERO,
        sub_path_start: Point::ZERO,
        clip: None,
    };
    reader.run(content).is_some()
}

#[derive(Clone)]
struct State {
    ctm: Affine,
    line_width: f32,
    line_cap: Cap,
    line_join: Join,
    miter_limit: f32,
    dash_array: Vec<f32>,
    dash_offset: f32,
    stroke_color: [u8; 4],
    fill_color: [u8; 4],
    /// The clips pushed in this state: true for one the builder saw,
    /// false for one that changed nothing.
    clips: Vec<bool>,
}

impl State {
    fn new(ctm: Affine) -> State {
        State {
            ctm,
            line_width: 1.0,
            line_cap: Cap::Butt,
            line_join: Join::Miter,
            miter_limit: 10.0,
            dash_array: Vec::new(),
            dash_offset: 0.0,
            stroke_color: BLACK,
            fill_color: BLACK,
            clips: Vec::new(),
        }
    }

    fn stroke(&self) -> Stroke {
        Stroke {
            width: f64::from(self.line_width),
            join: self.line_join,
            miter_limit: f64::from(self.miter_limit),
            start_cap: self.line_cap,
            end_cap: self.line_cap,
            dash_pattern: self.dash_array.iter().map(|n| f64::from(*n)).collect(),
            dash_offset: f64::from(self.dash_offset),
        }
    }
}

const BLACK: [u8; 4] = [0, 0, 0, 255];

struct Reader<'b> {
    builder: &'b mut Builder,
    states: Vec<State>,
    /// The drawing area as clips narrow it, as hayro's context keeps it.
    bboxes: Vec<Rect>,
    path: BezPath,
    last_point: Point,
    sub_path_start: Point,
    /// A clip (`W`, `W*`) waiting for the `n` that applies it.
    clip: Option<FillRule>,
}

/// The most operands an operator here takes.
const MAX_OPERANDS: usize = 6;

impl Reader<'_> {
    fn run(&mut self, content: &[u8]) -> Option<()> {
        let mut nums = [0f64; MAX_OPERANDS];
        let mut count = 0;
        let mut i = 0;
        while i < content.len() {
            match content[i] {
                b if is_white(b) => i += 1,
                b'%' => {
                    while i < content.len() && !matches!(content[i], b'\n' | b'\r') {
                        i += 1;
                    }
                }
                // Only `d` takes an array: `[dashes] phase d`.
                b'[' if count == 0 => i = self.dash(content, i + 1)?,
                b'0'..=b'9' | b'+' | b'-' | b'.' => {
                    if count == MAX_OPERANDS {
                        return None;
                    }
                    (nums[count], i) = number(content, i)?;
                    count += 1;
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'*' | b'\'' | b'"' => {
                    let start = i;
                    while i < content.len() && is_regular(content[i]) {
                        i += 1;
                    }
                    self.operator(&content[start..i], &nums[..count])?;
                    count = 0;
                }
                // Names, strings, dictionaries: not plain paths.
                _ => return None,
            }
        }
        (count == 0).then_some(())
    }

    /// Reads a dash pattern from just inside its `[` at `i` to its `d`,
    /// and applies it. Returns where reading goes on.
    fn dash(&mut self, content: &[u8], mut i: usize) -> Option<usize> {
        let skip = |i: &mut usize| {
            while content.get(*i).is_some_and(|&b| is_white(b)) {
                *i += 1;
            }
        };
        let mut dashes = Vec::new();
        loop {
            skip(&mut i);
            if content.get(i) == Some(&b']') {
                i += 1;
                break;
            }
            let (n, end) = number(content, i)?;
            dashes.push(n);
            i = end;
        }
        skip(&mut i);
        let (phase, end) = number(content, i)?;
        i = end;
        skip(&mut i);
        if content.get(i) != Some(&b'd') || content.get(i + 1).is_some_and(|&b| is_regular(b)) {
            return None;
        }
        let state = self.state();
        state.dash_offset = phase as f32;
        // As hayro: kurbo cannot deal with dashes of exactly 0.
        state.dash_array = dashes
            .iter()
            .map(|&n| n as f32)
            .map(|n| if n == 0.0 { 0.01 } else { n })
            .collect();
        Some(i + 1)
    }

    fn state(&mut self) -> &mut State {
        self.states.last_mut().expect("there is always a state")
    }

    fn operator(&mut self, op: &[u8], n: &[f64]) -> Option<()> {
        let arity = |want: usize| (n.len() == want).then_some(());
        let point = |x: f64, y: f64| Point::new(x, y);
        match op {
            b"q" => {
                arity(0)?;
                let top = self.states.last()?.clone();
                self.states.push(top);
            }
            b"Q" => {
                arity(0)?;
                self.restore();
            }
            b"cm" => {
                arity(6)?;
                let t = Affine::new([n[0], n[1], n[2], n[3], n[4], n[5]]);
                self.state().ctm *= t;
            }
            b"w" => {
                arity(1)?;
                self.state().line_width = n[0] as f32;
            }
            b"J" => {
                arity(1)?;
                self.state().line_cap = match n[0] as i64 {
                    1 => Cap::Round,
                    2 => Cap::Square,
                    _ => Cap::Butt,
                };
            }
            b"j" => {
                arity(1)?;
                self.state().line_join = match n[0] as i64 {
                    1 => Join::Round,
                    2 => Join::Bevel,
                    _ => Join::Miter,
                };
            }
            b"M" => {
                arity(1)?;
                self.state().miter_limit = n[0] as f32;
            }
            // Flatness: hayro ignores it too.
            b"i" => arity(1)?,
            b"g" | b"G" => {
                arity(1)?;
                let v = channel(n[0]);
                self.set_color(op == b"G", [v, v, v, 255]);
            }
            b"rg" | b"RG" => {
                arity(3)?;
                let rgb = [channel(n[0]), channel(n[1]), channel(n[2]), 255];
                self.set_color(op == b"RG", rgb);
            }
            b"m" => {
                arity(2)?;
                let p = point(n[0], n[1]);
                self.last_point = p;
                self.sub_path_start = p;
                self.path.move_to(p);
            }
            b"l" => {
                arity(2)?;
                if !self.path.elements().is_empty() {
                    let last = self.last_point;
                    let mut p = point(n[0], n[1]);
                    self.last_point = p;
                    if last == p {
                        // As hayro: so a zero-length line can still show
                        // a round cap.
                        p.x += 0.0001;
                    }
                    self.path.line_to(p);
                }
            }
            b"c" | b"v" | b"y" => {
                arity(if op == b"c" { 6 } else { 4 })?;
                if !self.path.elements().is_empty() {
                    let (p1, p2, p3) = match op {
                        b"c" => (point(n[0], n[1]), point(n[2], n[3]), point(n[4], n[5])),
                        b"v" => (self.last_point, point(n[0], n[1]), point(n[2], n[3])),
                        _ => {
                            let p3 = point(n[2], n[3]);
                            (point(n[0], n[1]), p3, p3)
                        }
                    };
                    self.last_point = p3;
                    self.path.curve_to(p1, p2, p3);
                }
            }
            b"h" => {
                arity(0)?;
                self.close();
            }
            b"re" => {
                arity(4)?;
                let rect = Rect::new(n[0], n[1], n[0] + n[2], n[1] + n[3]);
                self.path.extend(rect.to_path(0.1));
            }
            b"S" => self.paint(n, false, None, true)?,
            b"s" => self.paint(n, true, None, true)?,
            b"f" | b"F" => self.paint(n, false, Some(FillRule::NonZero), false)?,
            b"f*" => self.paint(n, false, Some(FillRule::EvenOdd), false)?,
            b"B" => self.paint(n, false, Some(FillRule::NonZero), true)?,
            b"B*" => self.paint(n, false, Some(FillRule::EvenOdd), true)?,
            b"b" => self.paint(n, true, Some(FillRule::NonZero), true)?,
            b"b*" => self.paint(n, true, Some(FillRule::EvenOdd), true)?,
            b"n" => {
                arity(0)?;
                if let Some(rule) = self.clip
                    && !self.path.elements().is_empty()
                {
                    let ctm = self.states.last()?.ctm;
                    let clip = ctm * self.path.clone();
                    self.push_clip(clip, rule);
                    self.clip = None;
                }
                self.path.truncate(0);
            }
            b"W" => {
                arity(0)?;
                self.clip = Some(FillRule::NonZero);
            }
            b"W*" => {
                arity(0)?;
                self.clip = Some(FillRule::EvenOdd);
            }
            _ => return None,
        }
        Some(())
    }

    fn set_color(&mut self, stroke: bool, color: [u8; 4]) {
        let state = self.state();
        if stroke {
            state.stroke_color = color;
        } else {
            state.fill_color = color;
        }
    }

    /// `h`, as hayro closes a path.
    fn close(&mut self) {
        if self.path.elements().last() != Some(&PathEl::ClosePath) && !self.path.is_empty() {
            self.path.close_path();
            self.last_point = self.sub_path_start;
        }
    }

    /// The painting operators: close first or not, fill with a rule or
    /// not, stroke or not; then the path is gone.
    fn paint(
        &mut self,
        n: &[f64],
        close: bool,
        fill: Option<FillRule>,
        stroke: bool,
    ) -> Option<()> {
        if !n.is_empty() {
            return None;
        }
        if close {
            self.close();
        }
        let state = self.states.last()?.clone();
        let path = std::mem::take(&mut self.path);
        if let Some(rule) = fill {
            let bbox = fast_bounding_box(&path);
            if !nearly_zero(bbox.width()) && !nearly_zero(bbox.height()) {
                let shape = as_rect(&path).map(|r| r.to_path(0.1));
                let shape = shape.as_ref().unwrap_or(&path);
                self.builder
                    .paint_fill(shape, state.ctm, state.fill_color, rule);
            } else {
                // As hayro (pdf.js issue 4260): a fill with no area is a
                // fine line across its box.
                let mut line = BezPath::new();
                line.move_to((bbox.x0, bbox.y0));
                line.line_to((bbox.x1, bbox.y1));
                let stroke = Stroke {
                    width: f64::from(0.001f32),
                    join: Join::Bevel,
                    miter_limit: 10.0,
                    start_cap: Cap::Butt,
                    end_cap: Cap::Butt,
                    dash_pattern: Default::default(),
                    dash_offset: 0.0,
                };
                self.builder
                    .paint_stroke(&line, state.ctm, state.fill_color, stroke);
            }
        }
        if stroke {
            let shape = as_rect(&path).map(|r| r.to_path(0.1));
            let shape = shape.as_ref().unwrap_or(&path);
            self.builder
                .paint_stroke(shape, state.ctm, state.stroke_color, state.stroke());
        }
        self.path = path;
        self.path.truncate(0);
        Some(())
    }

    /// Pushes a clip in page space, as hayro's context does: a rectangle
    /// that holds the whole drawing area changes nothing and is left out.
    fn push_clip(&mut self, clip: BezPath, rule: FillRule) {
        let area = *self.bboxes.last().expect("there is always an area");
        let real = if let Some(rect) = as_rect(&clip) {
            if nearly_ge(area.min_x(), rect.min_x())
                && nearly_ge(area.min_y(), rect.min_y())
                && nearly_le(area.max_x(), rect.max_x())
                && nearly_le(area.max_y(), rect.max_y())
            {
                false
            } else {
                self.builder.push_clip_rect(&rect);
                self.bboxes.push(area.intersect(rect));
                true
            }
        } else {
            let bounds = clip.bounding_box();
            self.builder.push_clip_path(&ClipPath {
                path: clip,
                fill: rule,
            });
            self.bboxes.push(area.intersect(bounds));
            true
        };
        self.state().clips.push(real);
    }

    /// `Q`, as hayro restores: pop the clips back to the saved state's,
    /// then the state, never the first one.
    fn restore(&mut self) {
        let target = self.states[self.states.len().saturating_sub(2)].clips.len();
        while self.states.last().is_some_and(|s| s.clips.len() > target) {
            if self.state().clips.pop() == Some(true) {
                self.builder.pop_clip();
                self.bboxes.pop();
            }
        }
        if self.states.len() > 1 {
            self.states.pop();
        }
    }
}

/// A colour component as hayro turns one into a byte.
fn channel(v: f64) -> u8 {
    (v as f32 * 255.0 + 0.5) as u8
}

fn is_white(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

/// Bytes that continue an operator: not white space or a delimiter.
fn is_regular(b: u8) -> bool {
    !is_white(b)
        && !matches!(
            b,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

/// The number at `content[i..]`, read as hayro-syntax reads it, and
/// where it ends; `None` for anything unusual, which hayro then reads.
fn number(content: &[u8], mut i: usize) -> Option<(f64, usize)> {
    const POWERS_OF_10: [f64; 20] = [
        1.0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19,
    ];
    let negative = match content.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let (mut mantissa, mut digits, mut shift, mut dot) = (0u64, 0usize, 0u32, false);
    while let Some(&b) = content.get(i) {
        match b {
            b'0'..=b'9' => {
                mantissa = mantissa.checked_mul(10)?.checked_add(u64::from(b - b'0'))?;
                digits += 1;
                if dot {
                    shift += 1;
                }
            }
            b'.' if !dot => dot = true,
            _ => break,
        }
        i += 1;
    }
    if digits == 0 || content.get(i).is_some_and(|&b| is_regular(b)) {
        return None;
    }
    let value = if dot {
        let value = mantissa as f64 / *POWERS_OF_10.get(shift as usize)?;
        if negative { -value } else { value }
    } else {
        // An integer, so `-0` is 0, not -0.0.
        let value = i64::try_from(mantissa).ok()?;
        (if negative { -value } else { value }) as f64
    };
    Some((value, i))
}

/// hayro's `path_as_rect`: the path is an axis-aligned rectangle, closed
/// or ending where it began.
fn as_rect(path: &BezPath) -> Option<Rect> {
    let points = match path.elements() {
        [
            PathEl::MoveTo(a),
            PathEl::LineTo(b),
            PathEl::LineTo(c),
            PathEl::LineTo(d),
            PathEl::ClosePath,
        ] => [*a, *b, *c, *d, *a],
        [
            PathEl::MoveTo(a),
            PathEl::LineTo(b),
            PathEl::LineTo(c),
            PathEl::LineTo(d),
            PathEl::LineTo(e),
        ] if nearly_eq(a.x, e.x) && nearly_eq(a.y, e.y) => [*a, *b, *c, *d, *e],
        _ => return None,
    };
    let mut previous = None;
    for edge in points.windows(2) {
        let same_x = nearly_eq(edge[0].x, edge[1].x);
        let same_y = nearly_eq(edge[0].y, edge[1].y);
        if same_x == same_y || previous == Some(same_x) {
            return None;
        }
        previous = Some(same_x);
    }
    Some(fast_bounding_box(path))
}

/// hayro's `fast_bounding_box`: control points count.
fn fast_bounding_box(path: &BezPath) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    let mut include = |p: Point| {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => include(p),
            PathEl::QuadTo(a, p) => {
                include(a);
                include(p);
            }
            PathEl::CurveTo(a, b, p) => {
                include(a);
                include(b);
                include(p);
            }
            PathEl::ClosePath => {}
        }
    }
    Rect::new(x0, y0, x1, y1)
}

/// hayro's tolerance for "nearly".
const NEARLY_ZERO: f32 = 1.0 / 256.0;

/// As hayro checks a fill's size: on f32.
fn nearly_zero(v: f64) -> bool {
    (v as f32).abs() <= NEARLY_ZERO
}

/// As hayro compares points and rectangles: on f64.
fn nearly_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= f64::from(NEARLY_ZERO)
}

fn nearly_le(a: f64, b: f64) -> bool {
    nearly_eq(a, b) || a < b
}

fn nearly_ge(a: f64, b: f64) -> bool {
    nearly_eq(a, b) || a > b
}
