// SPDX-License-Identifier: GPL-3.0-or-later

//! The three engines behind [`Doc`].

use std::sync::Arc;

use crate::{Doc, Image, TILE};

pub fn open(engine: &str, bytes: Vec<u8>) -> Result<Box<dyn Doc>, String> {
    match engine {
        "hayro" => hayro::open(bytes),
        "pdfium" => pdfium::open(bytes),
        "mupdf" => mupdf_engine::open(bytes),
        other => Err(format!("unknown engine {other}")),
    }
}

mod hayro {
    use super::*;
    use katna_pdf_render::{Document, Engine, Hayro, Region};

    struct HayroDoc(Arc<dyn Document>);

    pub fn open(bytes: Vec<u8>) -> Result<Box<dyn Doc>, String> {
        let doc = Hayro.open(Arc::new(bytes), "").map_err(|e| e.to_string())?;
        Ok(Box::new(HayroDoc(doc)))
    }

    impl Doc for HayroDoc {
        fn page_count(&self) -> usize {
            self.0.page_count()
        }

        fn page_size(&self, page: usize) -> (f32, f32) {
            self.0.page_size(page).unwrap_or((612.0, 792.0))
        }

        fn render(&mut self, page: usize, scale: f32) -> Result<Image, String> {
            let b = self.0.render(page, scale).ok_or("render failed")?;
            Ok(Image {
                width: b.width as usize,
                height: b.height as usize,
                channels: 4,
                stride: b.width as usize * 4,
                pixels: b.rgba,
            })
        }

        fn render_tile(&mut self, page: usize, scale: f32, x: u32, y: u32) -> Result<(), String> {
            let region = Region {
                x,
                y,
                width: TILE,
                height: TILE,
            };
            self.0
                .render_region(page, scale, region)
                .map(|_| ())
                .ok_or_else(|| "tile failed".into())
        }
    }
}

mod pdfium {
    use super::*;
    use pdfium_render::prelude::*;
    use std::sync::OnceLock;

    static PDFIUM: OnceLock<Pdfium> = OnceLock::new();

    struct PdfiumDoc {
        doc: PdfDocument<'static>,
    }

    pub fn open(bytes: Vec<u8>) -> Result<Box<dyn Doc>, String> {
        let pdfium = match PDFIUM.get() {
            Some(p) => p,
            None => {
                let lib = std::env::var("PDFIUM_LIB").map_err(|_| "set PDFIUM_LIB")?;
                let bindings = Pdfium::bind_to_library(lib).map_err(|e| e.to_string())?;
                PDFIUM.get_or_init(|| Pdfium::new(bindings))
            }
        };
        let doc = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .map_err(|e| e.to_string())?;
        Ok(Box::new(PdfiumDoc { doc }))
    }

    impl Doc for PdfiumDoc {
        fn page_count(&self) -> usize {
            self.doc.pages().len() as usize
        }

        fn page_size(&self, page: usize) -> (f32, f32) {
            self.doc
                .pages()
                .get(page as i32)
                .map(|p| (p.width().value, p.height().value))
                .unwrap_or((612.0, 792.0))
        }

        fn render(&mut self, page: usize, scale: f32) -> Result<Image, String> {
            let page = self
                .doc
                .pages()
                .get(page as i32)
                .map_err(|e| e.to_string())?;
            let config = PdfRenderConfig::new()
                .scale_page_by_factor(scale)
                .set_maximum_width(8192)
                .set_maximum_height(8192)
                .set_format(PdfBitmapFormat::BGRA);
            let bitmap = page
                .render_with_config(&config)
                .map_err(|e| e.to_string())?;
            let (width, height) = (bitmap.width() as usize, bitmap.height() as usize);
            // BGRA: the grey picture only weighs channels, so swap here to
            // keep the comparison fair.
            let mut pixels = bitmap.as_raw_bytes().to_vec();
            for px in pixels.as_chunks_mut::<4>().0 {
                px.swap(0, 2);
            }
            Ok(Image {
                width,
                height,
                channels: 4,
                stride: pixels.len() / height.max(1),
                pixels,
            })
        }

        fn render_tile(&mut self, page: usize, scale: f32, x: u32, y: u32) -> Result<(), String> {
            let page = self
                .doc
                .pages()
                .get(page as i32)
                .map_err(|e| e.to_string())?;
            let mut bitmap = PdfBitmap::empty(TILE as i32, TILE as i32, PdfBitmapFormat::BGRA)
                .map_err(|e| e.to_string())?;
            let config = PdfRenderConfig::new()
                .scale_page_by_factor(scale)
                .set_maximum_width(i32::MAX / 4)
                .set_maximum_height(i32::MAX / 4)
                .set_origin(-(x as i32), -(y as i32));
            page.render_into_bitmap_with_config(&mut bitmap, &config)
                .map_err(|e| e.to_string())
        }
    }
}

mod mupdf_engine {
    use super::*;
    use mupdf::{Colorspace, Device, Document, IRect, Matrix, Pixmap};

    struct MupdfDoc {
        doc: Document,
    }

    pub fn open(bytes: Vec<u8>) -> Result<Box<dyn Doc>, String> {
        let doc = Document::from_bytes(&bytes, "application/pdf").map_err(|e| e.to_string())?;
        if doc.needs_password().map_err(|e| e.to_string())? {
            return Err("password".into());
        }
        Ok(Box::new(MupdfDoc { doc }))
    }

    impl Doc for MupdfDoc {
        fn page_count(&self) -> usize {
            self.doc.page_count().unwrap_or(0).max(0) as usize
        }

        fn page_size(&self, page: usize) -> (f32, f32) {
            self.doc
                .load_page(page as i32)
                .and_then(|p| p.bounds())
                .map(|r| (r.x1 - r.x0, r.y1 - r.y0))
                .unwrap_or((612.0, 792.0))
        }

        fn render(&mut self, page: usize, scale: f32) -> Result<Image, String> {
            let page = self.doc.load_page(page as i32).map_err(|e| e.to_string())?;
            let pixmap = page
                .to_pixmap(
                    &Matrix::new_scale(scale, scale),
                    &Colorspace::device_rgb(),
                    false,
                    true,
                )
                .map_err(|e| e.to_string())?;
            Ok(Image {
                width: pixmap.width() as usize,
                height: pixmap.height() as usize,
                channels: pixmap.n() as usize,
                stride: pixmap.stride() as usize,
                pixels: pixmap.samples().to_vec(),
            })
        }

        fn render_tile(&mut self, page: usize, scale: f32, x: u32, y: u32) -> Result<(), String> {
            let page = self.doc.load_page(page as i32).map_err(|e| e.to_string())?;
            let bounds = page.bounds().map_err(|e| e.to_string())?;
            let (x0, y0) = (
                (bounds.x0 * scale) as i32 + x as i32,
                (bounds.y0 * scale) as i32 + y as i32,
            );
            let rect = IRect {
                x0,
                y0,
                x1: x0 + TILE as i32,
                y1: y0 + TILE as i32,
            };
            let mut pixmap = Pixmap::new_with_rect(&Colorspace::device_rgb(), rect, false)
                .map_err(|e| e.to_string())?;
            pixmap.clear_with(255).map_err(|e| e.to_string())?;
            let device = Device::from_pixmap(&pixmap).map_err(|e| e.to_string())?;
            page.run(&device, &Matrix::new_scale(scale, scale))
                .map_err(|e| e.to_string())?;
            drop(device);
            std::hint::black_box(pixmap.samples());
            Ok(())
        }
    }
}
