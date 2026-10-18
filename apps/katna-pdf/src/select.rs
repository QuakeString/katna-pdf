// SPDX-License-Identifier: GPL-3.0-or-later

//! Selecting text on the pages with the pointer, and copying it
//! (docs/PLAN.md, Phase 1). A selection runs in the text's own order from
//! the character pressed on to the one the pointer is over, across pages
//! when it is dragged past one.

use std::ops::Range;

use katna_pdf_render::text::PageText;

/// A place in the text: a page and a character on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Spot {
    pub page: usize,
    pub index: usize,
}

/// A selection: where the press was and where the pointer is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Selection {
    pub anchor: Spot,
    pub head: Spot,
}

impl Selection {
    /// The characters selected on `page` of `count` characters.
    pub fn on_page(&self, page: usize, count: usize) -> Option<Range<usize>> {
        let (from, to) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        if page < from.page || page > to.page {
            return None;
        }
        let start = if page == from.page { from.index } else { 0 };
        let end = if page == to.page {
            (to.index + 1).min(count)
        } else {
            count
        };
        (start < end).then_some(start..end)
    }

    pub fn pages(&self) -> Range<usize> {
        self.anchor.page.min(self.head.page)..self.anchor.page.max(self.head.page) + 1
    }
}

/// The character at `point` (in points from the page's top left): the
/// one whose box holds it, otherwise the nearest on the nearest line.
pub fn char_at(text: &PageText, point: (f32, f32)) -> Option<usize> {
    let (x, y) = point;
    let mut best: Option<(f32, usize)> = None;
    for (i, b) in text.boxes.iter().enumerate() {
        if b[2] <= b[0] || b[3] <= b[1] {
            continue;
        }
        let dx = (b[0] - x).max(x - b[2]).max(0.0);
        let dy = (b[1] - y).max(y - b[3]).max(0.0);
        if dx == 0.0 && dy == 0.0 {
            return Some(i);
        }
        // Being off the line counts for more than being along it.
        let d = dx + dy * 4.0;
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// The text of `range` on a page.
pub fn text_of(text: &PageText, range: Range<usize>) -> String {
    text.text
        .chars()
        .skip(range.start)
        .take(range.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(text: &str) -> PageText {
        // One line per line of `text`, each character 10 × 10.
        let mut boxes = Vec::new();
        let (mut x, mut y) = (0.0, 0.0);
        for c in text.chars() {
            if c == '\n' {
                boxes.push([x, y, x, y]);
                (x, y) = (0.0, y + 10.0);
            } else {
                boxes.push([x, y, x + 10.0, y + 10.0]);
                x += 10.0;
            }
        }
        PageText {
            text: text.into(),
            boxes,
        }
    }

    #[test]
    fn the_character_under_or_nearest_the_pointer() {
        let text = page("ab\ncde");
        assert_eq!(char_at(&text, (15.0, 5.0)), Some(1));
        assert_eq!(char_at(&text, (25.0, 15.0)), Some(5));
        // Past the end of the short first line: its last character.
        assert_eq!(char_at(&text, (28.0, 4.0)), Some(1));
        assert_eq!(char_at(&PageText::default(), (0.0, 0.0)), None);
    }

    #[test]
    fn a_selection_dragged_backwards_and_over_pages() {
        let spot = |page, index| Spot { page, index };
        let selection = Selection {
            anchor: spot(2, 3),
            head: spot(1, 4),
        };
        assert_eq!(selection.on_page(0, 10), None);
        assert_eq!(selection.on_page(1, 10), Some(4..10));
        assert_eq!(selection.on_page(2, 10), Some(0..4));
        assert_eq!(selection.pages(), 1..3);
        assert_eq!(text_of(&page("hello world"), 6..11), "world");
    }
}
