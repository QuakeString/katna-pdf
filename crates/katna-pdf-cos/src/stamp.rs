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

use crate::content::splice;
use crate::document::Document;
use crate::edit::EditError;
use crate::fonts::{stand_in, win_ansi_code, win_ansi_width};
use crate::forms::text_object;
use crate::insert::{Image, PageAdd, image_object, num};
use crate::object::{Dict, Object, Ref};
use crate::outline::text_string;

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

/// A mark on pages as it was put there: found again from what the
/// pages draw, so it can be taken off or changed.
#[derive(Debug, Clone, PartialEq)]
pub struct PageMark {
    pub id: String,
    /// The pages it is on, counted from 0.
    pub pages: Vec<usize>,
    pub what: MarkWhat,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarkWhat {
    Stamp(Stamp),
    /// A background; its picture, if one, is the page's XObject
    /// `picture`, which [`Document::background_with`] keeps.
    Background(Background, Option<Ref>),
}

impl StampPlace {
    pub fn name(self) -> &'static str {
        match self {
            StampPlace::TopLeft => "TopLeft",
            StampPlace::TopCentre => "TopCentre",
            StampPlace::TopRight => "TopRight",
            StampPlace::BottomLeft => "BottomLeft",
            StampPlace::BottomCentre => "BottomCentre",
            StampPlace::BottomRight => "BottomRight",
            StampPlace::Across => "Across",
            StampPlace::Middle => "Middle",
        }
    }

    pub fn from_name(name: &str) -> Option<StampPlace> {
        Some(match name {
            "TopLeft" => StampPlace::TopLeft,
            "TopCentre" => StampPlace::TopCentre,
            "TopRight" => StampPlace::TopRight,
            "BottomLeft" => StampPlace::BottomLeft,
            "BottomCentre" => StampPlace::BottomCentre,
            "BottomRight" => StampPlace::BottomRight,
            "Across" => StampPlace::Across,
            "Middle" => StampPlace::Middle,
            _ => return None,
        })
    }

    /// The artifact subtype a mark here is (§14.8.2.2).
    fn artifact(self) -> &'static str {
        match self {
            StampPlace::TopLeft | StampPlace::TopCentre | StampPlace::TopRight => "Header",
            StampPlace::BottomLeft | StampPlace::BottomCentre | StampPlace::BottomRight => "Footer",
            StampPlace::Across | StampPlace::Middle => "Watermark",
        }
    }
}

fn real(v: f32) -> Object {
    Object::Real(f64::from(v))
}

fn colour_array(c: [f32; 3]) -> Object {
    Object::Array(c.iter().map(|v| real(*v)).collect())
}

/// The mark's settings, kept with it so it can be read back.
fn mark_dict(id: &str, what: &MarkWhat) -> Dict {
    let mut d = Dict::new();
    d.set("Mark", text_object(id));
    match what {
        MarkWhat::Stamp(s) => {
            d.set("Kind", Object::name("Stamp"));
            d.set("Text", text_object(&s.text));
            d.set("Place", Object::name(s.place.name()));
            d.set("Font", text_object(&s.font));
            d.set("Size", real(s.size));
            d.set("Colour", colour_array(s.colour));
            d.set("Opacity", real(s.opacity));
            d.set("Margin", real(s.margin));
            d.set("Under", Object::Bool(s.under));
            if let Some(b) = &s.bates {
                let mut bates = Dict::new();
                bates.set("Prefix", text_object(&b.prefix));
                bates.set("Suffix", text_object(&b.suffix));
                bates.set("Start", Object::Int(b.start as i64));
                bates.set("Digits", Object::Int(b.digits as i64));
                d.set("Bates", Object::Dict(bates));
            }
        }
        MarkWhat::Background(b, picture) => {
            d.set("Kind", Object::name("Background"));
            if let Some(c) = b.colour {
                d.set("Colour", colour_array(c));
            }
            d.set("Stretch", Object::Bool(b.stretch));
            d.set("Opacity", real(b.opacity));
            if let Some(r) = picture {
                d.set("Picture", Object::Ref(*r));
            }
        }
    }
    d
}

impl Document {
    /// The marks on the pages, as [`Document::stamp`] and
    /// [`Document::background`] put them there.
    pub fn page_marks(&self) -> Vec<PageMark> {
        let mut found: Vec<PageMark> = Vec::new();
        for page in 0..self.pages().len() {
            let Some(read) = self.page_content(page) else {
                continue;
            };
            for op in &read.ops {
                let Some((id, what)) = self.mark_of(op) else {
                    continue;
                };
                match found.iter_mut().find(|m| m.id == id) {
                    Some(m) => {
                        if !m.pages.contains(&page) {
                            m.pages.push(page);
                        }
                    }
                    None => found.push(PageMark {
                        id,
                        pages: vec![page],
                        what,
                    }),
                }
            }
        }
        found
    }

    /// The mark `op` starts, if it is a `BDC` of one.
    fn mark_of(&self, op: &crate::content::Op) -> Option<(String, MarkWhat)> {
        if !op.is("BDC") {
            return None;
        }
        let Object::Dict(props) = op.operands.get(1)? else {
            return None;
        };
        let Object::Dict(k) = props.get("Katna")? else {
            return None;
        };
        let text = |key: &str| match k.get(key) {
            Some(Object::String { bytes, .. }) => Some(text_string(bytes)),
            _ => None,
        };
        let number = |key: &str| k.get(key).and_then(Object::as_number).map(|n| n as f32);
        let flag = |key: &str| matches!(k.get(key), Some(Object::Bool(true)));
        let colour = |key: &str| match k.get(key) {
            Some(Object::Array(a)) => {
                let v: Vec<f32> = a
                    .iter()
                    .filter_map(|n| n.as_number())
                    .map(|n| n as f32)
                    .collect();
                <[f32; 3]>::try_from(v).ok()
            }
            _ => None,
        };
        let id = text("Mark")?;
        let what = match k.name("Kind")? {
            b"Stamp" => MarkWhat::Stamp(Stamp {
                text: text("Text")?,
                place: StampPlace::from_name(&String::from_utf8_lossy(
                    k.name("Place").unwrap_or(b"BottomCentre"),
                ))?,
                font: text("Font").unwrap_or_else(|| "Helvetica".into()),
                size: number("Size").unwrap_or(10.0),
                colour: colour("Colour").unwrap_or([0.0; 3]),
                opacity: number("Opacity").unwrap_or(1.0),
                margin: number("Margin").unwrap_or(36.0),
                under: flag("Under"),
                bates: match k.get("Bates") {
                    Some(Object::Dict(b)) => Some(Bates {
                        prefix: match b.get("Prefix") {
                            Some(Object::String { bytes, .. }) => text_string(bytes),
                            _ => String::new(),
                        },
                        suffix: match b.get("Suffix") {
                            Some(Object::String { bytes, .. }) => text_string(bytes),
                            _ => String::new(),
                        },
                        start: b.get("Start").and_then(Object::as_int).unwrap_or(1).max(0) as u64,
                        digits: b
                            .get("Digits")
                            .and_then(Object::as_int)
                            .unwrap_or(6)
                            .clamp(1, 20) as usize,
                    }),
                    _ => None,
                },
            }),
            b"Background" => MarkWhat::Background(
                Background {
                    colour: colour("Colour"),
                    image: None,
                    stretch: flag("Stretch"),
                    opacity: number("Opacity").unwrap_or(1.0),
                },
                k.get("Picture").and_then(Object::as_ref),
            ),
            _ => return None,
        };
        Some((id, what))
    }

    /// An id no mark on the pages has yet.
    fn next_mark_id(&self) -> String {
        let taken: Vec<String> = self.page_marks().into_iter().map(|m| m.id).collect();
        (1..)
            .map(|n| format!("m{n}"))
            .find(|id| !taken.contains(id))
            .unwrap_or_default()
    }

    /// The file with the marks `ids` name taken off every page, as an
    /// incremental update.
    pub fn remove_marks(&self, ids: &[String]) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        for page in 0..self.pages().len() {
            let Some(read) = self.page_content(page) else {
                continue;
            };
            let mut edits = Vec::new();
            let mut i = 0;
            while i < read.ops.len() {
                let op = &read.ops[i];
                let mine = self.mark_of(op).is_some_and(|(id, _)| ids.contains(&id));
                if !mine {
                    i += 1;
                    continue;
                }
                // To the `EMC` that closes it, past any nested in it.
                let mut depth = 0usize;
                let mut end = op.span.end;
                let mut j = i;
                while j < read.ops.len() {
                    let o = &read.ops[j];
                    if o.is("BDC") || o.is("BMC") {
                        depth += 1;
                    } else if o.is("EMC") {
                        depth -= 1;
                        if depth == 0 {
                            end = o.span.end;
                            break;
                        }
                    }
                    j += 1;
                }
                edits.push((op.span.start..end, Vec::new()));
                i = j + 1;
            }
            if edits.is_empty() {
                continue;
            }
            let content = splice(&read.content, edits);
            let page_ref = self.pages()[page];
            let stream = update.add(Object::Stream(crate::dss::compressed(&content)));
            let mut dict = self
                .get(page_ref)
                .and_then(|o| o.as_dict().cloned())
                .unwrap_or_default();
            dict.set("Contents", Object::Ref(stream));
            update.replace(page_ref, Object::Dict(dict));
        }
        Ok(update.finish())
    }
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
        self.background_with(pages, background, None)
    }

    /// As [`Document::background`], the picture being the file's own
    /// image XObject `picture` when one is given (a background put on
    /// before, changed).
    pub fn background_with(
        &self,
        pages: &[usize],
        background: &Background,
        picture: Option<Ref>,
    ) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        let id = self.next_mark_id();
        // One picture, shared by every page.
        let mut image = background.image.as_ref().map(|image| {
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
        if image.is_none()
            && let Some(r) = picture
            && let Some(Object::Stream(s)) = self.get(r)
        {
            let size = |key: &str| {
                self.resolve_key(&s.dict, key)
                    .as_number()
                    .unwrap_or(1.0)
                    .max(1.0) as f32
            };
            image = Some((r, size("Width"), size("Height")));
        }
        let props = mark_dict(
            &id,
            &MarkWhat::Background(
                Background {
                    image: None,
                    ..background.clone()
                },
                image.map(|(r, _, _)| r),
            ),
        );
        for &page in pages {
            let info = self.page_info(page).ok_or(EditError::NoPage)?;
            let mut add = PageAdd::new(self, page)?;
            let state = (background.opacity < 1.0)
                .then(|| add.opacity(&mut update, background.opacity.clamp(0.0, 1.0)));
            let (w, h, shown) = shown_page(info.crop, info.rotate);
            let mut out = Vec::new();
            artifact(&mut out, "Background", &props);
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
            out.extend_from_slice(b"Q EMC\n");
            add.finish(&mut update, &out, true);
        }
        Ok(update.finish())
    }

    /// The file with `stamp` drawn on each of `pages` (counted from 0).
    pub fn stamp(&self, pages: &[usize], stamp: &Stamp) -> Result<Vec<u8>, EditError> {
        let count = self.pages().len();
        let mut update = self.update()?;
        let id = self.next_mark_id();
        let props = mark_dict(&id, &MarkWhat::Stamp(stamp.clone()));
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
            artifact(&mut out, stamp.place.artifact(), &props);
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
            out.extend_from_slice(b"ET Q EMC\n");
            add.finish(&mut update, &out, stamp.under);
        }
        Ok(update.finish())
    }
}

/// Opens the marked content a mark is drawn in: a pagination artifact
/// of `subtype`, with the mark's settings under `/Katna`.
fn artifact(out: &mut Vec<u8>, subtype: &str, props: &Dict) {
    let mut d = Dict::new();
    d.set("Type", Object::name("Pagination"));
    d.set("Subtype", Object::name(subtype));
    d.set("Katna", Object::Dict(props.clone()));
    out.extend_from_slice(b"/Artifact ");
    crate::write::write_object(out, &Object::Dict(d));
    out.extend_from_slice(b" BDC\n");
}

fn nums(values: &[f32]) -> String {
    values.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
}
