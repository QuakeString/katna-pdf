// SPDX-License-Identifier: GPL-3.0-or-later

//! Putting new text, pictures and shapes into a page's own content
//! (docs/PLAN.md §4.3, "add text, images and shapes"), not as
//! annotations: they are drawn as the page's text and pictures are, so
//! every reader shows and prints them, and Edit text can change the text
//! later. The page's content is kept as it was; a content stream of
//! their drawing goes after it, with the drawing before it wrapped in
//! `q` … `Q` so nothing it left set changes how they are drawn. Text and
//! pictures stand upright as the page is shown, on a page turned by
//! `/Rotate` too. The change is an incremental update.

use std::io::Write as _;

use crate::document::Document;
use crate::edit::EditError;
use crate::fonts::{stand_in, win_ansi_code};
use crate::object::{Dict, Name, Object, Ref};
use crate::pages::page_dict;
use crate::sign::{Picture, image_stream};
use crate::write::Update;

/// The start of the names given to fonts and pictures put in.
const FONT_KEY: &str = "KatnaT";
const IMAGE_KEY: &str = "KatnaI";
const STATE_KEY: &str = "KatnaG";
/// How far the curve handles of a quarter circle reach, as a share of
/// its radius.
const KAPPA: f32 = 0.552_284_8;

/// Something to put on a page. Places are in the page's own space (PDF's,
/// from the bottom left).
#[derive(Debug, Clone, PartialEq)]
pub enum Insert {
    /// Lines of text (split at `\n`), the first's baseline starting at
    /// `at`, in one of the 14 standard fonts by name (`Helvetica`,
    /// `Times-Bold` and so on; others are taken as the nearest of them).
    Text {
        at: [f32; 2],
        text: String,
        font: String,
        size: f32,
        colour: [f32; 3],
    },
    /// A picture filling `rect` (left, bottom, right, top).
    Image { rect: [f32; 4], image: Image },
    /// A shape: a rectangle or ellipse in `rect`, or a line from its
    /// first corner to its second (x0, y0, x1, y1).
    Shape {
        kind: ShapeKind,
        rect: [f32; 4],
        stroke: Option<[f32; 3]>,
        fill: Option<[f32; 3]>,
        width: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    Ellipse,
    Line,
}

/// A picture to put in: pixels, or a JPEG file's bytes as they are.
#[derive(Debug, Clone, PartialEq)]
pub enum Image {
    Pixels(Picture),
    /// A baseline or progressive JPEG with 1 (gray) or 3 (colour)
    /// components.
    Jpeg {
        data: Vec<u8>,
        width: u32,
        height: u32,
        components: u8,
    },
}

impl Document {
    /// The file with `items` drawn on page `page`, over what it draws.
    pub fn insert(&self, page: usize, items: &[Insert]) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        let mut add = PageAdd::new(self, page)?;
        let rotate = add.rotate;
        let mut out = Vec::new();
        for item in items {
            match item {
                Insert::Text {
                    at,
                    text,
                    font,
                    size,
                    colour,
                } => {
                    let (name, _) = stand_in(font);
                    let key = add.font(&mut update, name);
                    let [a, b, c, d] = upright(rotate);
                    let [r, g, bl] = colour;
                    let _ = writeln!(
                        out,
                        "q BT {} {} {} rg /{key} {} Tf {} TL {a} {b} {c} {d} {} {} Tm",
                        num(*r),
                        num(*g),
                        num(*bl),
                        num(*size),
                        num(size * 1.2),
                        num(at[0]),
                        num(at[1]),
                    );
                    for (i, line) in text.split('\n').enumerate() {
                        let mut bytes = Vec::new();
                        for ch in line.chars() {
                            let ch = if ch == '\t' || ch == '\u{a0}' {
                                ' '
                            } else {
                                ch
                            };
                            let (code, _) = win_ansi_code(ch).ok_or(EditError::Unsupported(ch))?;
                            bytes.push(code);
                        }
                        if i > 0 {
                            out.extend_from_slice(b"T* ");
                        }
                        crate::write::write_object(&mut out, &Object::String { bytes, hex: false });
                        out.extend_from_slice(b" Tj\n");
                    }
                    out.extend_from_slice(b"ET Q\n");
                }
                Insert::Image { rect, image } => {
                    let stream = match image {
                        Image::Pixels(picture) => image_stream(&mut update, picture),
                        Image::Jpeg {
                            data,
                            width,
                            height,
                            components,
                        } => jpeg_stream(data, *width, *height, *components),
                    };
                    let r = update.add(Object::Stream(stream));
                    let key = add.xobject(r);
                    let m = placed(rotate, *rect);
                    let _ = writeln!(
                        out,
                        "q {} {} {} {} {} {} cm /{key} Do Q",
                        num(m[0]),
                        num(m[1]),
                        num(m[2]),
                        num(m[3]),
                        num(m[4]),
                        num(m[5]),
                    );
                }
                Insert::Shape {
                    kind,
                    rect,
                    stroke,
                    fill,
                    width,
                } => {
                    out.extend_from_slice(b"q ");
                    if let Some([r, g, b]) = stroke {
                        let _ = write!(
                            out,
                            "{} {} {} RG {} w ",
                            num(*r),
                            num(*g),
                            num(*b),
                            num(*width)
                        );
                    }
                    if let Some([r, g, b]) = fill {
                        let _ = write!(out, "{} {} {} rg ", num(*r), num(*g), num(*b));
                    }
                    let [x0, y0, x1, y1] = *rect;
                    match kind {
                        ShapeKind::Line => {
                            let _ =
                                write!(out, "{} {} m {} {} l ", num(x0), num(y0), num(x1), num(y1));
                        }
                        ShapeKind::Rectangle => {
                            let _ = write!(
                                out,
                                "{} {} {} {} re ",
                                num(x0.min(x1)),
                                num(y0.min(y1)),
                                num((x1 - x0).abs()),
                                num((y1 - y0).abs())
                            );
                        }
                        ShapeKind::Ellipse => ellipse(&mut out, *rect),
                    }
                    let filled = fill.is_some() && *kind != ShapeKind::Line;
                    let paint = match (stroke.is_some(), filled) {
                        (true, true) => "B",
                        (true, false) => "S",
                        (false, true) => "f",
                        (false, false) => "n",
                    };
                    let _ = writeln!(out, "{paint} Q");
                }
            }
        }
        add.finish(&mut update, &out, false);
        Ok(update.finish())
    }
}

/// A page being drawn on: its dictionary and the resources new drawing
/// adds to, written back by [`PageAdd::finish`].
pub(crate) struct PageAdd<'a> {
    doc: &'a Document,
    page_ref: Ref,
    contents: Option<Object>,
    resources: Dict,
    fonts: Dict,
    xobjects: Dict,
    states: Dict,
    pub(crate) rotate: i64,
}

impl<'a> PageAdd<'a> {
    pub(crate) fn new(doc: &'a Document, page: usize) -> Result<Self, EditError> {
        let page_ref = *doc.pages().get(page).ok_or(EditError::NoPage)?;
        let rotate = doc.page_info(page).map_or(0, |i| i.rotate);
        let full = page_dict(doc, page_ref);
        let resources = match full.get("Resources").map(|r| doc.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let part = |key| match doc.resolve_key(&resources, key) {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        Ok(Self {
            doc,
            page_ref,
            contents: full.get("Contents").cloned(),
            fonts: part("Font"),
            xobjects: part("XObject"),
            states: part("ExtGState"),
            resources,
            rotate,
        })
    }

    /// The key of standard font `name` in the page's fonts, brought in if
    /// not there yet.
    pub(crate) fn font(&mut self, update: &mut Update, name: &str) -> String {
        font_key(self.doc, update, &mut self.fonts, name)
    }

    pub(crate) fn xobject(&mut self, r: Ref) -> String {
        let key = free_key(&self.xobjects, IMAGE_KEY);
        self.xobjects.0.push((Name::new(&key), Object::Ref(r)));
        key
    }

    /// The key of a graphics state drawing at `opacity` (0 to 1).
    pub(crate) fn opacity(&mut self, update: &mut Update, opacity: f32) -> String {
        let state = Dict(vec![
            (Name::new("Type"), Object::name("ExtGState")),
            (Name::new("CA"), Object::Real(f64::from(opacity))),
            (Name::new("ca"), Object::Real(f64::from(opacity))),
        ]);
        let r = update.add(Object::Dict(state));
        let key = free_key(&self.states, STATE_KEY);
        self.states.0.push((Name::new(&key), Object::Ref(r)));
        key
    }

    /// Writes the page back with `drawing` after what it draws, or under
    /// it with `under`; either way what is drawn first is wrapped in
    /// `q` … `Q` so nothing it leaves set reaches what comes after.
    pub(crate) fn finish(mut self, update: &mut Update, drawing: &[u8], under: bool) {
        let mut old = Vec::new();
        match &self.contents {
            Some(Object::Ref(r)) => match self.doc.get(*r) {
                Some(Object::Array(a)) => old.extend(a),
                _ => old.push(Object::Ref(*r)),
            },
            Some(Object::Array(a)) => old.extend(a.iter().cloned()),
            _ => {}
        }
        let contents = if under {
            let mut new = b"q\n".to_vec();
            new.extend_from_slice(drawing);
            new.extend_from_slice(b"Q\n");
            let new = update.add(Object::Stream(crate::dss::compressed(&new)));
            let mut all = vec![Object::Ref(new)];
            all.extend(old);
            all
        } else {
            let before = update.add(Object::Stream(crate::dss::compressed(b"q\n")));
            let mut new = b"Q\n".to_vec();
            new.extend_from_slice(drawing);
            let after = update.add(Object::Stream(crate::dss::compressed(&new)));
            let mut all = vec![Object::Ref(before)];
            all.extend(old);
            all.push(Object::Ref(after));
            all
        };
        let mut dict = self
            .doc
            .get(self.page_ref)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        dict.set("Contents", Object::Array(contents));
        self.resources.set("Font", self.fonts);
        self.resources.set("XObject", self.xobjects);
        if !self.states.0.is_empty() {
            self.resources.set("ExtGState", self.states);
        }
        dict.set("Resources", self.resources);
        update.replace(self.page_ref, Object::Dict(dict));
    }
}

/// The key of standard font `name` in the page's fonts, brought in if not
/// there yet.
fn font_key(doc: &Document, update: &mut Update, fonts: &mut Dict, name: &str) -> String {
    for (key, value) in fonts.iter() {
        if key.as_bytes().starts_with(FONT_KEY.as_bytes())
            && let Object::Dict(font) = doc.resolve(value)
            && font.name("BaseFont") == Some(name.as_bytes())
        {
            return String::from_utf8_lossy(key.as_bytes()).into_owned();
        }
    }
    let key = free_key(fonts, FONT_KEY);
    let font = Dict(vec![
        (Name::new("Type"), Object::name("Font")),
        (Name::new("Subtype"), Object::name("Type1")),
        (Name::new("BaseFont"), Object::name(name)),
        (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
    ]);
    let r: Ref = update.add(Object::Dict(font));
    fonts.0.push((Name::new(&key), Object::Ref(r)));
    key
}

fn free_key(dict: &Dict, prefix: &str) -> String {
    (1..)
        .map(|k| format!("{prefix}{k}"))
        .find(|name| dict.get(name).is_none())
        .unwrap_or_default()
}

/// The text matrix's first four numbers that stand text upright on a
/// page turned by `rotate` degrees clockwise.
fn upright(rotate: i64) -> [i64; 4] {
    match rotate.rem_euclid(360) {
        90 => [0, 1, -1, 0],
        180 => [-1, 0, 0, -1],
        270 => [0, -1, 1, 0],
        _ => [1, 0, 0, 1],
    }
}

/// The matrix that draws a picture (the unit square) upright, as the page
/// is shown, filling `rect`.
fn placed(rotate: i64, rect: [f32; 4]) -> [f32; 6] {
    let [l, b, r, t] = rect;
    let (w, h) = (r - l, t - b);
    match rotate.rem_euclid(360) {
        90 => [0.0, h, -w, 0.0, r, b],
        180 => [-w, 0.0, 0.0, -h, r, t],
        270 => [0.0, -h, w, 0.0, l, t],
        _ => [w, 0.0, 0.0, h, l, b],
    }
}

/// An ellipse in `rect`, as four curves.
fn ellipse(out: &mut Vec<u8>, rect: [f32; 4]) {
    let (l, r) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (b, t) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    let (cx, cy) = ((l + r) / 2.0, (b + t) / 2.0);
    let (rx, ry) = ((r - l) / 2.0, (t - b) / 2.0);
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    let p = |x: f32, y: f32| format!("{} {}", num(x), num(y));
    let _ = write!(
        out,
        "{} m {} {} {} c {} {} {} c {} {} {} c {} {} {} c h ",
        p(r, cy),
        p(r, cy + ky),
        p(cx + kx, t),
        p(cx, t),
        p(cx - kx, t),
        p(l, cy + ky),
        p(l, cy),
        p(l, cy - ky),
        p(cx - kx, b),
        p(cx, b),
        p(cx + kx, b),
        p(r, cy - ky),
        p(r, cy),
    );
}

fn jpeg_stream(data: &[u8], width: u32, height: u32, components: u8) -> crate::object::Stream {
    let dict = Dict(vec![
        (Name::new("Type"), Object::name("XObject")),
        (Name::new("Subtype"), Object::name("Image")),
        (Name::new("Width"), Object::Int(i64::from(width))),
        (Name::new("Height"), Object::Int(i64::from(height))),
        (
            Name::new("ColorSpace"),
            Object::name(if components == 1 {
                "DeviceGray"
            } else {
                "DeviceRGB"
            }),
        ),
        (Name::new("BitsPerComponent"), Object::Int(8)),
        (Name::new("Filter"), Object::name("DCTDecode")),
    ]);
    crate::object::Stream {
        dict,
        data: data.to_vec(),
    }
}

/// A number as content streams write it: no more digits than needed.
pub(crate) fn num(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}
