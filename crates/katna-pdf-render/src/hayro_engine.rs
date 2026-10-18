// SPDX-License-Identifier: GPL-3.0-or-later

//! The pure-Rust engine: hayro (docs/PLAN.md §3.1), with vector-heavy
//! pages drawn from a [`Scene`] (docs/PLAN.md §2.5).

use std::sync::{Arc, OnceLock};

use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::kurbo::{Affine, BezPath, Rect};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};

use crate::recolor::Area;
use crate::scene::{Scene, is_vector_heavy};
use crate::{Bitmap, Document, Engine, OpenError, Region, fit_scale};

/// hayro, pure Rust, drawing on the CPU. Vector-heavy pages, such as CAD
/// drawings, are read once into a [`Scene`] and drawn from it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Hayro;

/// [`Hayro`] drawing every page the usual way, never from a scene: for
/// comparing the two.
#[derive(Debug, Default, Clone, Copy)]
pub struct HayroPlain;

impl Engine for Hayro {
    fn name(&self) -> &'static str {
        "hayro"
    }

    fn open(&self, bytes: Arc<Vec<u8>>, password: &str) -> Result<Arc<dyn Document>, OpenError> {
        open(bytes, password, true)
    }
}

impl Engine for HayroPlain {
    fn name(&self) -> &'static str {
        "hayro-plain"
    }

    fn open(&self, bytes: Arc<Vec<u8>>, password: &str) -> Result<Arc<dyn Document>, OpenError> {
        open(bytes, password, false)
    }
}

fn open(bytes: Arc<Vec<u8>>, password: &str, scenes: bool) -> Result<Arc<dyn Document>, OpenError> {
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
    let scenes = scenes.then(|| sizes.iter().map(|_| OnceLock::new()).collect());
    Ok(Arc::new(HayroDocument { pdf, sizes, scenes }))
}

struct HayroDocument {
    pdf: Pdf,
    /// Each page's size in points, read once on open.
    sizes: Vec<(f32, f32)>,
    /// Each page's scene, once a draw has asked: `None` for pages that
    /// are not vector-heavy or hold something a scene cannot draw. `None`
    /// altogether for [`HayroPlain`].
    scenes: Option<Vec<OnceLock<Option<Arc<Scene>>>>>,
}

impl HayroDocument {
    /// Page `page`'s scene, built on first use, if it should have one.
    fn scene(&self, page: usize) -> Option<Arc<Scene>> {
        let cell = self.scenes.as_ref()?.get(page)?;
        cell.get_or_init(|| {
            let pages = self.pdf.pages();
            let pdf_page = pages.get(page)?;
            if !is_vector_heavy(pdf_page) {
                return None;
            }
            Scene::build(pdf_page).ok().map(Arc::new)
        })
        .clone()
    }

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
        if let Some(scene) = self.scene(page) {
            return scene.render(scale, origin, size);
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

    fn pictures(&self, page: usize, scale: f32) -> Vec<Area> {
        // A scene's page holds no pictures: it would not have one.
        if self.scene(page).is_some() {
            return Vec::new();
        }
        let (Some((w, h)), Some(pdf_page)) = (self.page_size(page), self.pdf.pages().get(page))
        else {
            return Vec::new();
        };
        let scale = fit_scale(w, h, scale);
        let size = ((w * scale).round(), (h * scale).round());
        let transform =
            Affine::scale(f64::from(scale)) * pdf_page.initial_transform(true).to_kurbo();
        let cache = InterpreterCache::new();
        let mut context = Context::new(
            transform,
            Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)),
            &cache,
            pdf_page.xref(),
            InterpreterSettings::default(),
        );
        let mut found = Pictures::default();
        interpret_page(pdf_page, &mut context, &mut found);
        found
            .areas
            .into_iter()
            .filter_map(|r| {
                let r = r.intersect(Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)));
                (r.width() >= 1.0 && r.height() >= 1.0).then(|| {
                    (
                        r.x0.floor() as u32,
                        r.y0.floor() as u32,
                        r.x1.ceil() as u32,
                        r.y1.ceil() as u32,
                    )
                })
            })
            .collect()
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

/// A device that only notes where raster images are drawn.
#[derive(Default)]
struct Pictures {
    areas: Vec<Rect>,
}

impl<'a> Device<'a> for Pictures {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}
    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        // Image masks (stencils) are ink, not pictures.
        if let Image::Raster(_) = &image {
            // The transform places the image's own pixels.
            let own = Rect::new(
                0.0,
                0.0,
                f64::from(image.width()),
                f64::from(image.height()),
            );
            self.areas.push(props.transform.transform_rect_bbox(own));
        }
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}
