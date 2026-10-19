// SPDX-License-Identifier: GPL-3.0-or-later

//! Comparing two versions of a document (docs/PLAN.md §4.1, "compare two
//! documents, side by side and as a diff overlay"; §2.5, "compare two
//! revisions of a drawing as a coloured overlay").
//!
//! The words of both, in reading order across all pages, are lined up
//! the way `diff` lines up lines, so text that moved to another page is
//! not a change. Pages whose words agree are drawn small and their
//! pixels compared, which finds changed pictures and drawings. The
//! overlay draws the older page red where only it has ink, the newer
//! blue where only it has, and what both share grey.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffTag};

use crate::text::PageText;
use crate::{Bitmap, Document};

/// One word and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub page: usize,
    /// Left, top, right, bottom in points from the page's top left.
    pub rect: [f32; 4],
}

/// What a change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Words only the newer version has.
    Added,
    /// Words only the older version had.
    Removed,
    /// Words the newer version has in place of others.
    Changed,
    /// No words changed, but the page draws differently here: a picture,
    /// a drawing or the way text looks.
    Drawing,
}

/// One change between the two versions.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub kind: Kind,
    /// The older version's words, as indices into [`Comparison::old`].
    pub old: Range<usize>,
    /// The newer version's words, as indices into [`Comparison::new`].
    pub new: Range<usize>,
    /// For [`Kind::Drawing`]: the newer page and the area that differs.
    pub area: Option<(usize, [f32; 4])>,
}

/// Two versions compared.
#[derive(Debug, Clone, Default)]
pub struct Comparison {
    pub old: Vec<Word>,
    pub new: Vec<Word>,
    /// In reading order of the newer version.
    pub changes: Vec<Change>,
    /// For each newer page, the older page most of its words came from
    /// (the same number when neither has words to tell by).
    pub pages: Vec<Option<usize>>,
}

/// A diff that takes longer than this settles for a coarser answer.
const PATIENCE: Duration = Duration::from_secs(4);

/// The words of `doc`, page by page.
pub fn words(doc: &dyn Document) -> Vec<Word> {
    (0..doc.page_count())
        .flat_map(|page| page_words(&doc.text(page), page))
        .collect()
}

/// The words of one page's text.
pub fn page_words(text: &PageText, page: usize) -> Vec<Word> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut rect: Option<[f32; 4]> = None;
    let mut finish = |word: &mut String, rect: &mut Option<[f32; 4]>| {
        if !word.is_empty() {
            out.push(Word {
                text: std::mem::take(word),
                page,
                rect: rect.take().unwrap_or_default(),
            });
        }
        *rect = None;
    };
    for (c, b) in text.text.chars().zip(&text.boxes) {
        if c.is_whitespace() {
            finish(&mut word, &mut rect);
            continue;
        }
        word.push(c);
        if b[2] > b[0] || b[3] > b[1] {
            rect = Some(rect.map_or(*b, |r| {
                [
                    r[0].min(b[0]),
                    r[1].min(b[1]),
                    r[2].max(b[2]),
                    r[3].max(b[3]),
                ]
            }));
        }
    }
    finish(&mut word, &mut rect);
    out
}

/// Lines up the words of two versions.
pub fn compare_words(
    old: Vec<Word>,
    new: Vec<Word>,
    old_pages: usize,
    new_pages: usize,
) -> Comparison {
    let a: Vec<&str> = old.iter().map(|w| w.text.as_str()).collect();
    let b: Vec<&str> = new.iter().map(|w| w.text.as_str()).collect();
    let ops = similar::capture_diff_slices_deadline(
        Algorithm::Patience,
        &a,
        &b,
        Some(Instant::now() + PATIENCE),
    );
    let mut changes = Vec::new();
    // For each newer page, how many of its words came from each older one.
    let mut from: Vec<std::collections::BTreeMap<usize, usize>> =
        vec![Default::default(); new_pages];
    for op in &ops {
        let (tag, o, n) = op.as_tag_tuple();
        let kind = match tag {
            DiffTag::Equal => {
                for (i, j) in o.zip(n) {
                    if let Some(count) = from.get_mut(new[j].page) {
                        *count.entry(old[i].page).or_default() += 1;
                    }
                }
                continue;
            }
            DiffTag::Insert => Kind::Added,
            DiffTag::Delete => Kind::Removed,
            DiffTag::Replace => Kind::Changed,
        };
        // A change running over a page break is two: one on each page.
        for (o, n) in split_by_page(&old, o, &new, n) {
            let kind = match (o.is_empty(), n.is_empty()) {
                (true, _) => Kind::Added,
                (_, true) => Kind::Removed,
                _ => kind,
            };
            changes.push(Change {
                kind,
                old: o,
                new: n,
                area: None,
            });
        }
    }
    let pages = from
        .iter()
        .enumerate()
        .map(|(page, counts)| {
            counts
                .iter()
                .max_by_key(|(p, n)| (**n, std::cmp::Reverse(**p)))
                .map(|(p, _)| *p)
                .or((page < old_pages).then_some(page))
        })
        .collect();
    Comparison {
        old,
        new,
        changes,
        pages,
    }
}

/// `o` and `n` cut where either side moves to another page.
fn split_by_page(
    old: &[Word],
    o: Range<usize>,
    new: &[Word],
    n: Range<usize>,
) -> Vec<(Range<usize>, Range<usize>)> {
    let runs = |words: &[Word], r: Range<usize>| {
        let mut out: Vec<Range<usize>> = Vec::new();
        for i in r {
            match out.last_mut() {
                Some(run) if words[run.start].page == words[i].page => run.end = i + 1,
                _ => out.push(i..i + 1),
            }
        }
        out
    };
    let (a, b) = (runs(old, o.clone()), runs(new, n.clone()));
    let count = a.len().max(b.len()).max(1);
    (0..count)
        .map(|k| {
            let pick = |runs: &[Range<usize>], all: &Range<usize>| match runs.get(k) {
                Some(r) => r.clone(),
                // Nothing left on this side: an empty range where it ends.
                None => all.end..all.end,
            };
            (pick(&a, &o), pick(&b, &n))
        })
        .collect()
}

impl Comparison {
    /// The older version's text in a change, and the newer's.
    pub fn texts(&self, change: &Change) -> (String, String) {
        let join = |words: &[Word], r: &Range<usize>| {
            words[r.clone()]
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        (join(&self.old, &change.old), join(&self.new, &change.new))
    }

    /// The newer page a change is on, and where on it: its words, or
    /// for removed words the place they were taken from.
    pub fn new_place(&self, change: &Change) -> Option<(usize, [f32; 4])> {
        if let Some(area) = change.area {
            return Some(area);
        }
        if !change.new.is_empty() {
            let words = &self.new[change.new.clone()];
            return Some((words[0].page, union(words.iter().map(|w| w.rect))));
        }
        // A thin mark just after the word before, or before the next.
        let at = change.new.start;
        if let Some(prev) = at.checked_sub(1).and_then(|i| self.new.get(i)) {
            let r = prev.rect;
            return Some((prev.page, [r[2], r[1], r[2] + 2.0, r[3]]));
        }
        self.new.get(at).map(|next| {
            let r = next.rect;
            (next.page, [r[0] - 2.0, r[1], r[0], r[3]])
        })
    }

    /// The older page a change is on, and where on it.
    pub fn old_place(&self, change: &Change) -> Option<(usize, [f32; 4])> {
        if let Some((page, area)) = change.area {
            return Some((self.pages.get(page).copied().flatten()?, area));
        }
        if !change.old.is_empty() {
            let words = &self.old[change.old.clone()];
            return Some((words[0].page, union(words.iter().map(|w| w.rect))));
        }
        let at = change.old.start;
        if let Some(prev) = at.checked_sub(1).and_then(|i| self.old.get(i)) {
            let r = prev.rect;
            return Some((prev.page, [r[2], r[1], r[2] + 2.0, r[3]]));
        }
        self.old.get(at).map(|next| {
            let r = next.rect;
            (next.page, [r[0] - 2.0, r[1], r[0], r[3]])
        })
    }

    /// The marks on newer page `page`: each changed word's box and its
    /// kind, and the thin marks where words were removed.
    pub fn new_marks(&self, page: usize) -> Vec<([f32; 4], Kind)> {
        self.marks(page, true)
    }

    /// The marks on older page `page`.
    pub fn old_marks(&self, page: usize) -> Vec<([f32; 4], Kind)> {
        self.marks(page, false)
    }

    fn marks(&self, page: usize, newer: bool) -> Vec<([f32; 4], Kind)> {
        let mut out = Vec::new();
        for change in &self.changes {
            let (words, range) = if newer {
                (&self.new, &change.new)
            } else {
                (&self.old, &change.old)
            };
            if change.kind == Kind::Drawing || range.is_empty() {
                let place = if newer {
                    self.new_place(change)
                } else {
                    self.old_place(change)
                };
                if let Some((p, rect)) = place
                    && p == page
                {
                    out.push((rect, change.kind));
                }
                continue;
            }
            out.extend(
                words[range.clone()]
                    .iter()
                    .filter(|w| w.page == page)
                    .map(|w| (w.rect, change.kind)),
            );
        }
        out
    }

    /// Adds the areas of newer page `page` that draw differently from
    /// its older page, keeping the changes in reading order.
    pub fn add_drawing(&mut self, page: usize, areas: Vec<[f32; 4]>) {
        let at = self
            .changes
            .iter()
            .position(|c| self.new_place(c).is_some_and(|(p, _)| p > page))
            .unwrap_or(self.changes.len());
        let word = self
            .new
            .iter()
            .position(|w| w.page > page)
            .unwrap_or(self.new.len());
        let old_word = self.changes[..at].last().map_or(0, |c| c.old.end);
        self.changes.splice(
            at..at,
            areas.into_iter().map(|area| Change {
                kind: Kind::Drawing,
                old: old_word..old_word,
                new: word..word,
                area: Some((page, area)),
            }),
        );
    }

    /// Newer pages whose words all agree with their older page, which
    /// only a look at their pixels can tell apart.
    pub fn pages_to_look_at(&self) -> Vec<(usize, usize)> {
        self.pages
            .iter()
            .enumerate()
            .filter_map(|(page, old)| {
                let old = (*old)?;
                let touched = self.changes.iter().any(|c| {
                    c.kind != Kind::Drawing
                        && (self.new[c.new.clone()].iter().any(|w| w.page == page)
                            || self.old[c.old.clone()].iter().any(|w| w.page == old)
                            || self.new_place(c).is_some_and(|(p, _)| p == page))
                });
                (!touched).then_some((page, old))
            })
            .collect()
    }
}

/// Compares two documents' words. Slow for long ones: run it in the
/// background.
pub fn compare(old: &dyn Document, new: &dyn Document) -> Comparison {
    compare_words(words(old), words(new), old.page_count(), new.page_count())
}

fn union(rects: impl Iterator<Item = [f32; 4]>) -> [f32; 4] {
    rects
        .reduce(|a, b| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        })
        .unwrap_or_default()
}

/// The darkness of each pixel, 0 for white and 255 for black.
fn ink(bitmap: &Bitmap, x: u32, y: u32) -> u8 {
    if x >= bitmap.width || y >= bitmap.height {
        return 0;
    }
    let i = ((y * bitmap.width + x) * 4) as usize;
    let p = &bitmap.rgba[i..i + 3];
    // Rec. 601 luma, inverted.
    let luma = (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000;
    255 - luma as u8
}

/// The two pages drawn over each other: grey where they agree, red where
/// only the older has ink, blue where only the newer has. Both must be
/// drawn at the same scale; a smaller one is padded with white.
pub fn overlay(old: &Bitmap, new: &Bitmap) -> Bitmap {
    let (width, height) = (old.width.max(new.width), old.height.max(new.height));
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (a, b) = (ink(old, x, y), ink(new, x, y));
            let shared = a.min(b);
            // What both draw, faded so the changes stand out.
            let grey = 255 - shared / 2;
            let only_old = a - shared;
            let only_new = b - shared;
            rgba.extend_from_slice(&[
                grey.saturating_sub(only_new),
                grey.saturating_sub(only_old.max(only_new)),
                grey.saturating_sub(only_old),
                255,
            ]);
        }
    }
    Bitmap {
        width,
        height,
        rgba,
    }
}

/// Side of the squares pixels are compared in.
const TILE: u32 = 8;
/// A square differs when this many of its pixels differ by more than
/// [`INK_STEP`]: one stray pixel from smoothing is not a change.
const TILE_PIXELS: u32 = 3;
const INK_STEP: u8 = 64;

/// Where two drawings of a page differ, as rectangles in points, given
/// the scale both were drawn at. Nearby differences are joined.
pub fn differences(old: &Bitmap, new: &Bitmap, scale: f32) -> Vec<[f32; 4]> {
    let (width, height) = (old.width.max(new.width), old.height.max(new.height));
    let (cols, rows) = (width.div_ceil(TILE), height.div_ceil(TILE));
    let mut hot = vec![false; (cols * rows) as usize];
    for row in 0..rows {
        for col in 0..cols {
            let mut count = 0;
            'tile: for y in row * TILE..((row + 1) * TILE).min(height) {
                for x in col * TILE..((col + 1) * TILE).min(width) {
                    if ink(old, x, y).abs_diff(ink(new, x, y)) > INK_STEP {
                        count += 1;
                        if count >= TILE_PIXELS {
                            break 'tile;
                        }
                    }
                }
            }
            hot[(row * cols + col) as usize] = count >= TILE_PIXELS;
        }
    }
    // Squares that touch, or nearly (one square apart), are one area.
    let mut seen = vec![false; hot.len()];
    let mut areas = Vec::new();
    for start in 0..hot.len() {
        if !hot[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        let mut stack = vec![start];
        let (mut c0, mut r0, mut c1, mut r1) = (u32::MAX, u32::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            (c0, r0, c1, r1) = (c0.min(c), r0.min(r), c1.max(c), r1.max(r));
            for dr in -2i64..=2 {
                for dc in -2i64..=2 {
                    let (nc, nr) = (i64::from(c) + dc, i64::from(r) + dr);
                    if nc < 0 || nr < 0 || nc >= i64::from(cols) || nr >= i64::from(rows) {
                        continue;
                    }
                    let j = (nr * i64::from(cols) + nc) as usize;
                    if hot[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let to_points = |v: u32| (v * TILE) as f32 / scale;
        areas.push([
            to_points(c0),
            to_points(r0),
            to_points(c1 + 1).min(width as f32 / scale),
            to_points(r1 + 1).min(height as f32 / scale),
        ]);
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str, page: usize) -> Vec<Word> {
        text.split_whitespace()
            .enumerate()
            .map(|(i, w)| Word {
                text: w.into(),
                page,
                rect: [i as f32 * 10.0, 0.0, i as f32 * 10.0 + 8.0, 10.0],
            })
            .collect()
    }

    #[test]
    fn finds_changed_words_across_pages() {
        let mut old = words("the quick brown fox", 0);
        old.extend(words("jumps over the dog", 1));
        let mut new = words("the quick red fox", 0);
        new.extend(words("jumps over the lazy dog today", 1));
        let c = compare_words(old, new, 2, 2);
        let summary: Vec<(Kind, String, String)> = c
            .changes
            .iter()
            .map(|ch| {
                let (a, b) = c.texts(ch);
                (ch.kind, a, b)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (Kind::Changed, "brown".into(), "red".into()),
                (Kind::Added, String::new(), "lazy".into()),
                (Kind::Added, String::new(), "today".into()),
            ]
        );
        assert_eq!(c.pages, vec![Some(0), Some(1)]);
        assert_eq!(c.new_marks(1).len(), 2);
        assert_eq!(
            c.old_marks(0),
            vec![([20.0, 0.0, 28.0, 10.0], Kind::Changed)]
        );
        // Removed words leave a thin mark where they were.
        let c = compare_words(words("a b c", 0), words("a c", 0), 1, 1);
        assert_eq!(c.changes[0].kind, Kind::Removed);
        assert_eq!(
            c.new_place(&c.changes[0]),
            Some((0, [8.0, 0.0, 10.0, 10.0]))
        );
    }

    #[test]
    fn text_moved_to_the_next_page_is_no_change() {
        let mut old = words("one two three", 0);
        old.extend(words("four five", 1));
        let mut new = words("one two", 0);
        new.extend(words("three four five", 1));
        let c = compare_words(old, new, 2, 2);
        assert!(c.changes.is_empty());
        assert_eq!(c.pages_to_look_at(), vec![(0, 0), (1, 1)]);
    }

    fn page(w: u32, h: u32, dark: &[(u32, u32)]) -> Bitmap {
        let mut rgba = vec![255; (w * h * 4) as usize];
        for &(x, y) in dark {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 3].fill(0);
        }
        Bitmap {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn overlay_colours_each_side() {
        let old = page(3, 1, &[(0, 0), (1, 0)]);
        let new = page(3, 1, &[(1, 0), (2, 0)]);
        let o = overlay(&old, &new);
        // Only the older: red. Both: grey. Only the newer: blue.
        assert_eq!(&o.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&o.rgba[4..8], &[128, 128, 128, 255]);
        assert_eq!(&o.rgba[8..12], &[0, 0, 255, 255]);
    }

    #[test]
    fn differences_are_joined_into_areas() {
        let mut dots: Vec<(u32, u32)> = (0..4).map(|i| (10 + i, 10)).collect();
        dots.extend((0..4).map(|i| (60 + i, 70)));
        let old = page(100, 100, &[]);
        let new = page(100, 100, &dots);
        let areas = differences(&old, &new, 2.0);
        assert_eq!(areas, vec![[4.0, 4.0, 8.0, 8.0], [28.0, 32.0, 32.0, 36.0]]);
        assert!(differences(&old, &page(100, 100, &[(5, 5)]), 1.0).is_empty());
    }
}
