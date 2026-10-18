// SPDX-License-Identifier: GPL-3.0-or-later

//! A quick rasterizer for tiles that hold only hairlines (docs/PLAN.md
//! §2.5, "a fast rasterizer").
//!
//! Zoomed out on a drawing, every stroke is drawn a pixel wide, and
//! vello_cpu spends about 2 µs on each path whatever its size. This draws
//! such a stroke as a one-pixel line straight into the tile: each column
//! (or row, for steep lines) it crosses gets the line's coverage of its
//! pixels, worked out from the line's thickness across that column. It
//! costs a few nanoseconds a pixel. Curves are flattened first, caps
//! lengthen the line, and clips must be rectangles. Overlaps within one
//! path cover twice where vello covers once, which only shows at the
//! joins of polylines.

use hayro::kurbo::{Affine, BezPath, PathEl, Point, Rect};

/// How far flattening may stray from a curve, in pixels.
const TOLERANCE: f64 = 0.1;

/// A tile being drawn: straight RGB in 0..=1, on white.
pub(super) struct Canvas {
    width: usize,
    height: usize,
    rgb: Vec<[f32; 3]>,
    /// The path being drawn's coverage of each pixel, summed so its
    /// pieces cover the pixels they share once, as vello's fill does.
    coverage: Vec<f32>,
    /// The pixels `coverage` holds anything for.
    touched: Vec<u32>,
    /// A stroke's points and a curve being flattened, kept between
    /// strokes so drawing one allocates nothing.
    points: Vec<Point>,
    flat: BezPath,
}

impl Canvas {
    pub(super) fn new(width: u32, height: u32) -> Canvas {
        Canvas {
            width: width as usize,
            height: height as usize,
            rgb: vec![[1.0; 3]; width as usize * height as usize],
            coverage: vec![0.0; width as usize * height as usize],
            touched: Vec::new(),
            points: Vec::new(),
            flat: BezPath::new(),
        }
    }

    /// The tile as RGBA bytes.
    pub(super) fn into_rgba(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgb.len() * 4);
        for [r, g, b] in self.rgb {
            let byte = |v: f32| (v * 255.0 + 0.5) as u8;
            out.extend_from_slice(&[byte(r), byte(g), byte(b), 255]);
        }
        out
    }

    /// Adds `coverage` of pixel (`x`, `y`), inside `clip`, to the path
    /// being drawn.
    fn cover(&mut self, x: i64, y: i64, coverage: f32, clip: &Bounds) {
        if x < clip.x0 || x >= clip.x1 || y < clip.y0 || y >= clip.y1 || coverage <= 0.0 {
            return;
        }
        self.add(y as usize * self.width + x as usize, coverage);
    }

    /// Paints the path drawn since the last paint in `color`.
    fn paint(&mut self, color: [u8; 4]) {
        let color = unit(color);
        for &i in &self.touched {
            let i = i as usize;
            let a = self.coverage[i].min(1.0) * color[3];
            self.coverage[i] = 0.0;
            let px = &mut self.rgb[i];
            for c in 0..3 {
                px[c] += (color[c] - px[c]) * a;
            }
        }
        self.touched.clear();
    }

    /// Fills the rectangle `rect`, in pixels, with `color`.
    pub(super) fn fill_rect(&mut self, rect: Rect, color: [u8; 4], clip: Rect) {
        let clip = self.bounds(clip);
        let (x0, x1) = (floor(rect.x0), ceil(rect.x1));
        let (y0, y1) = (floor(rect.y0), ceil(rect.y1));
        for y in y0.max(clip.y0)..y1.min(clip.y1) {
            let h = overlap(rect.y0, rect.y1, y as f64);
            for x in x0.max(clip.x0)..x1.min(clip.x1) {
                let w = overlap(rect.x0, rect.x1, x as f64);
                self.cover(x, y, (w * h) as f32, &clip);
            }
        }
        self.paint(color);
    }

    /// Strokes `path` through `transform` as a one-pixel line of `color`,
    /// lengthened by `cap` pixels at each end of each piece, inside
    /// `clip` (in pixels).
    pub(super) fn stroke(
        &mut self,
        path: &BezPath,
        transform: Affine,
        color: [u8; 4],
        cap: f64,
        clip: Rect,
    ) {
        let clip = self.bounds(clip);
        // Each subpath's points, flattened, then drawn piece by piece.
        let mut points = std::mem::take(&mut self.points);
        let mut flat = std::mem::take(&mut self.flat);
        points.clear();
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    self.polyline(&points, false, cap, &clip);
                    points.clear();
                    points.push(transform * p);
                }
                PathEl::LineTo(p) => points.push(transform * p),
                PathEl::ClosePath => {
                    if let Some(&first) = points.first() {
                        points.push(first);
                    }
                    self.polyline(&points, true, cap, &clip);
                    // A subpath after a close starts where it closed.
                    points.truncate(1);
                }
                PathEl::QuadTo(..) | PathEl::CurveTo(..) => {
                    let Some(&last) = points.last() else {
                        continue;
                    };
                    flat.truncate(0);
                    flat.move_to(last);
                    match *el {
                        PathEl::QuadTo(a, p) => flat.quad_to(transform * a, transform * p),
                        PathEl::CurveTo(a, b, p) => {
                            flat.curve_to(transform * a, transform * b, transform * p);
                        }
                        _ => unreachable!(),
                    }
                    hayro::kurbo::flatten(flat.iter(), TOLERANCE, |el| {
                        if let PathEl::LineTo(p) = el {
                            points.push(p);
                        }
                    });
                }
            }
        }
        self.polyline(&points, false, cap, &clip);
        self.paint(color);
        (self.points, self.flat) = (points, flat);
    }

    /// The pieces between `points`, with caps at the two ends unless
    /// `closed`. Joins get nothing: at a pixel wide they barely show.
    fn polyline(&mut self, points: &[Point], closed: bool, cap: f64, clip: &Bounds) {
        let n = points.len();
        for (k, pair) in points.windows(2).enumerate() {
            let start = if k == 0 && !closed { cap } else { 0.0 };
            let end = if k + 2 == n && !closed { cap } else { 0.0 };
            self.line(pair[0], pair[1], start, end, clip);
        }
    }

    /// One straight piece from `a` to `b`, in pixels, lengthened by
    /// `cap_a` and `cap_b` at its ends.
    fn line(&mut self, a: Point, b: Point, cap_a: f64, cap_b: f64, clip: &Bounds) {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        // Not `hypot`: it guards against overflow we cannot meet, slowly.
        let length = (dx * dx + dy * dy).sqrt();
        if length + cap_a + cap_b <= 0.0 || !length.is_finite() {
            return;
        }
        // Lengthen by the caps; a point with caps runs along x.
        let (ux, uy) = if length > 0.0 {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        };
        let a = Point::new(a.x - ux * cap_a, a.y - uy * cap_a);
        let b = Point::new(b.x + ux * cap_b, b.y + uy * cap_b);
        if (b.x - a.x).abs() >= (b.y - a.y).abs() {
            self.run(a.x, a.y, b.x, b.y, false, clip);
        } else {
            self.run(a.y, a.x, b.y, b.x, true, clip);
        }
    }

    /// A line along its major axis `u` from `u0` to `u1`, `v` its minor
    /// axis; `swap` when `u` is y.
    fn run(
        &mut self,
        mut u0: f64,
        mut v0: f64,
        mut u1: f64,
        mut v1: f64,
        swap: bool,
        clip: &Bounds,
    ) {
        if u1 < u0 {
            (u0, v0, u1, v1) = (u1, v1, u0, v0);
        }
        let slope = if u1 > u0 { (v1 - v0) / (u1 - u0) } else { 0.0 };
        // A one-pixel line is this thick along the minor axis.
        let thickness = (1.0 + slope * slope).sqrt();
        let ((lo, hi), (v_lo, v_hi)) = if swap {
            ((clip.y0, clip.y1), (clip.x0, clip.x1))
        } else {
            ((clip.x0, clip.x1), (clip.y0, clip.y1))
        };
        // A step along u and along v in `coverage`.
        let (u_step, v_step) = if swap {
            (self.width, 1)
        } else {
            (1, self.width)
        };
        let first = floor(u0).max(lo);
        let last = (ceil(u1) - 1).min(hi - 1);
        for u in first..=last {
            let (s0, s1) = ((u as f64).max(u0), ((u + 1) as f64).min(u1));
            if s1 <= s0 {
                continue;
            }
            let along = s1 - s0;
            let mid = v0 + slope * ((s0 + s1) / 2.0 - u0);
            let (b0, b1) = (mid - thickness / 2.0, mid + thickness / 2.0);
            // Both ranges are inside the tile, so the index is too.
            let column = u as usize * u_step;
            for v in floor(b0).max(v_lo)..ceil(b1).min(v_hi) {
                let coverage = (overlap(b0, b1, v as f64) * along) as f32;
                if coverage > 0.0 {
                    self.add(column + v as usize * v_step, coverage);
                }
            }
        }
    }

    /// Adds `coverage` of pixel `i` to the path being drawn.
    fn add(&mut self, i: usize, coverage: f32) {
        if self.coverage[i] == 0.0 {
            self.touched.push(i as u32);
        }
        self.coverage[i] += coverage;
    }

    /// `clip`, in pixels, as the pixels whose centres it holds, within the
    /// tile.
    fn bounds(&self, clip: Rect) -> Bounds {
        let edge = |v: f64, max: usize| ceil(v - 0.5).clamp(0, max as i64);
        Bounds {
            x0: edge(clip.x0, self.width),
            y0: edge(clip.y0, self.height),
            x1: edge(clip.x1, self.width),
            y1: edge(clip.y1, self.height),
        }
    }
}

/// Pixels `x0..x1` × `y0..y1` may be drawn.
struct Bounds {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

/// `v` rounded down. `f64::floor` is a call into the maths library on
/// x86-64 CPUs without SSE4.1, which Arch builds for; this is inline.
fn floor(v: f64) -> i64 {
    let t = v as i64;
    if (t as f64) > v { t - 1 } else { t }
}

/// `v` rounded up, as [`floor`].
fn ceil(v: f64) -> i64 {
    let t = v as i64;
    if (t as f64) < v { t + 1 } else { t }
}

/// How much of the pixel span `[at, at + 1]` lies in `[from, to]`.
fn overlap(from: f64, to: f64, at: f64) -> f64 {
    (to.min(at + 1.0) - from.max(at)).max(0.0)
}

fn unit(color: [u8; 4]) -> [f32; 4] {
    color.map(|c| f32::from(c) / 255.0)
}

/// Whether `path` is an axis-aligned rectangle; the rectangle if so.
pub(super) fn as_rect(path: &BezPath) -> Option<Rect> {
    let mut points = Vec::with_capacity(5);
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) if points.is_empty() => points.push(p),
            PathEl::LineTo(p) => points.push(p),
            PathEl::ClosePath => {}
            _ => return None,
        }
    }
    if points.len() == 5 && points[4] == points[0] {
        points.pop();
    }
    if points.len() != 4 {
        return None;
    }
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    let aligned = (0..4).all(|i| {
        let (p, q) = (points[i], points[(i + 1) % 4]);
        close(p.x, q.x) != close(p.y, q.y)
    });
    aligned.then(|| {
        points[1..]
            .iter()
            .fold(Rect::from_points(points[0], points[0]), |r, p| {
                r.union_pt(*p)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_the_standard_library() {
        for v in [-2.5, -2.0, -0.5, -0.0, 0.0, 0.3, 1.0, 1.7, 1e12, -1e12] {
            assert_eq!(floor(v), v.floor() as i64, "{v}");
            assert_eq!(ceil(v), v.ceil() as i64, "{v}");
        }
    }
}
