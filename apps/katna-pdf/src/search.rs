// SPDX-License-Identifier: GPL-3.0-or-later

//! Find in the document (docs/PLAN.md, Phase 1): pages are read for
//! their text on a background thread, starting at the page being read and
//! wrapping round, and each page's matches come back as soon as it is
//! read, so the first ones show while the rest of a long file is searched.
//! Each page's text is read once and kept for later searches.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use katna_pdf_render::Document;
use katna_pdf_render::text::PageText;

/// One match: its page and its boxes there (one per line it spans), in
/// points from the page's top left.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub page: usize,
    pub boxes: Vec<[f32; 4]>,
}

/// Each page's text, read on first need.
pub type Texts = Arc<Vec<OnceLock<PageText>>>;

pub fn texts(pages: usize) -> Texts {
    Arc::new((0..pages).map(|_| OnceLock::new()).collect())
}

/// What the search has found so far.
#[derive(Debug, Default)]
pub struct Found {
    /// The matches in page order.
    pub hits: Vec<Hit>,
    /// The match shown, an index into `hits`.
    pub current: Option<usize>,
    /// Pages searched, of all of them.
    pub searched: usize,
    pub pages: usize,
}

impl Found {
    pub fn done(&self) -> bool {
        self.searched >= self.pages
    }

    /// Adds page `page`'s matches, keeping page order. The first match
    /// found becomes the current one.
    pub fn add(&mut self, page: usize, hits: Vec<Hit>) {
        self.searched += 1;
        if hits.is_empty() {
            return;
        }
        let at = self.hits.partition_point(|h| h.page < page);
        let current = self.current.map(|c| &self.hits[c].page).copied();
        let n = hits.len();
        self.hits.splice(at..at, hits);
        self.current = match (self.current, current) {
            (None, _) => Some(at),
            // Matches before the current one move it along.
            (Some(c), Some(p)) if page < p => Some(c + n),
            (c, _) => c,
        };
    }

    /// Moves to the next match (or the one before), wrapping round.
    pub fn step(&mut self, forward: bool) -> Option<&Hit> {
        let n = self.hits.len();
        if n == 0 {
            return None;
        }
        let c = self.current.unwrap_or(0);
        let next = if forward {
            (c + 1) % n
        } else {
            (c + n - 1) % n
        };
        self.current = Some(next);
        self.hits.get(next)
    }

    pub fn current(&self) -> Option<&Hit> {
        self.hits.get(self.current?)
    }
}

/// Searches for `query` from page `from` on, calling `found` with each
/// page's matches, until done or `generation` moves on from `mine`.
pub fn run(
    doc: &dyn Document,
    texts: &Texts,
    query: &str,
    from: usize,
    generation: &AtomicU64,
    mine: u64,
    mut found: impl FnMut(usize, Vec<Hit>) -> bool,
) {
    let n = texts.len();
    for k in 0..n {
        if generation.load(Ordering::Relaxed) != mine {
            return;
        }
        let page = (from + k) % n;
        let text = texts[page].get_or_init(|| doc.text(page));
        let hits = text
            .find(query)
            .into_iter()
            .map(|range| Hit {
                page,
                boxes: text.line_boxes(range),
            })
            .filter(|hit| !hit.boxes.is_empty())
            .collect();
        if !found(page, hits) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(page: usize) -> Hit {
        Hit {
            page,
            boxes: vec![[0.0, 0.0, 1.0, 1.0]],
        }
    }

    #[test]
    fn matches_keep_page_order_as_pages_come_in_any_order() {
        let mut found = Found {
            pages: 4,
            ..Found::default()
        };
        // The search starts at page 2, the one being read.
        found.add(2, vec![hit(2)]);
        assert_eq!(found.current().map(|h| h.page), Some(2));
        found.add(3, vec![]);
        found.add(0, vec![hit(0), hit(0)]);
        // Still on the first match found, now third in page order.
        assert_eq!(found.current, Some(2));
        assert!(!found.done());
        found.add(1, vec![hit(1)]);
        assert!(found.done());
        let pages: Vec<_> = found.hits.iter().map(|h| h.page).collect();
        assert_eq!(pages, [0, 0, 1, 2]);
        assert_eq!(found.current().map(|h| h.page), Some(2));
        assert_eq!(found.step(true).map(|h| h.page), Some(0));
        assert_eq!(found.step(false).map(|h| h.page), Some(2));
    }
}
