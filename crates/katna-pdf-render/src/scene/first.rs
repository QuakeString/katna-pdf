// SPDX-License-Identifier: GPL-3.0-or-later

//! A page's first view, drawn while the page is still being read
//! (docs/PLAN.md §2.5, "open to first full-sheet view").
//!
//! Reading a large sheet into a scene and then drawing it takes the two
//! one after the other. Instead the reader hands what it has read, piece
//! by piece, to workers that draw it straight away, so the drawing is
//! done soon after the reading. The view is cut into thin bands dealt
//! out to the workers in turn, so each has a share of the busy parts of
//! the sheet. Only hairlines and dots are drawn this way (`hairline.rs`),
//! which is what large CAD sheets are; at the first item that is not,
//! the workers give up and the view is drawn from the finished scene.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use hayro::kurbo::{Affine, BezPath};

use super::{
    Builder, Clip, Hairlines, Item, Scene, Store, Style, Unsupported, Verb, build_path, hairline,
};
use crate::Bitmap;

/// Bands each worker draws; more share the busy parts out more evenly.
const BANDS_EACH: u32 = 8;

/// What was read since the last piece: the new tails of the builder's
/// arrays, and where each starts in the whole.
struct Chunk {
    item_base: u32,
    verb_base: u32,
    point_base: u32,
    items: Vec<Item>,
    boxes: Vec<[f32; 4]>,
    verbs: Vec<Verb>,
    points: Vec<[f32; 2]>,
    styles: Vec<Style>,
    transforms: Vec<Affine>,
    clips: Vec<Clip>,
}

/// How far the chunks sent so far reach into the builder's arrays.
#[derive(Default, Clone, Copy, PartialEq)]
struct Sent {
    items: usize,
    verbs: usize,
    points: usize,
    styles: usize,
    transforms: usize,
    clips: usize,
}

impl Sent {
    fn of(builder: &Builder) -> Sent {
        Sent {
            items: builder.items.len(),
            verbs: builder.verbs.len(),
            points: builder.points.len(),
            styles: builder.styles.len(),
            transforms: builder.transforms.len(),
            clips: builder.clips.len(),
        }
    }

    /// What `builder` holds past `self`.
    fn cut(self, builder: &Builder) -> Chunk {
        Chunk {
            item_base: self.items as u32,
            verb_base: self.verbs as u32,
            point_base: self.points as u32,
            items: builder.items[self.items..].to_vec(),
            boxes: builder.boxes[self.items..].to_vec(),
            verbs: builder.verbs[self.verbs..].to_vec(),
            points: builder.points[self.points..].to_vec(),
            styles: builder.styles[self.styles..].to_vec(),
            transforms: builder.transforms[self.transforms..].to_vec(),
            clips: builder.clips[self.clips..].to_vec(),
        }
    }
}

/// A worker's copy of the scene so far: the chunks, shared, and the
/// styles, transforms and clips gathered from them.
#[derive(Default)]
struct Pieces {
    chunks: Vec<Arc<Chunk>>,
    styles: Vec<Style>,
    transforms: Vec<Affine>,
    clips: Vec<Clip>,
}

impl Pieces {
    fn add(&mut self, chunk: Arc<Chunk>) {
        self.styles.extend_from_slice(&chunk.styles);
        self.transforms.extend_from_slice(&chunk.transforms);
        self.clips.extend_from_slice(&chunk.clips);
        self.chunks.push(chunk);
    }

    /// The chunk holding item `i`.
    fn with_item(&self, i: u32) -> &Chunk {
        let at = self.chunks.partition_point(|c| c.item_base <= i);
        &self.chunks[at - 1]
    }
}

impl Store for Pieces {
    fn item(&self, i: u32) -> &Item {
        let chunk = self.with_item(i);
        &chunk.items[(i - chunk.item_base) as usize]
    }

    fn bbox(&self, i: u32) -> &[f32; 4] {
        let chunk = self.with_item(i);
        &chunk.boxes[(i - chunk.item_base) as usize]
    }

    fn style(&self, s: u32) -> &Style {
        &self.styles[s as usize]
    }

    fn transform(&self, t: u32) -> &Affine {
        &self.transforms[t as usize]
    }

    fn clip(&self, c: u32) -> &Clip {
        &self.clips[c as usize]
    }

    fn path(&self, verbs: (u32, u32), first: u32, out: &mut BezPath) {
        // A path is stored whole, so its verbs and points are in the one
        // chunk; a glyph's may be in an earlier chunk than its item.
        let at = self.chunks.partition_point(|c| c.verb_base <= verbs.0);
        let chunk = &self.chunks[at - 1];
        let start = (verbs.0 - chunk.verb_base) as usize;
        build_path(
            &chunk.verbs[start..start + verbs.1 as usize],
            &chunk.points[(first - chunk.point_base) as usize..],
            out,
        );
    }
}

/// The view's bands: `count` bands of `rows` pixel rows, the last
/// perhaps shorter.
#[derive(Clone, Copy)]
struct Bands {
    width: u32,
    height: u32,
    rows: u32,
    count: u32,
    scale: f64,
    origin: (f64, f64),
}

impl Bands {
    fn size(&self, band: u32) -> (u32, u32) {
        let top = band * self.rows;
        (self.width, self.rows.min(self.height - top))
    }

    fn view(&self, band: u32) -> Affine {
        let top = f64::from(band * self.rows);
        Affine::translate((-self.origin.0, -(self.origin.1 + top))) * Affine::scale(self.scale)
    }

    /// The bands an item with box `b` (page points) may touch, grown by
    /// a pixel as [`Scene::render`] grows its region.
    fn touched(&self, b: &[f32; 4]) -> Option<(u32, u32)> {
        let (s, (ox, oy)) = (self.scale, self.origin);
        let x0 = f64::from(b[0]) * s - ox - 1.0;
        let x1 = f64::from(b[2]) * s - ox + 1.0;
        let y0 = f64::from(b[1]) * s - oy - 1.0;
        let y1 = f64::from(b[3]) * s - oy + 1.0;
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        if x1 < 0.0 || x0 > w || y1 < 0.0 || y0 > h {
            return None;
        }
        let rows = f64::from(self.rows);
        let first = (y0.max(0.0) / rows) as u32;
        let last = ((y1 / rows) as u32).min(self.count - 1);
        Some((first, last))
    }
}

/// Runs `read`, which reads a page and shows its progress, drawing the
/// `size` pixel region at `origin` of the page at `scale` from what it
/// has read as it goes. `read` returns the scene and whether the quick
/// reader read it; anything else, and the drawing is `None`.
pub(super) fn draw_while_reading(
    scale: f64,
    origin: (f64, f64),
    size: (u32, u32),
    read: impl FnOnce(&mut dyn FnMut(&Builder)) -> (Result<Scene, Unsupported>, bool),
) -> (Result<Scene, Unsupported>, Option<Bitmap>) {
    let (width, height) = size;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    // One core reads and one inflates, but inflating ends first.
    let workers = threads.saturating_sub(1) as u32;
    if workers == 0 || width == 0 || height == 0 {
        return (read(&mut |_| {}).0, None);
    }
    let count = (workers * BANDS_EACH).min(height);
    let rows = height.div_ceil(count);
    let bands = Bands {
        width,
        height,
        rows,
        count: height.div_ceil(rows),
        scale,
        origin,
    };
    let failed = AtomicBool::new(false);
    std::thread::scope(|s| {
        let mut senders = Vec::new();
        let mut handles = Vec::new();
        for me in 0..workers {
            let (send, receive) = mpsc::channel::<Arc<Chunk>>();
            senders.push(send);
            let failed = &failed;
            handles.push(s.spawn(move || draw_bands(me, workers, bands, receive, failed)));
        }
        let mut sent = Sent::default();
        let mut progress = |builder: &Builder| {
            if failed.load(Ordering::Relaxed) || builder.unsupported.is_some() {
                return;
            }
            let now = Sent::of(builder);
            if now == sent {
                return;
            }
            let chunk = Arc::new(sent.cut(builder));
            sent = now;
            for send in &senders {
                let _ = send.send(chunk.clone());
            }
        };
        let (scene, quick) = read(&mut progress);
        drop(senders);
        let drawn: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().ok().flatten())
            .collect();
        // Everything read was drawn: the quick reader read the whole page
        // and every piece of it was sent.
        let whole = quick
            && !failed.load(Ordering::Relaxed)
            && scene.as_ref().is_ok_and(|scene| {
                sent.items == scene.items.len() && sent.verbs == scene.verbs.len()
            });
        if !whole {
            return (scene, None);
        }
        let mut done: Vec<Option<Vec<u8>>> = vec![None; bands.count as usize];
        for worker in drawn {
            let Some(worker) = worker else {
                return (scene, None);
            };
            for (band, rgba) in worker {
                done[band as usize] = Some(rgba);
            }
        }
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for band in done {
            let Some(band) = band else {
                return (scene, None);
            };
            rgba.extend_from_slice(&band);
        }
        (
            scene,
            Some(Bitmap {
                width,
                height,
                rgba,
            }),
        )
    })
}

/// Worker `me` of `workers`: draws bands `me`, `me + workers`, … of
/// each chunk it receives. `None`, with `failed` set, at the first item
/// that is not a hairline or dot.
fn draw_bands(
    me: u32,
    workers: u32,
    bands: Bands,
    receive: mpsc::Receiver<Arc<Chunk>>,
    failed: &AtomicBool,
) -> Option<Vec<(u32, Vec<u8>)>> {
    let mine: Vec<u32> = (me..bands.count).step_by(workers as usize).collect();
    let mut hairlines: Vec<Hairlines> = mine
        .iter()
        .map(|&b| Hairlines::new(bands.view(b), bands.scale, bands.size(b)))
        .collect();
    let mut canvases: Vec<hairline::Canvas> = mine
        .iter()
        .map(|&b| {
            let (w, h) = bands.size(b);
            hairline::Canvas::new(w, h)
        })
        .collect();
    let mut pieces = Pieces::default();
    for chunk in receive {
        if failed.load(Ordering::Relaxed) {
            return None;
        }
        let (base, len) = (chunk.item_base, chunk.items.len() as u32);
        pieces.add(chunk);
        for i in base..base + len {
            let Some((first, last)) = bands.touched(pieces.bbox(i)) else {
                continue;
            };
            // The first of this worker's bands at or after `first`.
            let mut band = first + (me + workers - first % workers) % workers;
            while band <= last {
                let k = ((band - me) / workers) as usize;
                let h = &mut hairlines[k];
                if !h.fits(&pieces, i) || h.draw(&pieces, i, &mut canvases[k]).is_none() {
                    failed.store(true, Ordering::Relaxed);
                    return None;
                }
                band += workers;
            }
        }
    }
    Some(
        mine.into_iter()
            .zip(canvases)
            .map(|(b, canvas)| (b, canvas.into_rgba()))
            .collect(),
    )
}
