// SPDX-License-Identifier: GPL-3.0-or-later

//! A page's text and where each character is, for search and text
//! selection (docs/PLAN.md, Phase 1). Characters come in the order the
//! page draws them, which for nearly every file is reading order; spaces
//! and line breaks are added where the gaps between glyphs say a word or
//! a line ended. Invisible text, as OCR layers over scans use, counts.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};
use hayro::kurbo::{Affine, BezPath, Point, Rect, Vec2};

/// Glyph space is a thousandth of the text size.
const EM: f64 = 1000.0;
/// A glyph's box, up and down from its baseline, in glyph space: about
/// where the ascenders and descenders of most fonts reach.
const ASCENT: f64 = 800.0;
const DESCENT: f64 = -200.0;
/// A gap wider than this share of the text size is a space.
const SPACE: f64 = 0.15;
/// A glyph this far off the line, as a share of the text size, starts a
/// new one.
const NEW_LINE: f64 = 0.5;

/// A page's text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    /// The text, with spaces and line breaks where the page lays them out.
    pub text: String,
    /// For each `char` of [`PageText::text`], its box on the page in
    /// points from the page's top left: left, top, right, bottom. Added
    /// spaces and line breaks get an empty box where the gap starts.
    pub boxes: Vec<[f32; 4]>,
}

impl PageText {
    /// Where `query` occurs, ignoring case, with any run of spaces in it
    /// matching any run of spaces or line breaks: for each match, the
    /// indices of its characters' boxes.
    pub fn find(&self, query: &str) -> Vec<std::ops::Range<usize>> {
        let query: Vec<char> = fold(query.trim());
        if query.is_empty() {
            return Vec::new();
        }
        let text = fold(&self.text);
        let mut found = Vec::new();
        let mut start = 0;
        while start < text.len() {
            if let Some(end) = match_at(&text, start, &query) {
                found.push(start..end);
                start = end;
            } else {
                start += 1;
            }
        }
        found
    }

    /// The boxes of characters `range`, joined into one box per line.
    pub fn line_boxes(&self, range: std::ops::Range<usize>) -> Vec<[f32; 4]> {
        let chars: Vec<char> = self.text.chars().collect();
        let mut out = Vec::new();
        let mut line: Option<[f32; 4]> = None;
        for i in range {
            if chars[i] == '\n' {
                out.extend(line.take());
                continue;
            }
            let b = self.boxes[i];
            if b[2] <= b[0] && b[3] <= b[1] {
                continue;
            }
            line = Some(line.map_or(b, |l| {
                [
                    l[0].min(b[0]),
                    l[1].min(b[1]),
                    l[2].max(b[2]),
                    l[3].max(b[3]),
                ]
            }));
        }
        out.extend(line);
        out
    }
}

/// Text lowered for matching, one `char` for each of the text's, every
/// kind of white space made a plain space.
fn fold(text: &str) -> Vec<char> {
    text.chars()
        .map(|c| {
            if c.is_whitespace() {
                ' '
            } else {
                c.to_lowercase().next().unwrap_or(c)
            }
        })
        .collect()
}

/// Where a match of `query` that starts at `start` in `text` ends.
fn match_at(text: &[char], start: usize, query: &[char]) -> Option<usize> {
    let (mut t, mut q) = (start, 0);
    while q < query.len() {
        if query[q] == ' ' {
            // One or more spaces match one or more spaces.
            if text.get(t) != Some(&' ') {
                return None;
            }
            while text.get(t) == Some(&' ') {
                t += 1;
            }
            while query.get(q) == Some(&' ') {
                q += 1;
            }
            continue;
        }
        if text.get(t) != Some(&query[q]) {
            return None;
        }
        t += 1;
        q += 1;
    }
    Some(t)
}

/// A device that collects the text drawn, with its boxes.
#[derive(Default)]
pub(crate) struct TextDevice {
    pub(crate) text: PageText,
    /// Where the last glyph ended, its direction and its size.
    last: Option<(Point, Vec2, f64)>,
}

impl TextDevice {
    fn glyph(&mut self, transform: Affine, unicode: Option<BfString>, advance: f64) {
        let origin = transform * Point::ZERO;
        let along = transform * Point::new(advance, 0.0) - origin;
        let up = transform * Point::new(0.0, EM) - origin;
        let size = up.hypot();
        if !size.is_finite() || size <= 0.0 {
            return;
        }
        let dir = if along.hypot() > 0.0 {
            along / along.hypot()
        } else {
            let run = transform * Point::new(1.0, 0.0) - origin;
            run / run.hypot().max(f64::MIN_POSITIVE)
        };
        if let Some((end, last_dir, last_size)) = self.last {
            let gap = (origin - end).dot(dir);
            let off = (origin - end).cross(dir).abs();
            let size = size.max(last_size);
            let ends_space = self.text.text.ends_with([' ', '\n']);
            if off > NEW_LINE * size || gap < -size || last_dir.dot(dir) < 0.9 {
                self.push('\n', end);
            } else if gap > SPACE * size && !ends_space {
                self.push(' ', end);
            }
        }
        let corners = [
            Point::new(0.0, DESCENT),
            Point::new(advance, DESCENT),
            Point::new(0.0, ASCENT),
            Point::new(advance, ASCENT),
        ]
        .map(|p| transform * p);
        let bbox = corners[1..]
            .iter()
            .fold(Rect::from_points(corners[0], corners[0]), |r, p| {
                r.union_pt(*p)
            });
        let text = match unicode {
            Some(BfString::Char(c)) => ligature(c).map_or_else(|| c.to_string(), str::to_owned),
            Some(BfString::String(s)) => s,
            // Unknown characters still take their place, so matches
            // around them line up.
            None => '\u{fffd}'.to_string(),
        };
        // A ligature's characters share its box, in slices.
        let n = text.chars().count().max(1) as f64;
        for (i, c) in text.chars().enumerate() {
            let (a, b) = (i as f64 / n, (i + 1) as f64 / n);
            let slice = if dir.x.abs() >= dir.y.abs() {
                let w = bbox.width();
                Rect::new(bbox.x0 + w * a, bbox.y0, bbox.x0 + w * b, bbox.y1)
            } else {
                let h = bbox.height();
                Rect::new(bbox.x0, bbox.y0 + h * a, bbox.x1, bbox.y0 + h * b)
            };
            self.text.text.push(c);
            self.text.boxes.push([
                slice.x0 as f32,
                slice.y0 as f32,
                slice.x1 as f32,
                slice.y1 as f32,
            ]);
        }
        self.last = Some((origin + along, dir, size));
    }

    fn push(&mut self, c: char, at: Point) {
        self.text.text.push(c);
        let (x, y) = (at.x as f32, at.y as f32);
        self.text.boxes.push([x, y, x, y]);
    }
}

/// A typographic ligature's letters, so "ﬁnd" is found as "find".
fn ligature(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{fb00}' => "ff",
        '\u{fb01}' => "fi",
        '\u{fb02}' => "fl",
        '\u{fb03}' => "ffi",
        '\u{fb04}' => "ffl",
        '\u{fb05}' | '\u{fb06}' => "st",
        _ => return None,
    })
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let transform = props.transform * glyph.transform();
            let (unicode, advance) = match &**glyph {
                Glyph::Outline(outline) => (
                    outline.as_unicode(),
                    outline.advance_width().map_or(EM / 2.0, f64::from),
                ),
                // Type 3 glyphs live in their own space; half an em is a
                // guess at their width.
                Glyph::Type3(type3) => (type3.as_unicode(), EM / 2.0),
            };
            self.glyph(transform, unicode, advance);
        }
    }

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str) -> PageText {
        PageText {
            text: text.into(),
            boxes: text
                .chars()
                .enumerate()
                .map(|(i, _)| [i as f32, 0.0, i as f32 + 1.0, 1.0])
                .collect(),
        }
    }

    #[test]
    fn search_ignores_case_and_how_words_are_spaced() {
        let text = page("Hello  World\nhello world, HELLO");
        assert_eq!(text.find("hello world"), [0..12, 13..24]);
        assert_eq!(text.find("hello").len(), 3);
        assert_eq!(text.find("  "), []);
        assert_eq!(text.find("planet"), []);
    }

    #[test]
    fn a_match_over_a_line_break_has_a_box_per_line() {
        let mut text = page("ab\ncd");
        text.boxes[3] = [0.0, 2.0, 1.0, 3.0];
        text.boxes[4] = [1.0, 2.0, 2.0, 3.0];
        assert_eq!(
            text.line_boxes(0..5),
            [[0.0, 0.0, 2.0, 1.0], [0.0, 2.0, 2.0, 3.0]]
        );
    }
}
