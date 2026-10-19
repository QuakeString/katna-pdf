// SPDX-License-Identifier: GPL-3.0-or-later

//! Measuring on pages (docs/PLAN.md §2.5 and §4.1): the lines a page
//! draws, collected so a measuring tool can snap to their ends, their
//! middles and where two cross, and the lengths and areas measured.
//! Points are in points from the page's top left, as everywhere else
//! the app places things on pages.

use std::collections::HashMap;

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};
use hayro::kurbo::{BezPath, PathEl, Point};

/// A straight piece of a line the page draws: from `[0], [1]` to
/// `[2], [3]`.
pub type Segment = [f32; 4];

/// Segments kept from one page at most: a large drawing's worth.
const MOST: usize = 4_000_000;
/// The side of the grid's squares, in points.
const CELL: f32 = 16.0;
/// Pairs of segments looked at for crossings near the pointer, at most.
const CROSSINGS: usize = 400;

/// What a point snapped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
    /// A segment's end.
    End,
    /// The middle of a segment.
    Middle,
    /// Where two segments cross.
    Crossing,
    /// Somewhere along a segment.
    OnLine,
}

/// A page's segments, in a grid for looking up those near a point.
#[derive(Debug, Default, Clone)]
pub struct Snaps {
    segments: Vec<Segment>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl Snaps {
    pub fn new(segments: Vec<Segment>) -> Self {
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, s) in segments.iter().enumerate() {
            let (c0, r0) = cell(s[0].min(s[2]), s[1].min(s[3]));
            let (c1, r1) = cell(s[0].max(s[2]), s[1].max(s[3]));
            // A segment over very many squares (a sheet's border) is
            // kept in those along it only.
            if (c1 - c0 + 1) as i64 * (r1 - r0 + 1) as i64 > 64 {
                let steps = ((s[2] - s[0]).hypot(s[3] - s[1]) / (CELL / 2.0)).ceil() as usize;
                let mut seen = Vec::new();
                for k in 0..=steps.max(1) {
                    let t = k as f32 / steps.max(1) as f32;
                    let at = cell(s[0] + (s[2] - s[0]) * t, s[1] + (s[3] - s[1]) * t);
                    if !seen.contains(&at) {
                        seen.push(at);
                        grid.entry(at).or_default().push(i as u32);
                    }
                }
                continue;
            }
            for c in c0..=c1 {
                for r in r0..=r1 {
                    grid.entry((c, r)).or_default().push(i as u32);
                }
            }
        }
        Snaps { segments, grid }
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The segments that may come within `radius` of `at`.
    fn near(&self, at: [f32; 2], radius: f32) -> Vec<&Segment> {
        let (c0, r0) = cell(at[0] - radius, at[1] - radius);
        let (c1, r1) = cell(at[0] + radius, at[1] + radius);
        let mut ids: Vec<u32> = Vec::new();
        for c in c0..=c1 {
            for r in r0..=r1 {
                if let Some(cell) = self.grid.get(&(c, r)) {
                    ids.extend(cell);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|i| &self.segments[i as usize])
            .collect()
    }

    /// The point to snap `at` to, within `radius`: an end first, then a
    /// crossing, then a middle, then the nearest point along a line.
    pub fn snap(&self, at: [f32; 2], radius: f32) -> Option<([f32; 2], SnapKind)> {
        let near = self.near(at, radius);
        let dist = |p: [f32; 2]| (p[0] - at[0]).hypot(p[1] - at[1]);
        let closest = |points: &mut dyn Iterator<Item = [f32; 2]>| {
            points
                .map(|p| (dist(p), p))
                .filter(|(d, _)| *d <= radius)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, p)| p)
        };
        if let Some(p) = closest(&mut near.iter().flat_map(|s| [[s[0], s[1]], [s[2], s[3]]])) {
            return Some((p, SnapKind::End));
        }
        // Only segments that pass close by can cross close by.
        let passing: Vec<&Segment> = near
            .iter()
            .copied()
            .filter(|s| dist(nearest_on(s, at)) <= radius)
            .collect();
        let mut crossings = Vec::new();
        'pairs: for (i, a) in passing.iter().enumerate() {
            for b in &passing[i + 1..] {
                if crossings.len() >= CROSSINGS {
                    break 'pairs;
                }
                if let Some(p) = crossing(a, b) {
                    crossings.push(p);
                }
            }
        }
        if let Some(p) = closest(&mut crossings.into_iter()) {
            return Some((p, SnapKind::Crossing));
        }
        let middles = passing
            .iter()
            .map(|s| [(s[0] + s[2]) / 2.0, (s[1] + s[3]) / 2.0]);
        if let Some(p) = closest(&mut middles.into_iter()) {
            return Some((p, SnapKind::Middle));
        }
        closest(&mut passing.iter().map(|s| nearest_on(s, at))).map(|p| (p, SnapKind::OnLine))
    }
}

fn cell(x: f32, y: f32) -> (i32, i32) {
    ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
}

/// The point of `s` nearest `p`.
fn nearest_on(s: &Segment, p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (s[2] - s[0], s[3] - s[1]);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return [s[0], s[1]];
    }
    let t = (((p[0] - s[0]) * dx + (p[1] - s[1]) * dy) / len2).clamp(0.0, 1.0);
    [s[0] + dx * t, s[1] + dy * t]
}

/// Where `a` and `b` cross, if they do.
fn crossing(a: &Segment, b: &Segment) -> Option<[f32; 2]> {
    let (r, s) = ([a[2] - a[0], a[3] - a[1]], [b[2] - b[0], b[3] - b[1]]);
    let denom = r[0] * s[1] - r[1] * s[0];
    if denom.abs() < 1e-6 {
        return None;
    }
    let q = [b[0] - a[0], b[1] - a[1]];
    let t = (q[0] * s[1] - q[1] * s[0]) / denom;
    let u = (q[0] * r[1] - q[1] * r[0]) / denom;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
        .then(|| [a[0] + r[0] * t, a[1] + r[1] * t])
}

/// The length of the line through `points`.
pub fn length(points: &[[f32; 2]]) -> f64 {
    points
        .windows(2)
        .map(|w| f64::from(w[1][0] - w[0][0]).hypot(f64::from(w[1][1] - w[0][1])))
        .sum()
}

/// The length around the closed shape through `points`.
pub fn perimeter(points: &[[f32; 2]]) -> f64 {
    match (points.first(), points.last()) {
        (Some(first), Some(last)) if points.len() > 2 => {
            length(points) + f64::from(first[0] - last[0]).hypot(f64::from(first[1] - last[1]))
        }
        _ => length(points),
    }
}

/// The area inside the closed shape through `points`.
pub fn area(points: &[[f32; 2]]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let twice: f64 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1])
        })
        .sum();
    twice.abs() / 2.0
}

/// A device that keeps the straight pieces of every line drawn, curves
/// flattened into a few.
#[derive(Default)]
pub(crate) struct LineDevice {
    pub(crate) segments: Vec<Segment>,
}

impl<'a> Device<'a> for LineDevice {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, _: &DrawMode) {
        let transform = props.transform;
        let mut start = Point::ZERO;
        let mut at = Point::ZERO;
        let push = |segments: &mut Vec<Segment>, a: Point, b: Point| {
            if segments.len() < MOST && (a.x, a.y) != (b.x, b.y) {
                let (a, b) = (transform * a, transform * b);
                segments.push([a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
            }
        };
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    start = p;
                    at = p;
                }
                PathEl::LineTo(p) => {
                    push(&mut self.segments, at, p);
                    at = p;
                }
                PathEl::QuadTo(c, p) => {
                    let mut last = at;
                    for k in 1..=4 {
                        let t = f64::from(k) / 4.0;
                        let q = at.lerp(c, t).lerp(c.lerp(p, t), t);
                        push(&mut self.segments, last, q);
                        last = q;
                    }
                    at = p;
                }
                PathEl::CurveTo(c1, c2, p) => {
                    let mut last = at;
                    for k in 1..=6 {
                        let t = f64::from(k) / 6.0;
                        let (a, b, c) = (at.lerp(c1, t), c1.lerp(c2, t), c2.lerp(p, t));
                        let q = a.lerp(b, t).lerp(b.lerp(c, t), t);
                        push(&mut self.segments, last, q);
                        last = q;
                    }
                    at = p;
                }
                PathEl::ClosePath => {
                    push(&mut self.segments, at, start);
                    at = start;
                }
            }
        }
    }

    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snaps_to_ends_then_crossings_then_middles() {
        let snaps = Snaps::new(vec![[0.0, 0.0, 100.0, 0.0], [50.0, -50.0, 50.0, 50.0]]);
        assert_eq!(
            snaps.snap([98.0, 1.0], 5.0),
            Some(([100.0, 0.0], SnapKind::End))
        );
        assert_eq!(
            snaps.snap([52.0, 2.0], 5.0),
            Some(([50.0, 0.0], SnapKind::Crossing))
        );
        assert_eq!(
            snaps.snap([51.0, 30.0], 5.0),
            Some(([50.0, 30.0], SnapKind::OnLine))
        );
        let one = Snaps::new(vec![[0.0, 0.0, 100.0, 0.0]]);
        assert_eq!(
            one.snap([49.0, 3.0], 5.0),
            Some(([50.0, 0.0], SnapKind::Middle))
        );
        assert_eq!(one.snap([49.0, 30.0], 5.0), None);
        // A long border is found anywhere along it.
        let border = Snaps::new(vec![[0.0, 0.0, 2000.0, 2000.0]]);
        assert_eq!(
            border.snap([1200.0, 1203.0], 5.0).map(|s| s.1),
            Some(SnapKind::OnLine)
        );
    }

    #[test]
    fn lengths_and_areas() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(length(&square[..2]), 10.0);
        assert_eq!(perimeter(&square), 40.0);
        assert_eq!(area(&square), 100.0);
        assert_eq!(area(&square[..2]), 0.0);
    }
}
