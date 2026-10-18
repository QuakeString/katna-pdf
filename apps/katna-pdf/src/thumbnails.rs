// SPDX-License-Identifier: GPL-3.0-or-later

//! The pages tab of the side pane: a small picture of every page, drawn
//! as it scrolls into view, one at a time so the pages being read come
//! first (docs/PLAN.md, Phase 1).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{ListAlignment, ListState, RenderImage};
use katna_ui::ScrollBar;
use katna_ui::scale::desktop_px;

/// A thumbnail's width, in logical pixels.
pub const WIDTH: f32 = 120.0;
/// Thumbnails kept drawn; the ones farthest from the last asked for are
/// freed beyond this.
const KEEP: usize = 60;
/// Thumbnails waiting to be drawn beyond this many are dropped, oldest
/// first.
const QUEUE: usize = 24;

pub struct Thumbs {
    pub list: ListState,
    pub bar: ScrollBar,
    images: HashMap<usize, Arc<RenderImage>>,
    /// The page being drawn.
    pub drawing: Option<usize>,
    /// Pages waiting, newest last.
    queue: Vec<usize>,
    /// Pages that could not be drawn, not asked for again.
    failed: HashSet<usize>,
    /// The page last asked for: what the reader looks at.
    near: usize,
}

impl Thumbs {
    pub fn new() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, desktop_px(400.0)),
            bar: ScrollBar::default(),
            images: HashMap::new(),
            drawing: None,
            queue: Vec::new(),
            failed: HashSet::new(),
            near: 0,
        }
    }

    /// A new document of `pages` pages; the old thumbnails are returned
    /// to be freed.
    pub fn reset(&mut self, pages: usize) -> Vec<Arc<RenderImage>> {
        self.list.reset(pages);
        self.drawing = None;
        self.queue.clear();
        self.failed.clear();
        self.near = 0;
        self.images.drain().map(|(_, image)| image).collect()
    }

    pub fn image(&self, page: usize) -> Option<Arc<RenderImage>> {
        self.images.get(&page).cloned()
    }

    /// Page `page` is on screen: draw it if it is not drawn or waiting.
    pub fn want(&mut self, page: usize) {
        self.near = page;
        if self.images.contains_key(&page)
            || self.drawing == Some(page)
            || self.failed.contains(&page)
        {
            return;
        }
        self.queue.retain(|p| *p != page);
        self.queue.push(page);
        if self.queue.len() > QUEUE {
            self.queue.remove(0);
        }
    }

    /// The next page to draw, if none is being drawn.
    pub fn next(&mut self) -> Option<usize> {
        if self.drawing.is_some() {
            return None;
        }
        self.drawing = self.queue.pop();
        self.drawing
    }

    /// Page `page` is drawn. Returns the thumbnails to free: the farthest
    /// from where the reader is, beyond [`KEEP`].
    pub fn drawn(&mut self, page: usize, image: Arc<RenderImage>) -> Vec<Arc<RenderImage>> {
        self.drawing = None;
        let mut freed: Vec<_> = self.images.insert(page, image).into_iter().collect();
        while self.images.len() > KEEP {
            let near = self.near;
            let Some(&far) = self.images.keys().max_by_key(|p| p.abs_diff(near)) else {
                break;
            };
            freed.extend(self.images.remove(&far));
        }
        freed
    }

    /// Page `page` changed: its thumbnail is drawn again when next
    /// asked for. Returns the old one to free.
    pub fn forget(&mut self, page: usize) -> Option<Arc<RenderImage>> {
        self.failed.remove(&page);
        self.images.remove(&page)
    }

    /// The page being drawn was drawn from a file since changed: it is
    /// asked for again when next on screen.
    pub fn dropped(&mut self) {
        self.drawing = None;
    }

    /// Drawing `page` failed.
    pub fn failed(&mut self, page: usize) {
        self.drawing = None;
        self.failed.insert(page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_request_is_drawn_first_and_failures_are_not_retried() {
        let mut thumbs = Thumbs::new();
        thumbs.want(0);
        thumbs.want(1);
        thumbs.want(0);
        assert_eq!(thumbs.next(), Some(0));
        // One at a time.
        assert_eq!(thumbs.next(), None);
        thumbs.failed(0);
        thumbs.want(0);
        assert_eq!(thumbs.next(), Some(1));
        thumbs.failed(1);
        assert_eq!(thumbs.next(), None);
    }
}
