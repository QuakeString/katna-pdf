// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure-Rust engine: hayro (docs/PLAN.md §3.1).

use std::sync::Arc;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

use crate::{Bitmap, Document, Engine, OpenError, Region, fit_scale};

/// hayro, pure Rust, drawing on the CPU.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hayro;

impl Engine for Hayro {
    fn name(&self) -> &'static str {
        "hayro"
    }

    fn open(&self, bytes: Arc<Vec<u8>>, password: &str) -> Result<Arc<dyn Document>, OpenError> {
        let pdf = Pdf::new_with_password(bytes, password).map_err(|err| match err {
            LoadPdfError::Decryption(_) => OpenError::Locked,
            LoadPdfError::Invalid => OpenError::Invalid,
        })?;
        let sizes: Vec<_> = pdf
            .pages()
            .iter()
            .map(|page| {
                let (w, h) = page.render_dimensions();
                (w.max(1.0), h.max(1.0))
            })
            .collect();
        if sizes.is_empty() {
            return Err(OpenError::Invalid);
        }
        Ok(Arc::new(HayroDocument { pdf, sizes }))
    }
}

struct HayroDocument {
    pdf: Pdf,
    /// Each page's size in points, read once on open.
    sizes: Vec<(f32, f32)>,
}

impl HayroDocument {
    /// Draws page `page` through `transform` (from the page's own
    /// coordinates to pixels) into a `width`×`height` bitmap.
    fn draw(
        &self,
        page: usize,
        scale: f32,
        origin: (f64, f64),
        size: (u32, u32),
    ) -> Option<Bitmap> {
        let (width, height) = size;
        let (w16, h16) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
        if w16 == 0 || h16 == 0 {
            return None;
        }
        let pages = self.pdf.pages();
        let pdf_page = pages.get(page)?;
        let transform = Affine::translate((-origin.0, -origin.1))
            * Affine::scale(f64::from(scale))
            * pdf_page.initial_transform(true).to_kurbo();
        let cache = RenderCache::new();
        let mut ctx = RenderContext::new(w16, h16);
        render_into(
            pdf_page,
            &cache,
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &mut ctx,
            transform,
        );
        ctx.flush();
        let mut pixmap = Pixmap::new(w16, h16);
        ctx.render_with(
            &mut pixmap,
            &mut Resources::default(),
            RasterizerSettings {
                target_init: TargetInit::Clear(WHITE),
                ..Default::default()
            },
        );
        Some(Bitmap {
            width,
            height,
            rgba: pixmap.data_as_u8_slice().to_vec(),
        })
    }
}

impl Document for HayroDocument {
    fn page_count(&self) -> usize {
        self.sizes.len()
    }

    fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        self.sizes.get(page).copied()
    }

    fn render(&self, page: usize, scale: f32) -> Option<Bitmap> {
        let (w, h) = self.page_size(page)?;
        let scale = fit_scale(w, h, scale);
        let size = ((w * scale).round() as u32, (h * scale).round() as u32);
        self.draw(page, scale, (0.0, 0.0), (size.0.max(1), size.1.max(1)))
    }

    fn render_region(&self, page: usize, scale: f32, region: Region) -> Option<Bitmap> {
        if region.width > crate::MAX_SIDE || region.height > crate::MAX_SIDE {
            return None;
        }
        self.draw(
            page,
            scale,
            (f64::from(region.x), f64::from(region.y)),
            (region.width, region.height),
        )
    }
}
