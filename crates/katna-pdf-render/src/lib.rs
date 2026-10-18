// SPDX-License-Identifier: GPL-3.0-or-later

//! PDF pages as bitmaps for Katna PDF.
//!
//! Everything that draws pages goes through [`Engine`] and [`Document`],
//! so the Phase 0 bake-off (docs/PLAN.md §3.1) can put PDFium or MuPDF
//! beside [`hayro`](crate::Hayro) without the app noticing. No GPUI types
//! here: the app turns a [`Bitmap`] into a GPUI image itself.

mod grid;
mod hayro_engine;
pub mod recolor;
pub mod scene;
pub mod text;

use std::sync::Arc;

pub use hayro_engine::{Hayro, HayroPlain};

/// A page is never drawn larger than this many pixels on a side...
pub const MAX_SIDE: u32 = 8192;
/// ...or than this many pixels in all (about 128 MB of RGBA). Deeper zoom
/// draws tiles ([`Document::render_region`]) instead of whole pages.
pub const MAX_PIXELS: u64 = 32_000_000;

/// Why a PDF cannot be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// It needs a password, or the one given is wrong.
    Locked,
    /// It is damaged, or not a PDF.
    Invalid,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OpenError::Locked => "the PDF is protected by a password",
            OpenError::Invalid => "the file is damaged or not a PDF",
        })
    }
}

impl std::error::Error for OpenError {}

/// A drawn page or part of one: RGBA, 8 bits a channel, rows top to
/// bottom, on opaque white (so premultiplied and straight alpha agree).
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bitmap({}×{})", self.width, self.height)
    }
}

/// A rectangle in pixels at some scale, from the page's top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A PDF engine: opens files into [`Document`]s.
pub trait Engine: Send + Sync {
    /// Its name, for benchmarks and logs.
    fn name(&self) -> &'static str;

    /// Opens a PDF from its bytes, with `password` when it has one.
    fn open(&self, bytes: Arc<Vec<u8>>, password: &str) -> Result<Arc<dyn Document>, OpenError>;
}

/// An open PDF. It is shared between threads; each call draws on the
/// calling thread, so several pages can be drawn at once.
pub trait Document: Send + Sync {
    /// The number of pages; at least one.
    fn page_count(&self) -> usize;

    /// The size of page `page` in points (1/72 inch), its rotation
    /// applied. `None` past the last page.
    fn page_size(&self, page: usize) -> Option<(f32, f32)>;

    /// Draws page `page` whole at `scale` pixels per point, made smaller
    /// if needed to stay within [`MAX_SIDE`] and [`MAX_PIXELS`].
    fn render(&self, page: usize, scale: f32) -> Option<Bitmap>;

    /// Draws `region` of page `page` as it would be at `scale` pixels per
    /// point: a tile, for zoom levels too deep to draw whole pages.
    fn render_region(&self, page: usize, scale: f32, region: Region) -> Option<Bitmap>;

    /// Where page `page`'s pictures (raster images) fall when it is drawn
    /// whole by [`Document::render`] at `scale`, in pixels: night pages
    /// dim them instead of flipping them ([`recolor::darken`]).
    fn pictures(&self, _page: usize, _scale: f32) -> Vec<recolor::Area> {
        Vec::new()
    }

    /// Page `page`'s text and where each character is, in points from
    /// the page's top left.
    fn text(&self, _page: usize) -> text::PageText {
        text::PageText::default()
    }
}

/// `scale`, made smaller if a `w`×`h` point page drawn at it would pass
/// [`MAX_SIDE`] or [`MAX_PIXELS`].
pub fn fit_scale(w: f32, h: f32, scale: f32) -> f32 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let side = MAX_SIDE as f32 / w.max(h);
    let area = (MAX_PIXELS as f32 / (w * h)).sqrt();
    scale.min(side).min(area).max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_scale_keeps_small_pages() {
        assert_eq!(fit_scale(612.0, 792.0, 2.0), 2.0);
    }

    #[test]
    fn fit_scale_caps_the_longer_side() {
        // An A0 drawing (2384 × 3370 pt) at 4×: the side limit wins.
        let s = fit_scale(2384.0, 3370.0, 4.0);
        assert!(3370.0 * s <= MAX_SIDE as f32 + 0.5);
    }

    #[test]
    fn fit_scale_caps_the_area() {
        let s = fit_scale(8000.0, 8000.0, 1.0);
        assert!((8000.0 * s) * (8000.0 * s) <= MAX_PIXELS as f32 * 1.001);
    }
}
