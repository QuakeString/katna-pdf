// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing a document's pages from the pages pane (docs/PLAN.md,
//! Phase 2, page operations): turning, deleting and moving the pages
//! picked there. Each change gives the new order of the pages, which
//! `katna_pdf_cos::compose` writes as a new file.

use std::collections::BTreeSet;

/// A change to the pages picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOp {
    /// Turn clockwise by this many quarter turns.
    Turn(i32),
    Delete,
    /// Move one place towards the start, or the end.
    Earlier,
    Later,
}

/// The new pages, in order: each one's old page and the quarter turns
/// added.
pub type Order = Vec<(usize, i32)>;

/// The pages of a document of `pages` pages after `op` on the pages
/// `picked`, and the new places of the pages picked. `None` when `op`
/// would change nothing, or would leave no pages.
pub fn apply(
    op: PageOp,
    pages: usize,
    picked: &BTreeSet<usize>,
) -> Option<(Order, BTreeSet<usize>)> {
    let picked: BTreeSet<usize> = picked.iter().copied().filter(|&p| p < pages).collect();
    if picked.is_empty() {
        return None;
    }
    let mut order: Vec<(usize, i32)> = (0..pages).map(|p| (p, 0)).collect();
    match op {
        PageOp::Turn(turns) => {
            for &p in &picked {
                order[p].1 = turns;
            }
            Some((order, picked))
        }
        PageOp::Delete => {
            if picked.len() == pages {
                return None;
            }
            order.retain(|(p, _)| !picked.contains(p));
            // The page after the last deleted, or the new last.
            let next = picked
                .iter()
                .next_back()
                .map_or(0, |&last| last + 1 - picked.len());
            let next = next.min(order.len() - 1);
            Some((order, BTreeSet::from([next])))
        }
        PageOp::Earlier => {
            if picked.first() == Some(&0) {
                return None;
            }
            for &p in &picked {
                order.swap(p - 1, p);
            }
            Some((order, picked.iter().map(|p| p - 1).collect()))
        }
        PageOp::Later => {
            if picked.last() == Some(&(pages - 1)) {
                return None;
            }
            for &p in picked.iter().rev() {
                order.swap(p, p + 1);
            }
            Some((order, picked.iter().map(|p| p + 1).collect()))
        }
    }
}

/// Where pages inserted go: after the last page picked, or at the end.
pub fn insert_at(pages: usize, picked: &BTreeSet<usize>) -> usize {
    picked
        .iter()
        .next_back()
        .map_or(pages, |&last| (last + 1).min(pages))
}

/// The pages picked after a click on page `page`: alone, or added or
/// taken away with Ctrl, or the run from `anchor` with Shift.
pub fn click(
    picked: &BTreeSet<usize>,
    anchor: Option<usize>,
    page: usize,
    ctrl: bool,
    shift: bool,
) -> BTreeSet<usize> {
    match (anchor, shift, ctrl) {
        (Some(anchor), true, _) => (anchor.min(page)..=anchor.max(page)).collect(),
        (_, _, true) => {
            let mut picked = picked.clone();
            if !picked.remove(&page) {
                picked.insert(page);
            }
            picked
        }
        _ => BTreeSet::from([page]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pages: &[usize]) -> BTreeSet<usize> {
        pages.iter().copied().collect()
    }

    fn order(op: PageOp, pages: usize, picked: &[usize]) -> Option<(Vec<usize>, Vec<usize>)> {
        apply(op, pages, &set(picked)).map(|(order, picked)| {
            (
                order.into_iter().map(|(p, _)| p).collect(),
                picked.into_iter().collect(),
            )
        })
    }

    #[test]
    fn pages_move_turn_and_go() {
        assert_eq!(
            order(PageOp::Earlier, 5, &[1, 3]),
            Some((vec![1, 0, 3, 2, 4], vec![0, 2]))
        );
        assert_eq!(
            order(PageOp::Later, 5, &[2, 3]),
            Some((vec![0, 1, 4, 2, 3], vec![3, 4]))
        );
        assert_eq!(order(PageOp::Earlier, 5, &[0, 2]), None);
        assert_eq!(order(PageOp::Later, 5, &[4]), None);
        assert_eq!(
            order(PageOp::Delete, 5, &[1, 2]),
            Some((vec![0, 3, 4], vec![1]))
        );
        assert_eq!(order(PageOp::Delete, 3, &[2]), Some((vec![0, 1], vec![1])));
        assert_eq!(order(PageOp::Delete, 2, &[0, 1]), None);
        let (turned, _) = apply(PageOp::Turn(-1), 3, &set(&[1])).unwrap();
        assert_eq!(turned, vec![(0, 0), (1, -1), (2, 0)]);
        assert_eq!(apply(PageOp::Turn(1), 3, &set(&[7])), None);
    }

    #[test]
    fn clicks_pick_pages() {
        let none = BTreeSet::new();
        assert_eq!(click(&none, None, 3, false, false), set(&[3]));
        assert_eq!(
            click(&set(&[3]), Some(3), 6, false, true),
            set(&[3, 4, 5, 6])
        );
        assert_eq!(click(&set(&[3, 5]), Some(3), 5, true, false), set(&[3]));
        assert_eq!(click(&set(&[3]), Some(3), 1, true, false), set(&[1, 3]));
        assert_eq!(insert_at(4, &set(&[1, 2])), 3);
        assert_eq!(insert_at(4, &none), 4);
    }
}
