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
    /// Where `query` occurs, ignoring case and accents, with any run of
    /// spaces in it matching any run of spaces or line breaks: for each
    /// match, the indices of its characters' boxes.
    pub fn find(&self, query: &str) -> Vec<std::ops::Range<usize>> {
        self.find_with(query, FindOptions::default())
    }

    /// [`PageText::find`] with `options`. A query in double quotes is an
    /// exact phrase: whole words, no close spellings. A word broken over
    /// two lines with a hyphen is found whole.
    pub fn find_with(&self, query: &str, options: FindOptions) -> Vec<std::ops::Range<usize>> {
        let mut options = options;
        let mut query = query.trim();
        if let Some(inner) = query
            .strip_prefix('"')
            .and_then(|q| q.strip_suffix('"'))
            .filter(|q| !q.trim().is_empty())
        {
            query = inner.trim();
            options.whole_words = true;
            options.fuzzy = false;
        }
        let query = Folded::new(query, options).chars;
        if query.is_empty() || query.iter().all(|c| *c == ' ') {
            return Vec::new();
        }
        let text = Folded::new(&self.text, options);
        let chars = &text.chars;
        let errors = if options.fuzzy {
            allowed_errors(&query)
        } else {
            0
        };
        let mut found = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let at_word = start == 0 || !is_word(chars[start - 1]);
            let end = match_at(chars, start, &query)
                .or_else(|| {
                    // Close spellings start where a word does.
                    (errors > 0 && at_word && is_word(chars[start]))
                        .then(|| close_at(chars, start, &query, errors))
                        .flatten()
                })
                .filter(|&end| {
                    !options.whole_words
                        || (at_word && (end >= chars.len() || !is_word(chars[end])))
                });
            match end {
                Some(end) if end > start => {
                    found.push(text.origin[start]..text.origin[end - 1] + 1);
                    start = end;
                }
                _ => start += 1,
            }
        }
        found
    }

    /// The words around characters `range` on one line, for a list of
    /// matches: what comes before, the match, and what comes after.
    pub fn context(&self, range: std::ops::Range<usize>) -> (String, String, String) {
        const AROUND: usize = 40;
        let chars: Vec<char> = self.text.chars().collect();
        let end = range.end.min(chars.len());
        let start = range.start.min(end);
        // One space for every run of white space.
        let line = |cs: &[char]| -> String {
            let mut out = String::new();
            for c in cs {
                if c.is_whitespace() {
                    if !out.ends_with(' ') {
                        out.push(' ');
                    }
                } else {
                    out.push(*c);
                }
            }
            out
        };
        let mut from = start.saturating_sub(AROUND);
        // Start at a word, unless the line starts nearer.
        if from > 0 {
            while from < start && !chars[from - 1].is_whitespace() {
                from += 1;
            }
        }
        let mut to = (end + AROUND).min(chars.len());
        if to < chars.len() {
            while to > end && !chars[to].is_whitespace() {
                to -= 1;
            }
        }
        let before = line(&chars[from..start]).trim_start().to_owned();
        let after = line(&chars[end..to]).trim_end().to_owned();
        (
            if from > 0 {
                format!("…{before}")
            } else {
                before
            },
            line(&chars[start..end]),
            if to < chars.len() {
                format!("{after}…")
            } else {
                after
            },
        )
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

/// How a search matches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    /// Capitals only match capitals.
    pub match_case: bool,
    /// Only whole words match.
    pub whole_words: bool,
    /// Letters with accents only match the same accents.
    pub match_accents: bool,
    /// Close spellings match too: a letter or two wrong, missing, extra
    /// or swapped, for words of four letters or more.
    pub fuzzy: bool,
}

/// Text made ready for matching: every kind of white space a plain space,
/// lowered and without accents as the options say, soft hyphens and
/// hyphens that break a word over two lines left out. `origin` gives each
/// character's index in the text it came from.
struct Folded {
    chars: Vec<char>,
    origin: Vec<usize>,
}

impl Folded {
    fn new(text: &str, options: FindOptions) -> Self {
        use unicode_normalization::UnicodeNormalization;
        use unicode_normalization::char::is_combining_mark;
        let source: Vec<char> = text.chars().collect();
        let mut chars = Vec::with_capacity(source.len());
        let mut origin = Vec::with_capacity(source.len());
        let mut i = 0;
        while i < source.len() {
            let c = source[i];
            if c == '\u{ad}' {
                i += 1;
                continue;
            }
            // "pay-\nment" reads as "payment".
            if c == '-'
                && source.get(i + 1) == Some(&'\n')
                && i > 0
                && source[i - 1].is_alphabetic()
                && source.get(i + 2).is_some_and(|c| c.is_lowercase())
            {
                i += 2;
                continue;
            }
            if !options.match_accents && is_combining_mark(c) {
                i += 1;
                continue;
            }
            let mut c = if c.is_whitespace() { ' ' } else { c };
            if !options.match_accents {
                c = c.nfd().next().unwrap_or(c);
            }
            if !options.match_case {
                c = c.to_lowercase().next().unwrap_or(c);
            }
            chars.push(c);
            origin.push(i);
            i += 1;
        }
        Folded { chars, origin }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// How many letters a close spelling may get wrong: none for short
/// words, then one for every four letters, at most two.
fn allowed_errors(query: &[char]) -> usize {
    let letters = query.iter().filter(|c| is_word(**c)).count();
    match letters {
        0..4 => 0,
        4..8 => 1,
        _ => 2,
    }
}

/// Where a close spelling of `query` starting at `start` ends: the end
/// with the fewest changes (a letter wrong, missing, extra, or two
/// swapped), at most `errors`, ending at a word's end where it can.
fn close_at(text: &[char], start: usize, query: &[char], errors: usize) -> Option<usize> {
    let longest = (query.len() + errors).min(text.len() - start);
    let piece = &text[start..start + longest];
    let (n, m) = (query.len(), piece.len());
    // d[i][j]: changes to make query[..i] into piece[..j].
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(query[i - 1] != piece[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && query[i - 1] == piece[j - 2] && query[i - 2] == piece[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    let ends_word = |j: usize| start + j >= text.len() || !is_word(text[start + j]);
    (1..=m)
        // Ends on a letter if the query does, so a comma after a word stays
        // out.
        .filter(|&j| d[n][j] <= errors && j + errors >= n)
        .filter(|&j| is_word(piece[j - 1]) || !is_word(query[n - 1]))
        .min_by_key(|&j| (d[n][j], !ends_word(j), j.abs_diff(n)))
        .filter(|&j| ends_word(j))
        .map(|j| start + j)
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
    fn options_change_what_matches() {
        let text = page("Café cafe CAFE cafeteria");
        let all = FindOptions::default();
        assert_eq!(text.find_with("cafe", all).len(), 4);
        let case = FindOptions {
            match_case: true,
            ..all
        };
        assert_eq!(text.find_with("cafe", case), [5..9, 15..19]);
        let words = FindOptions {
            whole_words: true,
            ..all
        };
        assert_eq!(text.find_with("cafe", words), [0..4, 5..9, 10..14]);
        let accents = FindOptions {
            match_accents: true,
            ..all
        };
        assert_eq!(text.find_with("café", accents), vec![0..4]);
        // Quotes ask for the phrase as written, in whole words.
        assert_eq!(text.find_with("\"cafe\"", all).len(), 3);
    }

    #[test]
    fn close_spellings_are_found_when_asked() {
        let text = page("Payment summary: the paymnet was recieved. Pay me.");
        let fuzzy = FindOptions {
            fuzzy: true,
            ..FindOptions::default()
        };
        let words = |q: &str, o| -> Vec<String> {
            text.find_with(q, o)
                .into_iter()
                .map(|r| text.text.chars().skip(r.start).take(r.len()).collect())
                .collect()
        };
        assert_eq!(words("payment", fuzzy), ["Payment", "paymnet"]);
        assert_eq!(words("received", fuzzy), ["recieved"]);
        assert_eq!(words("payment", FindOptions::default()), ["Payment"]);
        // Short words only match as typed.
        assert_eq!(words("pay", fuzzy), ["Pay", "pay", "Pay"]);
        assert_eq!(words("summery", fuzzy), ["summary"]);
        let text = page("le salaire, net");
        let found: Vec<String> = text
            .find_with("salaries", fuzzy)
            .into_iter()
            .map(|r| text.text.chars().skip(r.start).take(r.len()).collect())
            .collect();
        assert_eq!(found, ["salaire"]);
    }

    #[test]
    fn a_word_hyphenated_over_two_lines_is_found() {
        let text = page("the pay-\nment is due; well-known");
        assert_eq!(text.find("payment"), vec![4..13]);
        assert_eq!(text.find("well-known"), vec![22..32]);
        let (before, hit, after) = text.context(4..13);
        assert_eq!(
            (before.as_str(), hit.as_str(), after.as_str()),
            ("the ", "pay- ment", " is due; well-known")
        );
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
