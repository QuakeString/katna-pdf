// SPDX-License-Identifier: GPL-3.0-or-later

//! Night pages (docs/PLAN.md §4.5, "Katna dark page"): a drawn page shown
//! dark for reading at night. Lightness is flipped and hue kept, so white
//! paper turns a soft dark grey, black ink a soft light grey, and a red
//! heading stays red. Photos are dimmed, not flipped, since a flipped
//! photo looks like a negative; a scan (a picture that is mostly plain
//! paper with dark marks) is flipped like text.
//!
//! This is Katna Mail's algorithm (`katna-preview`'s `dark` module), so a
//! PDF looks the same in Mail's attachment viewer and here; the numbers
//! must stay the same as there.

use crate::Bitmap;

/// How much photos are dimmed to.
const PHOTO: f32 = 0.72;
/// A picture is a scan when at least this share of it is plain paper.
const SCAN_PAPER: f32 = 0.5;

/// A picture on a page, in pixels: left, top, right, bottom.
pub type Area = (u32, u32, u32, u32);

/// One colour channel flipped: lightness turned over with hue and
/// saturation kept (invert, then the complement), squeezed between 10 %
/// and 90 % so the page is never pure black nor the ink pure white.
fn channel(c: u8, max: u8, min: u8) -> u8 {
    let flipped = 255.0 + f32::from(c) - f32::from(max) - f32::from(min);
    (25.5 + flipped * 0.8).round() as u8
}

/// `[r, g, b]` flipped for a dark page.
pub fn flip(rgb: [u8; 3]) -> [u8; 3] {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    rgb.map(|c| channel(c, max, min))
}

/// Whether a pixel looks like plain paper: light and nearly grey.
fn paper(rgb: [u8; 3]) -> bool {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    max >= 200 && max - min <= 40
}

/// `bitmap` (a drawn page, RGBA) made dark: flipped everywhere except
/// inside `pictures` that are photos, which are dimmed.
pub fn darken(bitmap: &mut Bitmap, pictures: &[Area]) {
    let (w, h) = (bitmap.width, bitmap.height);
    let photos: Vec<Area> = pictures
        .iter()
        .map(|&(x0, y0, x1, y1)| (x0.min(w), y0.min(h), x1.min(w), y1.min(h)))
        .filter(|&(x0, y0, x1, y1)| x1 > x0 && y1 > y0 && !scan(bitmap, (x0, y0, x1, y1)))
        .collect();
    // Flipping is per colour; a page holds few, so remember the last.
    let mut last = ([255, 255, 255], flip([255, 255, 255]));
    for (i, px) in bitmap.rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = ((i % w as usize) as u32, (i / w as usize) as u32);
        let rgb = [px[0], px[1], px[2]];
        let photo = photos
            .iter()
            .any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1);
        let [r, g, b] = if photo {
            rgb.map(|c| (f32::from(c) * PHOTO).round() as u8)
        } else {
            if rgb != last.0 {
                last = (rgb, flip(rgb));
            }
            last.1
        };
        px[0] = r;
        px[1] = g;
        px[2] = b;
    }
}

/// Whether the part of `bitmap` in `area` is mostly plain paper, looked
/// at on a sparse grid.
fn scan(bitmap: &Bitmap, (x0, y0, x1, y1): Area) -> bool {
    let step = ((x1 - x0).max(y1 - y0) / 64).max(1);
    let (mut all, mut light) = (0u32, 0u32);
    for y in (y0..y1).step_by(step as usize) {
        for x in (x0..x1).step_by(step as usize) {
            let i = (y as usize * bitmap.width as usize + x as usize) * 4;
            all += 1;
            if paper([bitmap.rgba[i], bitmap.rgba[i + 1], bitmap.rgba[i + 2]]) {
                light += 1;
            }
        }
    }
    all > 0 && light as f32 >= all as f32 * SCAN_PAPER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_and_ink_swap_softly_and_hue_stays() {
        assert_eq!(flip([255, 255, 255]), [26, 26, 26]);
        assert_eq!(flip([0, 0, 0]), [230, 230, 230]);
        // A red heading stays red.
        let [r, g, b] = flip([200, 30, 30]);
        assert!(r > g && g == b);
    }

    #[test]
    fn photos_are_dimmed_and_scans_flipped() {
        // 4×2: the left half a photo (mid grey-blue), the right half a
        // scan (white paper).
        let mut rgba = Vec::new();
        for _ in 0..2 {
            rgba.extend_from_slice(&[40, 80, 160, 255, 40, 80, 160, 255]);
            rgba.extend_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
        }
        let mut bitmap = Bitmap {
            width: 4,
            height: 2,
            rgba,
        };
        darken(&mut bitmap, &[(0, 0, 2, 2), (2, 0, 4, 2)]);
        assert_eq!(&bitmap.rgba[0..4], &[29, 58, 115, 255]);
        assert_eq!(&bitmap.rgba[8..12], &[26, 26, 26, 255]);
    }
}
