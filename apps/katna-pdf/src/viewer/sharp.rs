// SPDX-License-Identifier: GPL-3.0-or-later

//! The part of a page on screen drawn at the screen's own resolution.
//!
//! A page is drawn whole at most [`katna_pdf_render::MAX_SIDE`] pixels on
//! a side and [`katna_pdf_render::MAX_PIXELS`] in all, so a large drawing
//! (A1, A0) zoomed in is shown from a bitmap smaller than the screen asks
//! for, stretched, and its lines and letters look soft. While that is so,
//! the part of the page in the view (and a margin round it, for scrolling)
//! is drawn again at full resolution with [`Document::render_region`] and
//! laid over the whole page's bitmap.

use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, ImageSource, Pixels, RenderImage, WeakEntity, img, prelude::*,
};
use katna_pdf_render::recolor::{Area, PageColors};
use katna_pdf_render::{Document, MAX_SIDE, Region, fit_scale};
use katna_ui::scale::desktop_px;

use super::{Content, Viewer, render_image};

/// A part of a page drawn at full resolution.
pub(super) struct Sharp {
    /// Device pixels per point it was drawn at.
    scale: f32,
    colors: PageColors,
    /// The file as it was then.
    generation: u64,
    revision: u64,
    /// Where it goes on the page, in points from the top left: left,
    /// top, right, bottom.
    rect: [f32; 4],
    image: Arc<RenderImage>,
}

/// What a page's painting needs to know to ask for a sharper part.
pub(super) struct Wish {
    view: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    viewer: WeakEntity<Viewer>,
    page: usize,
    /// Device pixels per point wanted.
    scale: f32,
    /// Logical pixels per point.
    shown: f32,
    /// The part drawn or being drawn at this scale, if any.
    have: Option<[f32; 4]>,
}

impl Wish {
    /// Called as page `page` is painted at `bounds`: asks for the part
    /// in the view if it is not drawn sharp already.
    pub(super) fn painted(&self, bounds: Bounds<Pixels>, cx: &mut gpui::App) {
        let Some(view) = self.view.get() else {
            return;
        };
        let seen = bounds.intersect(&view);
        if seen.size.width <= desktop_px(0.0) || seen.size.height <= desktop_px(0.0) {
            return;
        }
        let unit = desktop_px(self.shown);
        let visible = [
            (seen.left() - bounds.left()) / unit,
            (seen.top() - bounds.top()) / unit,
            (seen.right() - bounds.left()) / unit,
            (seen.bottom() - bounds.top()) / unit,
        ];
        // Half a logical pixel of slack for rounding.
        let slack = 0.5 / self.shown;
        if self.have.is_some_and(|have| covers(have, visible, slack)) {
            return;
        }
        let (viewer, page, scale) = (self.viewer.clone(), self.page, self.scale);
        cx.defer(move |cx| {
            viewer
                .update(cx, |this, cx| this.sharpen(page, visible, scale, cx))
                .ok();
        });
    }
}

/// Whether `outer` holds `inner`, both left, top, right, bottom.
fn covers(outer: [f32; 4], inner: [f32; 4], slack: f32) -> bool {
    outer[0] <= inner[0] + slack
        && outer[1] <= inner[1] + slack
        && outer[2] >= inner[2] - slack
        && outer[3] >= inner[3] - slack
}

/// The part to draw for `visible` (in points) of a `w`×`h` point page at
/// `scale`: the view and a quarter of it more on each side, within the
/// page and [`MAX_SIDE`], in device pixels.
fn region_for(visible: [f32; 4], (w, h): (f32, f32), scale: f32) -> Option<Region> {
    let (vw, vh) = (visible[2] - visible[0], visible[3] - visible[1]);
    let x0 = (visible[0] - vw / 4.0).max(0.0) * scale;
    let y0 = (visible[1] - vh / 4.0).max(0.0) * scale;
    let x1 = (visible[2] + vw / 4.0).min(w) * scale;
    let y1 = (visible[3] + vh / 4.0).min(h) * scale;
    let (x, y) = (x0.floor().max(0.0), y0.floor().max(0.0));
    let width = ((x1.ceil() - x) as u32).min(MAX_SIDE);
    let height = ((y1.ceil() - y) as u32).min(MAX_SIDE);
    (width > 0 && height > 0).then_some(Region {
        x: x as u32,
        y: y as u32,
        width,
        height,
    })
}

/// Pictures found on the whole page drawn at its capped scale `whole`,
/// moved to `region` drawn at `scale`, for night pages to dim them.
fn pictures_in(pictures: &[Area], whole: f32, scale: f32, region: Region) -> Vec<Area> {
    let k = scale / whole;
    let place = |v: u32, off: u32| ((v as f32 * k) as u32).saturating_sub(off);
    pictures
        .iter()
        .map(|&(x0, y0, x1, y1)| {
            (
                place(x0, region.x),
                place(y0, region.y),
                place(x1, region.x),
                place(y1, region.y),
            )
        })
        .filter(|&(x0, y0, x1, y1)| x1 > x0 && y1 > y0)
        .collect()
}

/// `region` of page `page` at `scale` in `colors`, ready to show.
fn draw_part(
    doc: &dyn Document,
    page: usize,
    scale: f32,
    region: Region,
    colors: PageColors,
) -> Option<Arc<RenderImage>> {
    let mut bitmap = doc.render_region(page, scale, region)?;
    let pictures = if colors == PageColors::Night {
        let (w, h) = doc.page_size(page)?;
        let whole = fit_scale(w, h, scale);
        pictures_in(&doc.pictures(page, scale), whole, scale, region)
    } else {
        Vec::new()
    };
    colors.apply(&mut bitmap, &pictures);
    render_image(bitmap)
}

impl Viewer {
    /// Whether page `page` at `scale` device pixels per point is shown
    /// stretched from a smaller whole-page bitmap, so needs a sharp part.
    fn stretched(&self, page: usize, scale: f32) -> bool {
        let Content::Ready(opened) = &self.content else {
            return false;
        };
        let Some((w, h)) = opened.doc.page_size(page) else {
            return false;
        };
        fit_scale(w, h, scale) < scale * 0.99 && self.overlaid(page).is_none()
    }

    /// Page `page`'s sharp part to lay over its bitmap, and what its
    /// painting needs to ask for one, when it is shown stretched.
    pub(super) fn sharp_part(
        &self,
        page: usize,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> (Option<AnyElement>, Option<Wish>) {
        if !self.stretched(page, scale) {
            return (None, None);
        }
        let shown = self.shown;
        let colors = self.page_colors();
        let current = self.sharp.get(&page).filter(|s| {
            s.colors == colors && s.generation == self.generation && s.revision == self.revision
        });
        let close = |s: f32| (s / scale - 1.0).abs() < 0.01;
        let have = current
            .filter(|s| close(s.scale))
            .map(|s| s.rect)
            .or_else(|| {
                self.sharpening
                    .get(&page)
                    .filter(|(s, _)| close(*s))
                    .map(|(_, r)| *r)
            });
        // One drawn at another zoom is still in the right place, and
        // sharper than the stretched page, until the new one comes.
        let element = current.map(|s| {
            img(ImageSource::Render(s.image.clone()))
                .absolute()
                .left(desktop_px(s.rect[0] * shown))
                .top(desktop_px(s.rect[1] * shown))
                .w(desktop_px((s.rect[2] - s.rect[0]) * shown))
                .h(desktop_px((s.rect[3] - s.rect[1]) * shown))
                .into_any_element()
        });
        let wish = Wish {
            view: self.view_bounds.clone(),
            viewer: cx.entity().downgrade(),
            page,
            scale,
            shown,
            have,
        };
        (element, Some(wish))
    }

    /// Draws the part of page `page` round `visible` (points) at `scale`,
    /// unless one is being drawn for it already: when that comes, the
    /// page is painted again and asks once more if it still needs to.
    fn sharpen(&mut self, page: usize, visible: [f32; 4], scale: f32, cx: &mut Context<Self>) {
        if self.sharpening.contains_key(&page) || !self.stretched(page, scale) {
            return;
        }
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let Some(size) = opened.doc.page_size(page) else {
            return;
        };
        let Some(region) = region_for(visible, size, scale) else {
            return;
        };
        let rect = [
            region.x as f32 / scale,
            region.y as f32 / scale,
            (region.x + region.width) as f32 / scale,
            (region.y + region.height) as f32 / scale,
        ];
        self.sharpening.insert(page, (scale, rect));
        let doc = opened.doc.clone();
        let colors = self.page_colors();
        let (generation, revision) = (self.generation, self.revision);
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move { draw_part(doc.as_ref(), page, scale, region, colors) })
                .await;
            this.update(cx, |this, cx| {
                if this.sharpening.get(&page) == Some(&(scale, rect)) {
                    this.sharpening.remove(&page);
                }
                let Some(image) = image else {
                    return;
                };
                if this.generation != generation
                    || this.revision != revision
                    || !this.wanted().contains(&page)
                {
                    this.released.push(image);
                    cx.notify();
                    return;
                }
                let sharp = Sharp {
                    scale,
                    colors,
                    generation,
                    revision,
                    rect,
                    image,
                };
                if let Some(old) = this.sharp.insert(page, sharp) {
                    this.released.push(old.image);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Frees the sharp parts of pages not in `keep`, or all of them.
    pub(super) fn forget_sharp(&mut self, keep: Option<std::ops::Range<usize>>) {
        let gone: Vec<usize> = self
            .sharp
            .keys()
            .copied()
            .filter(|p| keep.as_ref().is_none_or(|k| !k.contains(p)))
            .collect();
        for page in gone {
            if let Some(sharp) = self.sharp.remove(&page) {
                self.released.push(sharp.image);
            }
        }
        if keep.is_none() {
            self.sharpening.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_has_a_margin_and_stays_on_the_page() {
        // An A1 sheet, 100 by 50 points of it in view at 4 pixels a point.
        let r = region_for([200.0, 300.0, 300.0, 350.0], (1684.0, 2384.0), 4.0).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (700, 1150, 600, 300));
        // At the page's corner the margin stops at its edge.
        let r = region_for([0.0, 0.0, 100.0, 100.0], (100.0, 100.0), 2.0).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 200, 200));
    }

    #[test]
    fn coverage_allows_rounding() {
        assert!(covers([0.0, 0.0, 10.0, 10.0], [0.1, 0.0, 10.1, 9.0], 0.2));
        assert!(!covers([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 11.0, 9.0], 0.2));
    }

    #[test]
    fn pictures_move_with_the_part() {
        let region = Region {
            x: 100,
            y: 50,
            width: 400,
            height: 400,
        };
        // Drawn whole at 1, the part at 2: a picture at 100..200 × 50..100
        // lands at 100..300 × 50..150 of the part.
        let moved = pictures_in(&[(100, 50, 200, 100), (0, 0, 10, 10)], 1.0, 2.0, region);
        assert_eq!(moved, vec![(100, 50, 300, 150)]);
    }
}
