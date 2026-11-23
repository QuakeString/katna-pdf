// SPDX-License-Identifier: GPL-3.0-or-later

//! A document seen turned a quarter, half or three quarters clockwise,
//! without changing the file (Rotate view). Everything a [`Document`]
//! says about its pages turns together: sizes, drawings, tiles, pictures,
//! text boxes, line pieces and the page transform, so what is laid over
//! the pages through them turns with the pages.

use std::sync::Arc;

use crate::{Bitmap, Document, Region, fit_scale, measure, recolor, text};

/// `inner` turned `quarters` quarter turns clockwise.
pub struct Turned {
    inner: Arc<dyn Document>,
    quarters: u8,
}

impl Turned {
    /// `inner` turned `quarters` quarter turns clockwise; `inner` itself
    /// when that is no turn at all.
    pub fn wrap(inner: Arc<dyn Document>, quarters: u8) -> Arc<dyn Document> {
        match quarters % 4 {
            0 => inner,
            quarters => Arc::new(Turned { inner, quarters }),
        }
    }

    /// A point on the page as it is, `w`×`h`, as it is shown turned.
    fn point(&self, (w, h): (f32, f32), [x, y]: [f32; 2]) -> [f32; 2] {
        match self.quarters {
            1 => [h - y, x],
            2 => [w - x, h - y],
            _ => [y, w - x],
        }
    }

    /// A box (left, top, right, bottom) on the page as it is, as shown.
    fn rect(&self, size: (f32, f32), r: [f32; 4]) -> [f32; 4] {
        let [x0, y0] = self.point(size, [r[0], r[1]]);
        let [x1, y1] = self.point(size, [r[2], r[3]]);
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    }

    /// The map from the page as it is to the page shown, in points.
    fn matrix(&self, (w, h): (f64, f64)) -> [f64; 6] {
        match self.quarters {
            1 => [0.0, 1.0, -1.0, 0.0, h, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0, w, h],
            _ => [0.0, -1.0, 1.0, 0.0, 0.0, w],
        }
    }

    fn sideways(&self) -> bool {
        self.quarters % 2 == 1
    }
}

/// `m` after `n`: first `n`, then `m`.
fn then(m: [f64; 6], n: [f64; 6]) -> [f64; 6] {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

/// `bitmap` turned `quarters` quarter turns clockwise.
pub fn turn(bitmap: &Bitmap, quarters: u8) -> Bitmap {
    let (w, h) = (bitmap.width as usize, bitmap.height as usize);
    let quarters = quarters % 4;
    if quarters == 0 {
        return bitmap.clone();
    }
    let (tw, th) = if quarters % 2 == 1 { (h, w) } else { (w, h) };
    let mut rgba = vec![0u8; bitmap.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = match quarters {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let from = (y * w + x) * 4;
            let to = (ty * tw + tx) * 4;
            rgba[to..to + 4].copy_from_slice(&bitmap.rgba[from..from + 4]);
        }
    }
    Bitmap {
        width: tw as u32,
        height: th as u32,
        rgba,
    }
}

impl Document for Turned {
    fn page_count(&self) -> usize {
        self.inner.page_count()
    }

    fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        let (w, h) = self.inner.page_size(page)?;
        Some(if self.sideways() { (h, w) } else { (w, h) })
    }

    fn render(&self, page: usize, scale: f32) -> Option<Bitmap> {
        Some(turn(&self.inner.render(page, scale)?, self.quarters))
    }

    fn render_region(&self, page: usize, scale: f32, region: Region) -> Option<Bitmap> {
        let (w, h) = self.inner.page_size(page)?;
        // The page as it is, in pixels at `scale`.
        let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
        let Region {
            x,
            y,
            width,
            height,
        } = region;
        let inner = match self.quarters {
            1 => Region {
                x: y,
                y: ph.saturating_sub(x + width),
                width: height,
                height: width,
            },
            2 => Region {
                x: pw.saturating_sub(x + width),
                y: ph.saturating_sub(y + height),
                width,
                height,
            },
            _ => Region {
                x: pw.saturating_sub(y + height),
                y: x,
                width: height,
                height: width,
            },
        };
        Some(turn(
            &self.inner.render_region(page, scale, inner)?,
            self.quarters,
        ))
    }

    fn pictures(&self, page: usize, scale: f32) -> Vec<recolor::Area> {
        let Some((w, h)) = self.inner.page_size(page) else {
            return Vec::new();
        };
        // In pixels of the page as `render` draws it.
        let s = fit_scale(w, h, scale);
        let size = ((w * s).round(), (h * s).round());
        self.inner
            .pictures(page, scale)
            .into_iter()
            .map(|(x0, y0, x1, y1)| {
                let r = self.rect(size, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
                (r[0] as u32, r[1] as u32, r[2] as u32, r[3] as u32)
            })
            .collect()
    }

    fn text(&self, page: usize) -> text::PageText {
        let mut text = self.inner.text(page);
        if let Some(size) = self.inner.page_size(page) {
            for b in &mut text.boxes {
                *b = self.rect(size, *b);
            }
        }
        text
    }

    fn segments(&self, page: usize) -> Vec<measure::Segment> {
        let Some(size) = self.inner.page_size(page) else {
            return Vec::new();
        };
        self.inner
            .segments(page)
            .into_iter()
            .map(|[x0, y0, x1, y1]| {
                let [a, b] = self.point(size, [x0, y0]);
                let [c, d] = self.point(size, [x1, y1]);
                [a, b, c, d]
            })
            .collect()
    }

    fn page_transform(&self, page: usize) -> Option<[f64; 6]> {
        let (w, h) = self.inner.page_size(page)?;
        let inner = self.inner.page_transform(page)?;
        Some(then(self.matrix((f64::from(w), f64::from(h))), inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_point;

    /// A 4×2 point page: its text is one letter at the top left.
    struct Page;

    impl Document for Page {
        fn page_count(&self) -> usize {
            1
        }
        fn page_size(&self, _: usize) -> Option<(f32, f32)> {
            Some((4.0, 2.0))
        }
        fn render(&self, _: usize, scale: f32) -> Option<Bitmap> {
            let (w, h) = ((4.0 * scale) as u32, (2.0 * scale) as u32);
            // Each pixel's red is its index, to follow it.
            let rgba = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
            Some(Bitmap {
                width: w,
                height: h,
                rgba,
            })
        }
        fn render_region(&self, page: usize, scale: f32, r: Region) -> Option<Bitmap> {
            let whole = self.render(page, scale)?;
            let mut rgba = Vec::new();
            for y in r.y..r.y + r.height {
                let from = ((y * whole.width + r.x) * 4) as usize;
                rgba.extend_from_slice(&whole.rgba[from..from + (r.width * 4) as usize]);
            }
            Some(Bitmap {
                width: r.width,
                height: r.height,
                rgba,
            })
        }
        fn text(&self, _: usize) -> text::PageText {
            text::PageText {
                text: "A".into(),
                boxes: vec![[0.0, 0.0, 1.0, 1.0]],
            }
        }
        fn page_transform(&self, _: usize) -> Option<[f64; 6]> {
            // From the bottom left, as PDF counts.
            Some([1.0, 0.0, 0.0, -1.0, 0.0, 2.0])
        }
    }

    #[test]
    fn a_quarter_turn_turns_everything_together() {
        let doc = Turned::wrap(Arc::new(Page), 1);
        assert_eq!(doc.page_size(0), Some((2.0, 4.0)));
        // The letter at the top left goes to the top right.
        assert_eq!(doc.text(0).boxes, [[1.0, 0.0, 2.0, 1.0]]);
        // The page transform agrees: PDF's top left corner (0, 2) is
        // shown at the top right.
        let t = doc.page_transform(0).unwrap();
        assert_eq!(map_point(t, (0.0, 2.0)), (2.0, 0.0));
        let bitmap = doc.render(0, 1.0).unwrap();
        assert_eq!((bitmap.width, bitmap.height), (2, 4));
        // The page's top left pixel (red 0) is now top right.
        assert_eq!(bitmap.rgba[4], 0);
    }

    #[test]
    fn tiles_match_the_whole_page_turned() {
        for quarters in 1..4 {
            let doc = Turned::wrap(Arc::new(Page), quarters);
            let whole = doc.render(0, 2.0).unwrap();
            let region = Region {
                x: 1,
                y: 1,
                width: 2,
                height: 3,
            };
            let Some(tile) = doc.render_region(0, 2.0, region) else {
                continue;
            };
            if region.x + region.width > whole.width || region.y + region.height > whole.height {
                continue;
            }
            for y in 0..region.height {
                for x in 0..region.width {
                    let t = ((y * tile.width + x) * 4) as usize;
                    let w = (((y + region.y) * whole.width + x + region.x) * 4) as usize;
                    assert_eq!(
                        tile.rgba[t], whole.rgba[w],
                        "quarters {quarters} at {x},{y}"
                    );
                }
            }
        }
    }

    #[test]
    fn four_quarters_are_no_turn() {
        let page: Arc<dyn Document> = Arc::new(Page);
        let doc = Turned::wrap(page, 4);
        assert_eq!(doc.page_size(0), Some((4.0, 2.0)));
    }
}
