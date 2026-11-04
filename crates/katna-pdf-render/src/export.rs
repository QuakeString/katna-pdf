// SPDX-License-Identifier: GPL-3.0-or-later

//! Pages drawn out to picture files (PNG or JPEG at a resolution), and
//! drawn as a scanner would see them, for a copy that is pictures only.

use crate::{Bitmap, Document};

/// The kind of picture file a page is drawn to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    /// JPEG at `quality` (1 to 100).
    Jpeg {
        quality: u8,
    },
}

impl PictureKind {
    pub fn extension(self) -> &'static str {
        match self {
            PictureKind::Png => "png",
            PictureKind::Jpeg { .. } => "jpg",
        }
    }
}

/// Page `page` drawn at `dpi` dots to the inch, as a picture file.
pub fn page_picture(
    doc: &dyn Document,
    page: usize,
    dpi: f32,
    kind: PictureKind,
) -> Option<Vec<u8>> {
    let bitmap = doc.render(page, dpi / 72.0)?;
    Some(match kind {
        PictureKind::Png => bitmap.png(),
        PictureKind::Jpeg { quality } => jpeg(&bitmap, quality, false)?,
    })
}

/// Page `page` drawn at `dpi` as a scanner sees a sheet: in shades of
/// grey when `grey`, a touch soft, as a JPEG with its size in pixels.
pub fn scanned_page(
    doc: &dyn Document,
    page: usize,
    dpi: f32,
    grey: bool,
) -> Option<(Vec<u8>, u32, u32)> {
    let mut bitmap = doc.render(page, dpi / 72.0)?;
    soften(&mut bitmap);
    let data = jpeg(&bitmap, 80, grey)?;
    Some((data, bitmap.width, bitmap.height))
}

/// The bitmap as a JPEG, in colour or grey.
fn jpeg(bitmap: &Bitmap, quality: u8, grey: bool) -> Option<Vec<u8>> {
    let pixels: Vec<u8> = bitmap
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            if grey {
                let luma =
                    (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000;
                vec![luma as u8]
            } else {
                p[..3].to_vec()
            }
        })
        .collect();
    let color = if grey {
        jpeg_encoder::ColorType::Luma
    } else {
        jpeg_encoder::ColorType::Rgb
    };
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100))
        .encode(
            &pixels,
            u16::try_from(bitmap.width).ok()?,
            u16::try_from(bitmap.height).ok()?,
            color,
        )
        .ok()?;
    Some(out)
}

/// A slight blur across each row and a little less than full white, so
/// the page reads as scanned, not drawn.
fn soften(bitmap: &mut Bitmap) {
    let w = bitmap.width as usize * 4;
    for row in bitmap.rgba.chunks_exact_mut(w.max(1)) {
        let before = row.to_vec();
        for x in 4..row.len().saturating_sub(4) {
            if x % 4 == 3 {
                continue;
            }
            let mixed =
                (u32::from(before[x - 4]) + 2 * u32::from(before[x]) + u32::from(before[x + 4]))
                    / 4;
            row[x] = (mixed * 250 / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_comes_out_in_colour_and_grey() {
        let bitmap = Bitmap {
            width: 4,
            height: 2,
            rgba: vec![255; 32],
        };
        let colour = jpeg(&bitmap, 80, false).unwrap();
        let grey = jpeg(&bitmap, 80, true).unwrap();
        assert!(colour.starts_with(&[0xFF, 0xD8]));
        assert!(grey.starts_with(&[0xFF, 0xD8]));
        assert_eq!(PictureKind::Jpeg { quality: 80 }.extension(), "jpg");
    }

    #[test]
    fn softening_keeps_the_size_and_dims_white() {
        let mut bitmap = Bitmap {
            width: 6,
            height: 1,
            rgba: vec![255; 24],
        };
        soften(&mut bitmap);
        assert_eq!(bitmap.rgba.len(), 24);
        assert!(bitmap.rgba[8] < 255 && bitmap.rgba[8] > 240);
        assert_eq!(bitmap.rgba[11], 255, "alpha stays");
    }
}
