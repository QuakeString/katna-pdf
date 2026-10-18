// SPDX-License-Identifier: GPL-3.0-or-later

//! How pages are laid out (docs/PLAN.md, Phase 1, page display modes):
//! one under another, or two side by side like an open book, with or
//! without the first page alone as a cover. The page list holds rows;
//! this maps between rows and pages.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spread {
    /// One page a row.
    #[default]
    Single,
    /// Two pages a row: 1–2, 3–4, …
    Facing,
    /// The first page alone, then two a row: 2–3, 4–5, … as a book
    /// opens.
    Book,
}

impl Spread {
    /// The next layout, for the toolbar button that goes round them.
    pub fn next(self) -> Self {
        match self {
            Self::Single => Self::Facing,
            Self::Facing => Self::Book,
            Self::Book => Self::Single,
        }
    }

    /// Pages side by side in a full row.
    pub fn columns(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Facing | Self::Book => 2,
        }
    }

    /// Rows for `pages` pages.
    pub fn rows(self, pages: usize) -> usize {
        match self {
            Self::Single => pages,
            Self::Facing => pages.div_ceil(2),
            Self::Book => (pages + 1).div_ceil(2).min(pages),
        }
    }

    /// The row page `page` is on.
    pub fn row_of(self, page: usize) -> usize {
        match self {
            Self::Single => page,
            Self::Facing => page / 2,
            Self::Book => page.div_ceil(2),
        }
    }

    /// The pages on row `row` of a document of `pages` pages.
    pub fn pages_in(self, row: usize, pages: usize) -> Range<usize> {
        let range = match self {
            Self::Single => row..row + 1,
            Self::Facing => row * 2..row * 2 + 2,
            Self::Book if row == 0 => 0..1,
            Self::Book => row * 2 - 1..row * 2 + 1,
        };
        range.start.min(pages)..range.end.min(pages)
    }

    /// The pages on rows `rows`.
    pub fn pages_on(self, rows: Range<usize>, pages: usize) -> Range<usize> {
        if rows.is_empty() {
            let start = self.pages_in(rows.start, pages).start;
            return start..start;
        }
        self.pages_in(rows.start, pages).start..self.pages_in(rows.end - 1, pages).end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_pages_map_both_ways() {
        // Five pages.
        assert_eq!(Spread::Single.rows(5), 5);
        assert_eq!(Spread::Facing.rows(5), 3);
        assert_eq!(Spread::Book.rows(5), 3);
        assert_eq!(Spread::Book.rows(4), 3);
        assert_eq!(Spread::Book.rows(1), 1);
        assert_eq!(Spread::Book.rows(0), 0);
        assert_eq!(Spread::Facing.pages_in(2, 5), 4..5);
        assert_eq!(Spread::Book.pages_in(0, 5), 0..1);
        assert_eq!(Spread::Book.pages_in(2, 5), 3..5);
        assert_eq!(Spread::Book.pages_in(2, 4), 3..4);
        for spread in [Spread::Single, Spread::Facing, Spread::Book] {
            for n in 0..6 {
                for page in 0..n {
                    let row = spread.row_of(page);
                    assert!(row < spread.rows(n));
                    assert!(
                        spread.pages_in(row, n).contains(&page),
                        "{spread:?} {n} {page}"
                    );
                }
            }
        }
        assert_eq!(Spread::Facing.pages_on(1..3, 5), 2..5);
        assert_eq!(Spread::Single.next().next().next(), Spread::Single);
    }
}
