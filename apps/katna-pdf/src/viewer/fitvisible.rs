// SPDX-License-Identifier: GPL-3.0-or-later

//! Fit visible (Ctrl+5): the part of the page that has something on it,
//! found from the page drawn small, fills the view's width, its left
//! edge at the left; the white margins are left out.

use gpui::Context;
use katna_pdf_render::Bitmap;

use super::{Content, POINT, STEPS, Viewer, Zoom, said};

/// Drawn at this many pixels a point to find what is on the page.
const SCALE: f32 = 0.75;

impl Viewer {
    /// Zooms so what is on the page shown fills the view's width.
    pub(super) fn fit_visible(&mut self, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let page = self.current_page();
        let doc = opened.doc.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let (w, _) = doc.page_size(page)?;
                    let bitmap = doc.render(page, SCALE)?;
                    // Drawn smaller than asked when the page is huge.
                    let scale = bitmap.width as f32 / w.max(1.0);
                    let [x0, y0, x1, y1] = content_box(&bitmap)?;
                    // Back to points, a little room kept around it.
                    let room = 4.0;
                    Some([
                        (x0 as f32 / scale - room).max(0.0),
                        (y0 as f32 / scale - room).max(0.0),
                        x1 as f32 / scale + room,
                        y1 as f32 / scale + room,
                    ])
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match found {
                    Some(rect) => this.zoom_to_width(page, rect),
                    None => {
                        this.set_zoom(Zoom::FitPage, cx);
                        this.notice = Some(said("fit-visible-blank", &[]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Zooms so `rect` (points on `page`) is as wide as the view, its
    /// top near the view's top.
    fn zoom_to_width(&mut self, page: usize, rect: [f32; 4]) {
        let Some(view) = self.view_bounds.get() else {
            return;
        };
        let (view_w, view_h) = (f32::from(view.size.width), f32::from(view.size.height));
        let width = (rect[2] - rect[0]).max(1.0);
        let zoom = (view_w * 0.97 / width / POINT).clamp(STEPS[0], STEPS[STEPS.len() - 1]);
        // As tall as the view at that size, so the top lines up.
        let tall = view_h / (zoom * POINT) * 0.9;
        self.zoom_to(page, [rect[0], rect[1], rect[2], rect[1] + tall], zoom);
    }
}

/// The box (left, top, right, bottom, in pixels, the right and bottom
/// past the last) of what is not white on `bitmap`; `None` when it is
/// all white.
fn content_box(bitmap: &Bitmap) -> Option<[u32; 4]> {
    let (w, h) = (bitmap.width, bitmap.height);
    let mut b: Option<[u32; 4]> = None;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let px = &bitmap.rgba[i..i + 3];
            if px.iter().all(|&c| c >= 245) {
                continue;
            }
            b = Some(match b {
                None => [x, y, x + 1, y + 1],
                Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
            });
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_box_holds_what_is_not_white() {
        let (w, h) = (10u32, 8u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for (x, y) in [(2u32, 3u32), (6, 5)] {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
        }
        let bitmap = Bitmap {
            width: w,
            height: h,
            rgba,
        };
        assert_eq!(content_box(&bitmap), Some([2, 3, 7, 6]));
        let blank = Bitmap {
            width: w,
            height: h,
            rgba: vec![255; (w * h * 4) as usize],
        };
        assert_eq!(content_box(&blank), None);
    }
}
