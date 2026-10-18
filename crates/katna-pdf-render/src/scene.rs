// SPDX-License-Identifier: GPL-3.0-or-later

//! The drawing pipeline for vector-heavy pages (docs/PLAN.md §2.5).
//!
//! CAD exports hold hundreds of thousands of tiny strokes. Drawing such a
//! page the usual way reads and flattens every stroke for every tile, so
//! a tile at 3200 % costs as much as the whole sheet. Instead the page is
//! read once into a [`Scene`]: every path, with its colour, stroke, clip
//! and bounding box, in compact arrays, and every box in a grid
//! (`grid.rs`). A tile then draws only what the grid finds inside it, and
//! anything smaller than a pixel at that zoom is drawn as a dot (level of
//! detail).
//!
//! A page uses a scene only when it is vector-heavy ([`is_vector_heavy`])
//! and holds nothing the scene cannot draw yet: images, shadings and
//! patterns, soft masks, blend modes and Type 3 text. Those pages draw the
//! usual way.

use std::collections::HashMap;
use std::sync::Arc;

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_syntax::Pdf;
use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::util::{RectExt, TransformExt};
use hayro::hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, DrawMode, DrawProps, FillRule, Image,
    ImageDrawProps, InterpreterCache, InterpreterSettings, Paint, SoftMask, StrokeProps,
    interpret_page,
};
use hayro::kurbo::{Affine, BezPath, Cap, PathEl, Point, Rect, Shape, Stroke};
use hayro::vello_cpu::color::AlphaColor;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::Fill;
use hayro::vello_cpu::{
    Pixmap, RasterizerSettings, RenderContext, RenderSettings, Resources, TargetInit,
};

use crate::Bitmap;
use crate::grid::Grid;

/// A page with at least this many path segments is vector-heavy.
pub const HEAVY_SEGMENTS: usize = 50_000;

/// Something drawn smaller than this many pixels on both sides is drawn
/// as a dot of its colour rather than stroked or filled (level of detail).
const DOT: f64 = 0.75;

/// Whether `page` is worth a [`Scene`]: it builds at least
/// [`HEAVY_SEGMENTS`] path segments in its own content stream. A quick
/// count of operators; it does not look inside form XObjects.
pub fn is_vector_heavy(page: &Page<'_>) -> bool {
    let mut ops = page.operations();
    let (mut seen, mut segments) = (0, 0);
    while let Some(op) = ops.next() {
        if matches!(op.operator.as_ref(), b"l" | b"c" | b"v" | b"y" | b"re") {
            segments += 1;
            if segments >= HEAVY_SEGMENTS {
                return true;
            }
        }
        // A page this busy with other things is not a drawing; stop
        // rather than read all of a huge or hostile stream.
        seen += 1;
        if seen >= 40 * HEAVY_SEGMENTS {
            return false;
        }
    }
    false
}

/// Reads page `page` of the PDF in `bytes` into a scene, whether or not
/// it is vector-heavy: for tools that measure scenes. `None` when the file
/// does not open or has no such page.
pub fn page_scene(bytes: Arc<Vec<u8>>, page: usize) -> Option<Result<Scene, Unsupported>> {
    let pdf = Pdf::new_with_password(bytes, "").ok()?;
    let pages = pdf.pages();
    Some(Scene::build(pages.get(page)?))
}

/// Why a page cannot be drawn from a scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    Image,
    Pattern,
    SoftMask,
    BlendMode,
    Group,
    Type3,
}

/// A page read once into drawable items and indexed by position.
pub struct Scene {
    /// The page's size in points, rotation applied.
    size: (f32, f32),
    /// Every path's verbs and points, end to end.
    verbs: Vec<Verb>,
    points: Vec<[f32; 2]>,
    items: Vec<Item>,
    styles: Vec<Style>,
    transforms: Vec<Affine>,
    /// Clip paths; an item's clip is its innermost one, and each clip
    /// knows the one around it. Index 0 is "no clip".
    clips: Vec<Clip>,
    /// Each item's box in page points, clip applied: `[x0, y0, x1, y1]`.
    boxes: Vec<[f32; 4]>,
    grid: Grid,
}

#[derive(Debug, Clone, Copy)]
enum Verb {
    Move,
    Line,
    Quad,
    Cubic,
    Close,
}

/// One drawn path. Its points are in the coordinates of its transform;
/// its box is in [`Scene::boxes`].
struct Item {
    verbs: (u32, u32),
    points: u32,
    style: u32,
    transform: u32,
    clip: u32,
}

#[derive(Clone, PartialEq)]
struct Style {
    color: [u8; 4],
    draw: Draw,
}

#[derive(Clone, PartialEq)]
enum Draw {
    Fill(Fill),
    Stroke { stroke: Stroke, text: bool },
}

struct Clip {
    parent: u32,
    verbs: (u32, u32),
    points: u32,
    fill: Fill,
}

impl Scene {
    /// Reads `page` into a scene, or says what on it a scene cannot draw.
    pub fn build(page: &Page<'_>) -> Result<Scene, Unsupported> {
        let (w, h) = page.render_dimensions();
        let transform = page.initial_transform(true).to_kurbo();
        let cache = InterpreterCache::new();
        let mut context = Context::new(
            transform,
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            page.xref(),
            InterpreterSettings::default(),
        );
        let mut builder = Builder::default();
        // The crop box clips everything, as in `hayro::render_into`.
        let mut crop = page.intersected_crop_box().to_kurbo().to_path(0.1);
        crop.apply_affine(transform);
        builder.push_clip_path(&ClipPath {
            path: crop,
            fill: FillRule::NonZero,
        });
        interpret_page(page, &mut context, &mut builder);
        if let Some(why) = builder.unsupported {
            return Err(why);
        }
        let Builder {
            verbs,
            points,
            items,
            styles,
            transforms,
            clips,
            boxes,
            ..
        } = builder;
        let grid = Grid::new([w, h], &boxes);
        Ok(Scene {
            size: (w, h),
            verbs,
            points,
            items,
            styles,
            transforms,
            clips,
            boxes,
            grid,
        })
    }

    /// How many items the scene holds.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The page's size in points.
    pub fn size(&self) -> (f32, f32) {
        self.size
    }

    /// Roughly how much memory the scene holds, in bytes.
    pub fn memory(&self) -> usize {
        use std::mem::size_of;
        self.verbs.capacity() * size_of::<Verb>()
            + self.points.capacity() * size_of::<[f32; 2]>()
            + self.items.capacity() * size_of::<Item>()
            + self.transforms.capacity() * size_of::<Affine>()
            + self.styles.capacity() * size_of::<Style>()
            + self.clips.capacity() * size_of::<Clip>()
            + self.boxes.capacity() * size_of::<[f32; 4]>()
            + self.grid.memory()
    }

    /// Draws the `width`×`height` pixel region whose top left is at
    /// `origin` pixels, of the page at `scale` pixels per point, on white.
    /// It draws on the calling thread; callers draw tiles in parallel.
    pub fn render(&self, scale: f32, origin: (f64, f64), size: (u32, u32)) -> Option<Bitmap> {
        let (width, height) = size;
        let (w16, h16) = (u16::try_from(width).ok()?, u16::try_from(height).ok()?);
        if w16 == 0 || h16 == 0 {
            return None;
        }
        let scale = f64::from(scale);
        let view = Affine::translate((-origin.0, -origin.1)) * Affine::scale(scale);
        // The region in page points, grown by a pixel for strokes and dots
        // that reach in from just outside.
        let pad = 1.0 / scale;
        let region = [
            (origin.0 / scale - pad) as f32,
            (origin.1 / scale - pad) as f32,
            ((origin.0 + f64::from(width)) / scale + pad) as f32,
            ((origin.1 + f64::from(height)) / scale + pad) as f32,
        ];
        // In the page's order, so later items cover earlier ones.
        let mut found = Vec::new();
        self.grid.find(region, &self.boxes, &mut found);

        let mut ctx = RenderContext::new_with(
            w16,
            h16,
            RenderSettings {
                num_threads: 0,
                ..Default::default()
            },
        );
        let mut path = BezPath::new();
        let mut open: Vec<u32> = Vec::new();
        let mut chain: Vec<u32> = Vec::new();
        for i in found {
            let item = &self.items[i as usize];
            self.clip_chain(item.clip, &mut chain);
            let keep = open.iter().zip(&chain).take_while(|(a, b)| a == b).count();
            for _ in keep..open.len() {
                ctx.pop_clip();
            }
            open.truncate(keep);
            for &c in &chain[keep..] {
                let clip = &self.clips[c as usize];
                self.path(clip.verbs, clip.points, &mut path);
                ctx.set_transform(view);
                ctx.set_fill_rule(clip.fill);
                ctx.push_clip_path(&path);
                open.push(c);
            }

            let style = &self.styles[item.style as usize];
            let [r, g, b, a] = style.color;
            ctx.set_paint(AlphaColor::from_rgba8(r, g, b, a));
            let [x0, y0, x1, y1] = self.boxes[i as usize].map(f64::from);
            if (x1 - x0) * scale < DOT && (y1 - y0) * scale < DOT {
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let half = DOT / 2.0 / scale;
                ctx.set_transform(view);
                ctx.fill_rect(&Rect::new(cx - half, cy - half, cx + half, cy + half));
                continue;
            }
            let transform = view * self.transforms[item.transform as usize];
            ctx.set_transform(transform);
            self.path(item.verbs, item.points, &mut path);
            match &style.draw {
                Draw::Fill(fill) => {
                    ctx.set_fill_rule(*fill);
                    ctx.fill_path(&path);
                }
                Draw::Stroke { stroke, text } => {
                    ctx.set_stroke(hairline(stroke, &transform, *text));
                    ctx.stroke_path(&path);
                }
            }
        }
        for _ in &open {
            ctx.pop_clip();
        }
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

    /// The clips around clip `c`, outermost first.
    fn clip_chain(&self, mut c: u32, chain: &mut Vec<u32>) {
        chain.clear();
        while c != 0 {
            chain.push(c);
            c = self.clips[c as usize].parent;
        }
        chain.reverse();
    }

    fn path(&self, verbs: (u32, u32), first: u32, out: &mut BezPath) {
        out.truncate(0);
        let mut p = first as usize;
        let mut pt = || {
            let [x, y] = self.points[p];
            p += 1;
            Point::new(f64::from(x), f64::from(y))
        };
        for verb in &self.verbs[verbs.0 as usize..(verbs.0 + verbs.1) as usize] {
            match verb {
                Verb::Move => out.move_to(pt()),
                Verb::Line => out.line_to(pt()),
                Verb::Quad => {
                    let a = pt();
                    out.quad_to(a, pt());
                }
                Verb::Cubic => {
                    let (a, b) = (pt(), pt());
                    out.curve_to(a, b, pt());
                }
                Verb::Close => out.close_path(),
            }
        }
    }
}

/// `stroke`, at least a pixel wide on screen (a quarter pixel for text), as
/// the PDF specification asks and as hayro draws it.
fn hairline(stroke: &Stroke, transform: &Affine, text: bool) -> Stroke {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let factor = (a * a + b * b).sqrt().max((c * c + d * d).sqrt());
    let threshold = if text { 0.25 } else { 1.0 };
    let width = stroke.width.max(0.01 / factor);
    let mut stroke = stroke.clone();
    stroke.width = if width * factor < threshold {
        threshold / factor
    } else {
        width
    };
    stroke
}

/// The [`Device`] that records a page into a scene.
#[derive(Default)]
struct Builder {
    verbs: Vec<Verb>,
    points: Vec<[f32; 2]>,
    items: Vec<Item>,
    boxes: Vec<[f32; 4]>,
    styles: Vec<Style>,
    style_ids: HashMap<StyleKey, u32>,
    transforms: Vec<Affine>,
    clips: Vec<Clip>,
    /// The clip stack while interpreting: each entry is a clip index, or
    /// 0 for a clip that did not change anything.
    clip_stack: Vec<u32>,
    /// The page-point bounds of each open clip, matching `clip_stack`.
    clip_bounds: Vec<Rect>,
    /// Glyph outlines already stored: their verbs and first point.
    glyphs: HashMap<u128, ((u32, u32), u32)>,
    groups: usize,
    unsupported: Option<Unsupported>,
    /// The style the last item used.
    last_style: Option<u32>,
}

/// A style's identity for sharing: colour, how it is drawn, and the
/// stroke's numbers as bits.
#[derive(Hash, PartialEq, Eq)]
struct StyleKey([u8; 4], u8, [u64; 4], Vec<u64>);

impl Builder {
    fn fail(&mut self, why: Unsupported) {
        self.unsupported.get_or_insert(why);
    }

    fn current_clip(&self) -> u32 {
        self.clip_stack
            .iter()
            .rev()
            .copied()
            .find(|&c| c != 0)
            .unwrap_or(0)
    }

    fn store_path(&mut self, path: &BezPath) -> ((u32, u32), u32) {
        let first_verb = self.verbs.len() as u32;
        let first_point = self.points.len() as u32;
        let push = |p: Point, points: &mut Vec<[f32; 2]>| points.push([p.x as f32, p.y as f32]);
        for el in path.elements() {
            match *el {
                PathEl::MoveTo(p) => {
                    self.verbs.push(Verb::Move);
                    push(p, &mut self.points);
                }
                PathEl::LineTo(p) => {
                    self.verbs.push(Verb::Line);
                    push(p, &mut self.points);
                }
                PathEl::QuadTo(a, p) => {
                    self.verbs.push(Verb::Quad);
                    push(a, &mut self.points);
                    push(p, &mut self.points);
                }
                PathEl::CurveTo(a, b, p) => {
                    self.verbs.push(Verb::Cubic);
                    push(a, &mut self.points);
                    push(b, &mut self.points);
                    push(p, &mut self.points);
                }
                PathEl::ClosePath => self.verbs.push(Verb::Close),
            }
        }
        (
            (first_verb, self.verbs.len() as u32 - first_verb),
            first_point,
        )
    }

    fn style(&mut self, color: [u8; 4], draw: Draw) -> u32 {
        // A drawing uses one style many times in a row: check the last
        // one before hashing.
        if let Some(last) = self.last_style
            && let Some(style) = self.styles.get(last as usize)
            && style.color == color
            && style.draw == draw
        {
            return last;
        }
        let id = self.intern_style(color, draw);
        self.last_style = Some(id);
        id
    }

    fn intern_style(&mut self, color: [u8; 4], draw: Draw) -> u32 {
        let key = match &draw {
            Draw::Fill(fill) => StyleKey(color, *fill as u8, [0; 4], Vec::new()),
            Draw::Stroke { stroke, text } => StyleKey(
                color,
                2 + u8::from(*text),
                [
                    stroke.width.to_bits(),
                    stroke.miter_limit.to_bits(),
                    stroke.dash_offset.to_bits(),
                    (stroke.join as u64) << 8 | stroke.start_cap as u64,
                ],
                stroke.dash_pattern.iter().map(|d| d.to_bits()).collect(),
            ),
        };
        if let Some(&id) = self.style_ids.get(&key) {
            return id;
        }
        let id = self.styles.len() as u32;
        self.styles.push(Style { color, draw });
        self.style_ids.insert(key, id);
        id
    }

    fn transform(&mut self, transform: Affine) -> u32 {
        if self.transforms.last() != Some(&transform) {
            self.transforms.push(transform);
        }
        self.transforms.len() as u32 - 1
    }

    /// Adds an item drawing the stored path `stored` through `transform`.
    fn add(
        &mut self,
        stored: ((u32, u32), u32),
        bounds: Rect,
        transform: Affine,
        style: u32,
        grow: f64,
    ) {
        let mut bbox = transform.transform_rect_bbox(bounds).inflate(grow, grow);
        if let Some(clip) = self.clip_bounds.last() {
            bbox = bbox.intersect(*clip);
            if bbox.width() < 0.0 || bbox.height() < 0.0 {
                return;
            }
        }
        let transform = self.transform(transform);
        self.items.push(Item {
            verbs: stored.0,
            points: stored.1,
            style,
            transform,
            clip: self.current_clip(),
        });
        self.boxes.push([
            bbox.x0 as f32,
            bbox.y0 as f32,
            bbox.x1 as f32,
            bbox.y1 as f32,
        ]);
    }

    fn color(&mut self, props: &DrawProps<'_>) -> Option<[u8; 4]> {
        if props.soft_mask.is_some() {
            self.fail(Unsupported::SoftMask);
            return None;
        }
        if !matches!(props.blend_mode, BlendMode::Normal) {
            self.fail(Unsupported::BlendMode);
            return None;
        }
        match &props.paint {
            Paint::Color(c) => Some(c.to_rgba().to_rgba8()),
            Paint::Pattern(_) => {
                self.fail(Unsupported::Pattern);
                None
            }
        }
    }

    /// Adds a fill of the stored path. A path with no area fills nothing.
    fn fill(
        &mut self,
        stored: ((u32, u32), u32),
        bounds: Rect,
        transform: Affine,
        color: [u8; 4],
        rule: FillRule,
    ) {
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let style = self.style(color, Draw::Fill(fill_rule(rule)));
        self.add(stored, bounds, transform, style, 0.0);
    }

    /// Adds a stroke of the stored path. A path that is a single point
    /// draws nothing with butt caps, and a dot with round or square ones.
    fn stroke(
        &mut self,
        stored: ((u32, u32), u32),
        bounds: Rect,
        transform: Affine,
        color: [u8; 4],
        props: &StrokeProps,
        text: bool,
    ) {
        let stroke = kurbo_stroke(props);
        let point = bounds.width() <= 0.0 && bounds.height() <= 0.0;
        if stored.0.1 == 0 || (point && stroke.start_cap == Cap::Butt) {
            return;
        }
        // Half the width, in page points, plus room for joins.
        let [a, b, c, d, _, _] = transform.as_coeffs();
        let factor = (a * a + b * b).sqrt().max((c * c + d * d).sqrt());
        let grow = stroke.width * factor * stroke.miter_limit.max(1.0) / 2.0;
        let style = self.style(color, Draw::Stroke { stroke, text });
        self.add(stored, bounds, transform, style, grow);
    }
}

impl<'a> Device<'a> for Builder {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, draw_mode: &DrawMode) {
        if self.unsupported.is_some() || matches!(draw_mode, DrawMode::Invisible) {
            return;
        }
        let Some(color) = self.color(&props) else {
            return;
        };
        let stored = self.store_path(path);
        let bounds = path.bounding_box();
        if let DrawMode::Fill(rule) | DrawMode::FillAndStroke(rule, _) = draw_mode {
            self.fill(stored, bounds, props.transform, color, *rule);
        }
        if let DrawMode::Stroke(stroke) | DrawMode::FillAndStroke(_, stroke) = draw_mode {
            self.stroke(stored, bounds, props.transform, color, stroke, false);
        }
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        let bounds = clip_path.path.bounding_box();
        let bounds = match self.clip_bounds.last() {
            Some(outer) => outer.intersect(bounds),
            None => bounds,
        };
        let (verbs, points) = self.store_path(&clip_path.path);
        self.clips.resize_with(self.clips.len().max(1), || Clip {
            parent: 0,
            verbs: (0, 0),
            points: 0,
            fill: Fill::NonZero,
        });
        let id = self.clips.len() as u32;
        self.clips.push(Clip {
            parent: self.current_clip(),
            verbs,
            points,
            fill: fill_rule(clip_path.fill),
        });
        self.clip_stack.push(id);
        self.clip_bounds.push(bounds);
    }

    fn push_transparency_group(
        &mut self,
        opacity: f32,
        mask: Option<SoftMask<'a>>,
        blend_mode: BlendMode,
    ) {
        // A group that changes nothing can be ignored; real groups need
        // compositing the scene does not do yet.
        if opacity < 1.0 || mask.is_some() || !matches!(blend_mode, BlendMode::Normal) {
            self.fail(Unsupported::Group);
        }
        self.groups += 1;
    }

    fn draw_glyph_run(
        &mut self,
        glyph_run: &GlyphRun<'_, 'a>,
        props: DrawProps<'a>,
        draw_mode: &DrawMode,
    ) {
        if self.unsupported.is_some() || matches!(draw_mode, DrawMode::Invisible) {
            return;
        }
        let Some(color) = self.color(&props) else {
            return;
        };
        for glyph in glyph_run.glyphs() {
            let Glyph::Outline(outline) = &**glyph else {
                self.fail(Unsupported::Type3);
                return;
            };
            // As hayro draws text: fills through the glyph's own
            // transform, strokes in text space so the line width is the
            // text state's, not shrunk by the font matrix.
            if let DrawMode::Fill(_) | DrawMode::FillAndStroke(..) = draw_mode {
                let key = outline.identifier().cache_key();
                let stored = match self.glyphs.get(&key) {
                    Some(&stored) => stored,
                    None => {
                        let stored = self.store_path(&outline.outline());
                        self.glyphs.insert(key, stored);
                        stored
                    }
                };
                let bounds = self.bounds_of(stored);
                let transform = props.transform * glyph.transform();
                self.fill(stored, bounds, transform, color, FillRule::NonZero);
            }
            if let DrawMode::Stroke(stroke) | DrawMode::FillAndStroke(_, stroke) = draw_mode {
                let path = glyph.transform() * outline.outline();
                let stored = self.store_path(&path);
                let bounds = path.bounding_box();
                self.stroke(stored, bounds, props.transform, color, stroke, true);
            }
        }
    }

    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {
        self.fail(Unsupported::Image);
    }

    fn pop_clip(&mut self) {
        self.clip_stack.pop();
        self.clip_bounds.pop();
    }

    fn pop_transparency_group(&mut self) {
        self.groups = self.groups.saturating_sub(1);
    }
}

impl Builder {
    /// The bounding box of a stored path's points.
    fn bounds_of(&self, stored: ((u32, u32), u32)) -> Rect {
        let count: usize = self.verbs[stored.0.0 as usize..(stored.0.0 + stored.0.1) as usize]
            .iter()
            .map(|v| match v {
                Verb::Move | Verb::Line => 1,
                Verb::Quad => 2,
                Verb::Cubic => 3,
                Verb::Close => 0,
            })
            .sum();
        let points = &self.points[stored.1 as usize..stored.1 as usize + count];
        let mut rect = Rect::new(f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for [x, y] in points {
            rect = rect.union_pt(Point::new(f64::from(*x), f64::from(*y)));
        }
        if points.is_empty() { Rect::ZERO } else { rect }
    }
}

fn fill_rule(fill: FillRule) -> Fill {
    match fill {
        FillRule::NonZero => Fill::NonZero,
        FillRule::EvenOdd => Fill::EvenOdd,
    }
}

fn kurbo_stroke(props: &StrokeProps) -> Stroke {
    Stroke {
        width: f64::from(props.line_width),
        join: props.line_join,
        miter_limit: f64::from(props.miter_limit),
        start_cap: props.line_cap,
        end_cap: props.line_cap,
        dash_pattern: props.dash_array.iter().map(|n| f64::from(*n)).collect(),
        dash_offset: f64::from(props.dash_offset),
    }
}
