// SPDX-License-Identifier: GPL-3.0-or-later

//! Headers, footers, watermarks and Bates numbers (docs/PLAN.md §4.3):
//! a line or a few of text drawn on many pages at once, into each page's
//! own content as [`Document::insert`] draws, so every reader shows and
//! prints them. Places are taken as the page is shown (its crop box,
//! turned by `/Rotate`), so a footer sits at the foot of a turned page
//! too. The text can carry the page's number, the page count and a Bates
//! number. Backgrounds, a colour or a picture under each page's own
//! drawing, are put on the same way. The change is one incremental
//! update.

use std::io::Write as _;

use crate::document::Document;
use crate::edit::EditError;
use crate::fonts::{stand_in, win_ansi_code, win_ansi_width};
use crate::insert::{Image, PageAdd, image_object, num};

/// Where on the page a stamp goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampPlace {
    TopLeft,
    TopCentre,
    TopRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
    /// Across the middle, from the bottom left corner towards the top
    /// right, as a watermark.
    Across,
    /// In the middle, level.
    Middle,
}

/// Numbers that run on from page to page, for legal papers: `prefix`,
/// then `start` (counting up a page at a time) padded with zeros to
/// `digits`, then `suffix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bates {
    pub prefix: String,
    pub suffix: String,
    pub start: u64,
    pub digits: usize,
}

/// Text drawn on many pages. In `text`, `{page}` becomes the page's
/// number, `{pages}` the number of pages and `{bates}` the page's Bates
/// number; `\n` starts a new line.
#[derive(Debug, Clone, PartialEq)]
pub struct Stamp {
    pub text: String,
    pub place: StampPlace,
    /// One of the 14 standard fonts by name.
    pub font: String,
    pub size: f32,
    pub colour: [f32; 3],
    /// From 0 (unseen) to 1 (solid).
    pub opacity: f32,
    /// How far from the page's edges, in points.
    pub margin: f32,
    /// Drawn under the page's own drawing rather than over it.
    pub under: bool,
    pub bates: Option<Bates>,
}

impl Stamp {
    /// The text as it reads on page `page` (counted from 0) of `pages`,
    /// the `nth` page stamped.
    pub fn text_for(&self, page: usize, pages: usize, nth: usize) -> String {
        let mut text = self
            .text
            .replace("{page}", &(page + 1).to_string())
            .replace("{pages}", &pages.to_string());
        if let Some(b) = &self.bates {
            let number = b.start.saturating_add(nth as u64);
            let bates = format!(
                "{}{number:0digits$}{}",
                b.prefix,
                b.suffix,
                digits = b.digits
            );
            text = text.replace("{bates}", &bates);
        }
        text
    }
}

/// A colour, a picture, or both (the picture over the colour), under
/// what pages draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    pub colour: Option<[f32; 3]>,
    pub image: Option<Image>,
    /// The picture stretched over the whole page rather than fitted
    /// inside it, centred, keeping its shape.
    pub stretch: bool,
    /// From 0 (unseen) to 1 (solid).
    pub opacity: f32,
}

/// The shown page's width and height, and the matrix from its upright
/// bottom left corner to the page's own space.
fn shown_page(crop: [f32; 4], rotate: i64) -> (f32, f32, [f32; 6]) {
    let [l, b, r, t] = crop;
    let (l, r, b, t) = (l.min(r), l.max(r), b.min(t), b.max(t));
    match rotate.rem_euclid(360) {
        90 => (t - b, r - l, [0.0, 1.0, -1.0, 0.0, r, b]),
        180 => (r - l, t - b, [-1.0, 0.0, 0.0, -1.0, r, t]),
        270 => (t - b, r - l, [0.0, -1.0, 1.0, 0.0, l, t]),
        _ => (r - l, t - b, [1.0, 0.0, 0.0, 1.0, l, b]),
    }
}

impl Document {
    /// The file with `background` under each of `pages` (counted from 0).
    pub fn background(
        &self,
        pages: &[usize],
        background: &Background,
    ) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        // One picture, shared by every page.
        let image = background.image.as_ref().map(|image| {
            let (w, h) = match image {
                Image::Pixels(p) => (p.width, p.height),
                Image::Jpeg { width, height, .. } => (*width, *height),
            };
            (
                image_object(&mut update, image),
                w.max(1) as f32,
                h.max(1) as f32,
            )
        });
        for &page in pages {
            let info = self.page_info(page).ok_or(EditError::NoPage)?;
            let mut add = PageAdd::new(self, page)?;
            let state = (background.opacity < 1.0)
                .then(|| add.opacity(&mut update, background.opacity.clamp(0.0, 1.0)));
            let (w, h, shown) = shown_page(info.crop, info.rotate);
            let mut out = Vec::new();
            let _ = write!(out, "q {} cm ", nums(&shown));
            if let Some(state) = &state {
                let _ = write!(out, "/{state} gs ");
            }
            if let Some([r, g, b]) = background.colour {
                let _ = writeln!(
                    out,
                    "{} rg 0 0 {} {} re f",
                    nums(&[r, g, b]),
                    num(w),
                    num(h)
                );
            }
            if let Some((image, iw, ih)) = image {
                let key = add.xobject(image);
                let (dw, dh) = if background.stretch {
                    (w, h)
                } else {
                    let scale = (w / iw).min(h / ih);
                    (iw * scale, ih * scale)
                };
                let _ = writeln!(
                    out,
                    "q {} cm /{key} Do Q",
                    nums(&[dw, 0.0, 0.0, dh, (w - dw) / 2.0, (h - dh) / 2.0])
                );
            }
            out.extend_from_slice(b"Q\n");
            add.finish(&mut update, &out, true);
        }
        Ok(update.finish())
    }

    /// The file with `stamp` drawn on each of `pages` (counted from 0).
    pub fn stamp(&self, pages: &[usize], stamp: &Stamp) -> Result<Vec<u8>, EditError> {
        let count = self.pages().len();
        let mut update = self.update()?;
        let (font, metrics) = stand_in(&stamp.font);
        for (nth, &page) in pages.iter().enumerate() {
            let info = self.page_info(page).ok_or(EditError::NoPage)?;
            let mut add = PageAdd::new(self, page)?;
            let key = add.font(&mut update, font);
            let state = (stamp.opacity < 1.0)
                .then(|| add.opacity(&mut update, stamp.opacity.clamp(0.0, 1.0)));
            let text = stamp.text_for(page, count, nth);
            let mut lines = Vec::new();
            for line in text.split('\n') {
                let mut bytes = Vec::new();
                let mut wide = 0.0;
                for ch in line.chars() {
                    let ch = if ch == '\t' || ch == '\u{a0}' {
                        ' '
                    } else {
                        ch
                    };
                    let (code, _) = win_ansi_code(ch).ok_or(EditError::Unsupported(ch))?;
                    bytes.push(code);
                    wide += win_ansi_width(metrics, code);
                }
                lines.push((bytes, wide as f32 * stamp.size / 1000.0));
            }
            // The shown page, upright, from its bottom left corner.
            let (w, h, shown) = shown_page(info.crop, info.rotate);
            let size = stamp.size;
            let lead = size * 1.2;
            let tall = lead * (lines.len() - 1) as f32;
            let m = stamp.margin;
            // The first line's baseline, and the turn about the middle.
            let (top, angle) = match stamp.place {
                StampPlace::TopLeft | StampPlace::TopCentre | StampPlace::TopRight => {
                    (h - m - size * 0.75, 0.0)
                }
                StampPlace::BottomLeft | StampPlace::BottomCentre | StampPlace::BottomRight => {
                    (m + tall, 0.0)
                }
                StampPlace::Middle => (h / 2.0 + tall / 2.0 - size * 0.35, 0.0),
                StampPlace::Across => (tall / 2.0 - size * 0.35, h.atan2(w)),
            };
            let mut out = Vec::new();
            let _ = write!(out, "q {} cm ", nums(&shown));
            if let Some(state) = &state {
                let _ = write!(out, "/{state} gs ");
            }
            if stamp.place == StampPlace::Across {
                let (s, c) = angle.sin_cos();
                let _ = write!(out, "{} cm ", nums(&[c, s, -s, c, w / 2.0, h / 2.0]));
            }
            let [cr, cg, cb] = stamp.colour;
            let _ = writeln!(out, "BT {} rg /{key} {} Tf", nums(&[cr, cg, cb]), num(size));
            for (i, (bytes, wide)) in lines.iter().enumerate() {
                let x = match stamp.place {
                    StampPlace::TopLeft | StampPlace::BottomLeft => m,
                    StampPlace::TopRight | StampPlace::BottomRight => w - m - wide,
                    StampPlace::Across => -wide / 2.0,
                    _ => (w - wide) / 2.0,
                };
                let y = top - lead * i as f32;
                let _ = write!(out, "1 0 0 1 {} {} Tm ", num(x), num(y));
                crate::write::write_object(
                    &mut out,
                    &crate::object::Object::String {
                        bytes: bytes.clone(),
                        hex: false,
                    },
                );
                out.extend_from_slice(b" Tj\n");
            }
            out.extend_from_slice(b"ET Q\n");
            add.finish(&mut update, &out, stamp.under);
        }
        Ok(update.finish())
    }
}

fn nums(values: &[f32]) -> String {
    values.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
}
