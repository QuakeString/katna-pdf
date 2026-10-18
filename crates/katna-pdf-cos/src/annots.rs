// SPDX-License-Identifier: GPL-3.0-or-later

//! Annotations people add and read (docs/PLAN.md §4.2): text markup,
//! notes, free text, ink, shapes and stamps. Reading gives each page's
//! annotations as plain values; writing adds, changes and removes them
//! as an incremental update. Every annotation written carries its own
//! appearance stream, drawn in page space, so it looks the same in every
//! viewer (§4.2, "Requirement").

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::document::Document;
use crate::forms::{literal, text_object};
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::outline::text_string;
use crate::write::UpdateError;

/// A note's icon, in points.
pub const NOTE_SIZE: f32 = 20.0;

/// How text markup marks its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkupStyle {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
}

impl MarkupStyle {
    fn subtype(self) -> &'static str {
        match self {
            MarkupStyle::Highlight => "Highlight",
            MarkupStyle::Underline => "Underline",
            MarkupStyle::StrikeOut => "StrikeOut",
            MarkupStyle::Squiggly => "Squiggly",
        }
    }
}

/// What an annotation is, with what places it on the page, in page
/// space (points from the page's bottom left, as the file has them).
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotKind {
    /// Marks text; one quadrilateral per line, its corners in the order
    /// top left, top right, bottom left, bottom right.
    Markup(MarkupStyle, Vec<[f32; 8]>),
    /// A sticky note: an icon at the rectangle's top left.
    Note,
    /// Text written on the page, in the rectangle, at `size` points.
    FreeText { size: f32 },
    /// Pen strokes.
    Ink(Vec<Vec<[f32; 2]>>),
    /// A rectangle in the rectangle, filled if `fill`.
    Square { fill: Option<[f32; 3]> },
    /// An ellipse in the rectangle, filled if `fill`.
    Circle { fill: Option<[f32; 3]> },
    /// A line, with an arrowhead at `to` if `arrow`.
    Line {
        from: [f32; 2],
        to: [f32; 2],
        arrow: bool,
    },
    /// A closed shape through the points.
    Polygon(Vec<[f32; 2]>),
    /// An open line through the points.
    PolyLine(Vec<[f32; 2]>),
    /// A stamp: a word in a box filling the rectangle.
    Stamp(String),
    /// Something else (a file attachment, a sound, a caret...), by its
    /// subtype; read and removed, never written.
    Other(String),
}

/// One annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// Its object in the file; `None` for one not yet written.
    pub id: Option<Ref>,
    pub page: usize,
    pub kind: AnnotKind,
    /// Its box in page space: left, bottom, right, top. Worked out from
    /// the points when written, except for notes, free text, rectangles,
    /// ellipses and stamps, which fill it.
    pub rect: [f32; 4],
    /// Red, green, blue, 0 to 1.
    pub colour: [f32; 3],
    pub opacity: f32,
    /// Line width in points, for ink, shapes and lines.
    pub width: f32,
    /// The comment.
    pub contents: String,
    pub author: String,
    /// When it was last changed, as the file writes dates
    /// (`D:20261018123000Z`).
    pub modified: Option<String>,
    /// The annotation this one replies to.
    pub reply_to: Option<Ref>,
    /// Its name, unique in the file (`/NM`); empty for one not yet
    /// written, which is given one.
    pub name: String,
}

impl Annotation {
    /// A new annotation of `kind` on `page`, yellow, opaque, a point wide.
    pub fn new(page: usize, kind: AnnotKind) -> Self {
        let mut a = Annotation {
            id: None,
            page,
            kind,
            rect: [0.0; 4],
            colour: [1.0, 0.85, 0.0],
            opacity: 1.0,
            width: 1.0,
            contents: String::new(),
            author: String::new(),
            modified: None,
            reply_to: None,
            name: String::new(),
        };
        a.rect = a.bounds();
        a
    }

    /// The box the points reach, grown by half the line width (and the
    /// arrowhead); the rectangle itself for kinds that fill it.
    pub fn bounds(&self) -> [f32; 4] {
        let points: Vec<[f32; 2]> = match &self.kind {
            AnnotKind::Markup(_, quads) => quads
                .iter()
                .flat_map(|q| q.as_chunks::<2>().0.to_vec())
                .collect(),
            AnnotKind::Ink(strokes) => strokes.iter().flatten().copied().collect(),
            AnnotKind::Line { from, to, .. } => vec![*from, *to],
            AnnotKind::Polygon(p) | AnnotKind::PolyLine(p) => p.clone(),
            _ => return normal(self.rect),
        };
        let Some(first) = points.first() else {
            return normal(self.rect);
        };
        let mut b = [first[0], first[1], first[0], first[1]];
        for p in &points {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        }
        let grow = match self.kind {
            AnnotKind::Markup(..) => 1.0,
            AnnotKind::Line { arrow: true, .. } => self.width * 0.5 + arrow_size(self.width),
            _ => self.width * 0.5 + 1.0,
        };
        [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow]
    }
}

/// Changes to a file's annotations, written by [`Document::annotate`].
#[derive(Debug, Default, Clone)]
pub struct AnnotChanges {
    /// Annotations to add (`id` is `None`) or change (`id` is theirs).
    pub put: Vec<Annotation>,
    /// Annotations to take off their pages.
    pub remove: Vec<Ref>,
}

impl Document {
    /// Each page's annotations, in the order the page lists them; links,
    /// form fields and pop-ups are left out.
    pub fn annotations(&self) -> Vec<Vec<Annotation>> {
        self.pages()
            .into_iter()
            .enumerate()
            .map(|(page, r)| {
                let Some(Object::Dict(dict)) = self.get(r) else {
                    return Vec::new();
                };
                let Object::Array(annots) = self.resolve_key(&dict, "Annots") else {
                    return Vec::new();
                };
                annots
                    .iter()
                    .filter_map(|a| self.read_annotation(page, a))
                    .collect()
            })
            .collect()
    }

    fn read_annotation(&self, page: usize, obj: &Object) -> Option<Annotation> {
        let Object::Dict(d) = self.resolve(obj) else {
            return None;
        };
        let subtype = String::from_utf8_lossy(d.name("Subtype")?).into_owned();
        let numbers = |key: &str| -> Vec<f32> {
            match self.resolve_key(&d, key) {
                Object::Array(a) => a
                    .iter()
                    .filter_map(|n| self.resolve(n).as_number())
                    .map(|n| n as f32)
                    .collect(),
                _ => Vec::new(),
            }
        };
        let points = |key: &str| -> Vec<[f32; 2]> { numbers(key).as_chunks::<2>().0.to_vec() };
        let colour = |key: &str| -> Option<[f32; 3]> {
            match numbers(key).as_slice() {
                [g] => Some([*g; 3]),
                [r, g, b] => Some([*r, *g, *b]),
                [c, m, y, k] => Some([
                    (1.0 - c) * (1.0 - k),
                    (1.0 - m) * (1.0 - k),
                    (1.0 - y) * (1.0 - k),
                ]),
                _ => None,
            }
        };
        let markup =
            |style| AnnotKind::Markup(style, numbers("QuadPoints").as_chunks::<8>().0.to_vec());
        let kind = match subtype.as_str() {
            "Link" | "Widget" | "Popup" => return None,
            "Highlight" => markup(MarkupStyle::Highlight),
            "Underline" => markup(MarkupStyle::Underline),
            "StrikeOut" => markup(MarkupStyle::StrikeOut),
            "Squiggly" => markup(MarkupStyle::Squiggly),
            "Text" => AnnotKind::Note,
            "FreeText" => AnnotKind::FreeText {
                size: self.free_text_size(&d).unwrap_or(12.0),
            },
            "Ink" => AnnotKind::Ink(match self.resolve_key(&d, "InkList") {
                Object::Array(strokes) => strokes
                    .iter()
                    .map(|s| match self.resolve(s) {
                        Object::Array(a) => a
                            .iter()
                            .filter_map(|n| self.resolve(n).as_number())
                            .map(|n| n as f32)
                            .collect::<Vec<_>>()
                            .as_chunks::<2>()
                            .0
                            .to_vec(),
                        _ => Vec::new(),
                    })
                    .collect(),
                _ => Vec::new(),
            }),
            "Square" => AnnotKind::Square { fill: colour("IC") },
            "Circle" => AnnotKind::Circle { fill: colour("IC") },
            "Line" => {
                let l = numbers("L");
                let arrow = match self.resolve_key(&d, "LE") {
                    Object::Array(ends) => ends
                        .get(1)
                        .and_then(Object::as_name)
                        .is_some_and(|n| n != b"None"),
                    _ => false,
                };
                match l.as_slice() {
                    [x0, y0, x1, y1] => AnnotKind::Line {
                        from: [*x0, *y0],
                        to: [*x1, *y1],
                        arrow,
                    },
                    _ => AnnotKind::Other(subtype),
                }
            }
            "Polygon" => AnnotKind::Polygon(points("Vertices")),
            "PolyLine" => AnnotKind::PolyLine(points("Vertices")),
            "Stamp" => AnnotKind::Stamp(
                d.name("Name")
                    .map_or("Draft".into(), |n| String::from_utf8_lossy(n).into_owned()),
            ),
            _ => AnnotKind::Other(subtype),
        };
        let text = |key: &str| match self.resolve_key(&d, key) {
            Object::String { bytes, .. } => Some(text_string(&bytes)),
            _ => None,
        };
        let width = match self.resolve_key(&d, "BS") {
            Object::Dict(bs) => self.resolve_key(&bs, "W").as_number(),
            _ => None,
        }
        .unwrap_or(1.0) as f32;
        Some(Annotation {
            id: obj.as_ref(),
            page,
            rect: match numbers("Rect").as_slice() {
                [a, b, c, d] => normal([*a, *b, *c, *d]),
                _ => [0.0; 4],
            },
            colour: match kind {
                AnnotKind::FreeText { .. } => self.free_text_colour(&d),
                _ => colour("C"),
            }
            .unwrap_or([1.0, 0.85, 0.0]),
            opacity: self.resolve_key(&d, "CA").as_number().unwrap_or(1.0) as f32,
            width,
            contents: text("Contents").unwrap_or_default(),
            author: text("T").unwrap_or_default(),
            modified: text("M"),
            reply_to: d.get("IRT").and_then(Object::as_ref),
            name: text("NM").unwrap_or_default(),
            kind,
        })
    }

    /// Page `page`'s list of annotations as it will be, read from the
    /// file the first time it is asked for.
    fn page_list<'m>(
        &self,
        lists: &'m mut BTreeMap<usize, Vec<Object>>,
        pages: &[Ref],
        page: usize,
    ) -> Option<&'m mut Vec<Object>> {
        let r = *pages.get(page)?;
        Some(lists.entry(page).or_insert_with(|| {
            let Some(Object::Dict(dict)) = self.get(r) else {
                return Vec::new();
            };
            match self.resolve_key(&dict, "Annots") {
                Object::Array(a) => a,
                _ => Vec::new(),
            }
        }))
    }

    /// The words of free text's default appearance (`/DA`).
    fn free_text_da(&self, d: &Dict) -> Vec<String> {
        match self.resolve_key(d, "DA") {
            Object::String { bytes, .. } => String::from_utf8_lossy(&bytes)
                .split_whitespace()
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The operand before `op` in free text's default appearance.
    fn free_text_operands(&self, d: &Dict, op: &str, count: usize) -> Option<Vec<f32>> {
        let words = self.free_text_da(d);
        let at = words.iter().position(|w| w == op)?;
        words[at.checked_sub(count)?..at]
            .iter()
            .map(|w| w.parse().ok())
            .collect()
    }

    fn free_text_size(&self, d: &Dict) -> Option<f32> {
        Some(self.free_text_operands(d, "Tf", 1)?[0])
    }

    fn free_text_colour(&self, d: &Dict) -> Option<[f32; 3]> {
        if let Some(c) = self.free_text_operands(d, "rg", 3) {
            return Some([c[0], c[1], c[2]]);
        }
        let g = self.free_text_operands(d, "g", 1)?[0];
        Some([g; 3])
    }

    /// The file with `changes` made, as an incremental update. A changed
    /// annotation keeps what it had that [`Annotation`] does not hold,
    /// and keeps its appearance unless how it looks changed.
    pub fn annotate(&self, changes: &AnnotChanges) -> Result<Vec<u8>, UpdateError> {
        let mut update = self.update()?;
        let pages = self.pages();
        let existing: BTreeMap<Ref, Annotation> = self
            .annotations()
            .into_iter()
            .flatten()
            .filter_map(|a| Some((a.id?, a)))
            .collect();
        let date = pdf_date(std::time::SystemTime::now());
        // Each page's list of annotations, as it will be.
        let mut lists: BTreeMap<usize, Vec<Object>> = BTreeMap::new();
        for annot in &changes.put {
            let Some(&page_ref) = pages.get(annot.page) else {
                continue;
            };
            let mut annot = annot.clone();
            annot.modified = Some(date.clone());
            if !matches!(
                annot.kind,
                AnnotKind::Note
                    | AnnotKind::FreeText { .. }
                    | AnnotKind::Square { .. }
                    | AnnotKind::Circle { .. }
                    | AnnotKind::Stamp(_)
            ) {
                annot.rect = annot.bounds();
            }
            let old = annot.id.and_then(|id| existing.get(&id));
            let mut dict = match annot.id.and_then(|id| self.get(id)) {
                Some(Object::Dict(d)) => d,
                _ => Dict::new(),
            };
            let looks_same = old.is_some_and(|old| {
                old.kind == annot.kind
                    && old.rect == annot.rect
                    && old.colour == annot.colour
                    && old.opacity == annot.opacity
                    && old.width == annot.width
                    && (old.contents == annot.contents
                        || !matches!(annot.kind, AnnotKind::FreeText { .. }))
            });
            if matches!(annot.kind, AnnotKind::Other(_)) && !looks_same {
                continue;
            }
            write_dict(&mut dict, &annot, page_ref);
            let other = matches!(annot.kind, AnnotKind::Other(_));
            if !other && (!looks_same || dict.get("AP").is_none()) {
                let ap = update.add(appearance(&annot));
                dict.set("AP", Dict(vec![(Name::new("N"), Object::Ref(ap))]));
                // An appearance state picks among several; ours is one.
                dict.remove("AS");
            }
            match annot.id {
                Some(id) => {
                    update.replace(id, Object::Dict(dict));
                    // Moved to another page: off the old one, on the new.
                    if let Some(old) = old
                        && old.page != annot.page
                    {
                        if let Some(l) = self.page_list(&mut lists, &pages, old.page) {
                            l.retain(|o| o.as_ref() != Some(id));
                        }
                        if let Some(l) = self.page_list(&mut lists, &pages, annot.page) {
                            l.push(Object::Ref(id));
                        }
                    }
                }
                None => {
                    let id = update.add(Object::Dict(dict));
                    if let Some(l) = self.page_list(&mut lists, &pages, annot.page) {
                        l.push(Object::Ref(id));
                    }
                }
            }
        }
        for &gone in &changes.remove {
            let Some(old) = existing.get(&gone) else {
                continue;
            };
            let popup = match self.get(gone) {
                Some(Object::Dict(d)) => d.get("Popup").and_then(Object::as_ref),
                _ => None,
            };
            if let Some(l) = self.page_list(&mut lists, &pages, old.page) {
                l.retain(|o| {
                    let r = o.as_ref();
                    r != Some(gone) && (popup.is_none() || r != popup)
                });
            }
        }
        for (page, annots) in lists {
            let r = pages[page];
            let Some(Object::Dict(mut dict)) = self.get(r) else {
                continue;
            };
            match dict.get("Annots").and_then(Object::as_ref) {
                // The list is its own object: change that.
                Some(list) => update.replace(list, Object::Array(annots)),
                None => {
                    if annots.is_empty() {
                        dict.remove("Annots");
                    } else {
                        dict.set("Annots", annots);
                    }
                    update.replace(r, Object::Dict(dict));
                }
            }
        }
        Ok(update.finish())
    }
}

/// `a`'s entries in its dictionary.
fn write_dict(dict: &mut Dict, a: &Annotation, page: Ref) {
    let subtype = match &a.kind {
        AnnotKind::Markup(style, _) => style.subtype(),
        AnnotKind::Note => "Text",
        AnnotKind::FreeText { .. } => "FreeText",
        AnnotKind::Ink(_) => "Ink",
        AnnotKind::Square { .. } => "Square",
        AnnotKind::Circle { .. } => "Circle",
        AnnotKind::Line { .. } => "Line",
        AnnotKind::Polygon(_) => "Polygon",
        AnnotKind::PolyLine(_) => "PolyLine",
        AnnotKind::Stamp(_) => "Stamp",
        AnnotKind::Other(_) => "",
    };
    dict.set("Type", Object::name("Annot"));
    if !subtype.is_empty() {
        dict.set("Subtype", Object::name(subtype));
    }
    dict.set("Rect", reals(&a.rect));
    dict.set("P", page);
    if dict.get("F").is_none() {
        // Printed.
        dict.set("F", 4_i64);
    }
    dict.set("C", reals(&a.colour));
    if a.opacity < 1.0 {
        dict.set("CA", Object::Real(f64::from(a.opacity)));
    } else {
        dict.remove("CA");
    }
    if a.contents.is_empty() {
        dict.remove("Contents");
    } else {
        dict.set("Contents", text_object(&a.contents));
    }
    if !a.author.is_empty() {
        dict.set("T", text_object(&a.author));
    }
    if let Some(m) = &a.modified {
        dict.set("M", Object::string(m));
    }
    if !a.name.is_empty() {
        dict.set("NM", text_object(&a.name));
    } else if dict.get("NM").is_none() {
        dict.set("NM", Object::string(&unique_name()));
    }
    match a.reply_to {
        Some(r) => dict.set("IRT", r),
        None => {
            dict.remove("IRT");
        }
    }
    let border = |dict: &mut Dict| {
        dict.set(
            "BS",
            Dict(vec![(Name::new("W"), Object::Real(f64::from(a.width)))]),
        );
    };
    match &a.kind {
        AnnotKind::Markup(_, quads) => {
            dict.set("QuadPoints", reals(quads.as_flattened()));
        }
        AnnotKind::Note => {
            if dict.get("Name").is_none() {
                dict.set("Name", Object::name("Comment"));
            }
        }
        AnnotKind::FreeText { size } => {
            let [r, g, b] = a.colour;
            dict.set(
                "DA",
                Object::string(&format!("/Helv {size:.1} Tf {r:.3} {g:.3} {b:.3} rg")),
            );
            // The colour is the text's; the box has none.
            dict.remove("C");
        }
        AnnotKind::Ink(strokes) => {
            dict.set(
                "InkList",
                strokes
                    .iter()
                    .map(|s| reals(s.as_flattened()))
                    .collect::<Vec<_>>(),
            );
            border(dict);
        }
        AnnotKind::Square { fill } | AnnotKind::Circle { fill } => {
            match fill {
                Some(f) => dict.set("IC", reals(f)),
                None => {
                    dict.remove("IC");
                }
            }
            border(dict);
        }
        AnnotKind::Line { from, to, arrow } => {
            dict.set("L", reals(&[from[0], from[1], to[0], to[1]]));
            dict.set(
                "LE",
                vec![
                    Object::name("None"),
                    Object::name(if *arrow { "OpenArrow" } else { "None" }),
                ],
            );
            border(dict);
        }
        AnnotKind::Polygon(points) | AnnotKind::PolyLine(points) => {
            dict.set("Vertices", reals(points.as_flattened()));
            border(dict);
        }
        AnnotKind::Stamp(name) => {
            dict.set("Name", Object::name(name));
        }
        AnnotKind::Other(_) => {}
    }
}

/// The appearance stream: a form drawn in page space, its box the
/// annotation's rectangle.
fn appearance(a: &Annotation) -> Object {
    let mut c = String::new();
    let [r, g, b] = a.colour;
    let fill = format!("{r:.3} {g:.3} {b:.3} rg");
    let stroke = format!("{r:.3} {g:.3} {b:.3} RG");
    let w = a.width;
    let mut font = false;
    let mut multiply = false;
    if a.opacity < 1.0 {
        c.push_str("/GS0 gs\n");
    }
    match &a.kind {
        AnnotKind::Markup(style, quads) => {
            for q in quads {
                let [tl, tr, bl, br] = corners(q);
                let height = dist(tl, bl).max(1.0);
                match style {
                    MarkupStyle::Highlight => {
                        multiply = true;
                        let _ = writeln!(
                            c,
                            "{fill} {} m {} l {} l {} l h f",
                            pt(tl),
                            pt(tr),
                            pt(br),
                            pt(bl)
                        );
                    }
                    MarkupStyle::Underline | MarkupStyle::StrikeOut => {
                        // Under the text, or through its middle.
                        let t = if *style == MarkupStyle::Underline {
                            0.93
                        } else {
                            0.55
                        };
                        let from = lerp(tl, bl, t);
                        let to = lerp(tr, br, t);
                        let _ = writeln!(
                            c,
                            "{stroke} {:.2} w {} m {} l S",
                            height / 14.0,
                            pt(from),
                            pt(to)
                        );
                    }
                    MarkupStyle::Squiggly => {
                        let from = lerp(tl, bl, 0.9);
                        let to = lerp(tr, br, 0.9);
                        let up = [tl[0] - bl[0], tl[1] - bl[1]];
                        let up = [up[0] / height, up[1] / height];
                        let len = dist(from, to);
                        let step = (height / 6.0).max(1.0);
                        let amp = height / 14.0;
                        let _ = write!(c, "{stroke} {:.2} w {} m", height / 20.0, pt(from));
                        let mut s = step;
                        let mut i = 0;
                        while s < len + step {
                            let p = lerp(from, to, (s / len).min(1.0));
                            let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                            let p = [p[0] + up[0] * amp * sign, p[1] + up[1] * amp * sign];
                            let _ = write!(c, " {} l", pt(p));
                            s += step;
                            i += 1;
                        }
                        c.push_str(" S\n");
                    }
                }
            }
        }
        AnnotKind::Note => {
            // A speech bubble with lines of text in it, at the top left.
            let (x, top) = (a.rect[0], a.rect[3]);
            let s = NOTE_SIZE;
            let (x0, y0, x1, y1) = (x + 1.0, top - s * 0.8, x + s - 1.0, top - 1.0);
            let _ = writeln!(
                c,
                "{fill} 0.25 0.25 0.25 RG 0.8 w {x0:.2} {y0:.2} m {x1:.2} {y0:.2} l {x1:.2} {y1:.2} l \
                 {x0:.2} {y1:.2} l {x0:.2} {y0:.2} l h B"
            );
            let _ = writeln!(
                c,
                "{:.2} {y0:.2} m {:.2} {:.2} l {:.2} {y0:.2} l h B",
                x + s * 0.25,
                x + s * 0.2,
                top - s,
                x + s * 0.45
            );
            for k in 1..=3 {
                let y = y1 - s * 0.2 * k as f32;
                let _ = writeln!(c, "{:.2} {y:.2} m {:.2} {y:.2} l S", x0 + 3.0, x1 - 3.0);
            }
        }
        AnnotKind::FreeText { size } => {
            font = true;
            let [x0, y0, x1, y1] = a.rect;
            let leading = size * 1.2;
            let _ = writeln!(
                c,
                "q {x0:.2} {y0:.2} {:.2} {:.2} re W n BT /Helv {size:.2} Tf {fill} {:.2} {:.2} Td \
                 {leading:.2} TL",
                x1 - x0,
                y1 - y0,
                x0 + 2.0,
                y1 - 2.0 - size
            );
            for (i, line) in a.contents.lines().enumerate() {
                if i > 0 {
                    c.push_str("T* ");
                }
                let _ = writeln!(c, "{} Tj", literal(line));
            }
            c.push_str("ET Q\n");
        }
        AnnotKind::Ink(strokes) => {
            let _ = writeln!(c, "{stroke} {w:.2} w 1 J 1 j");
            for s in strokes {
                let Some((first, rest)) = s.split_first() else {
                    continue;
                };
                let _ = write!(c, "{} m", pt(*first));
                if rest.is_empty() {
                    // A dot.
                    let _ = write!(c, " {} l", pt(*first));
                }
                for p in rest {
                    let _ = write!(c, " {} l", pt(*p));
                }
                c.push_str(" S\n");
            }
        }
        AnnotKind::Square { fill: inside } | AnnotKind::Circle { fill: inside } => {
            let h = w * 0.5;
            let [x0, y0, x1, y1] = [a.rect[0] + h, a.rect[1] + h, a.rect[2] - h, a.rect[3] - h];
            if let Some([r, g, b]) = inside {
                let _ = write!(c, "{r:.3} {g:.3} {b:.3} rg ");
            }
            let _ = write!(c, "{stroke} {w:.2} w ");
            if matches!(a.kind, AnnotKind::Square { .. }) {
                let _ = write!(c, "{x0:.2} {y0:.2} {:.2} {:.2} re", x1 - x0, y1 - y0);
            } else {
                ellipse(&mut c, [x0, y0, x1, y1]);
            }
            c.push_str(if inside.is_some() { " B\n" } else { " S\n" });
        }
        AnnotKind::Line { from, to, arrow } => {
            let _ = writeln!(
                c,
                "{stroke} {w:.2} w 1 J 1 j {} m {} l S",
                pt(*from),
                pt(*to)
            );
            if *arrow {
                let len = dist(*from, *to).max(0.001);
                let d = [(to[0] - from[0]) / len, (to[1] - from[1]) / len];
                let n = [-d[1], d[0]];
                let s = arrow_size(w);
                let back = [to[0] - d[0] * s, to[1] - d[1] * s];
                let left = [back[0] + n[0] * s * 0.5, back[1] + n[1] * s * 0.5];
                let right = [back[0] - n[0] * s * 0.5, back[1] - n[1] * s * 0.5];
                let _ = writeln!(c, "{} m {} l {} l S", pt(left), pt(*to), pt(right));
            }
        }
        AnnotKind::Polygon(points) | AnnotKind::PolyLine(points) => {
            let _ = write!(c, "{stroke} {w:.2} w 1 J 1 j");
            for (i, p) in points.iter().enumerate() {
                let _ = write!(c, " {} {}", pt(*p), if i == 0 { "m" } else { "l" });
            }
            c.push_str(if matches!(a.kind, AnnotKind::Polygon(_)) {
                " h S\n"
            } else {
                " S\n"
            });
        }
        AnnotKind::Stamp(name) => {
            font = true;
            let [x0, y0, x1, y1] = a.rect;
            let (bw, bh) = (x1 - x0, y1 - y0);
            let label = stamp_label(name);
            // Helvetica Bold capitals are about 0.72 em wide.
            let size =
                (bh * 0.55).min((bw - 8.0).max(1.0) / (label.chars().count().max(1) as f32 * 0.72));
            let tw = label.chars().count() as f32 * size * 0.72;
            let _ = writeln!(
                c,
                "{stroke} 2 w {:.2} {:.2} {:.2} {:.2} re S BT /HeBo {size:.2} Tf {fill} {:.2} {:.2} \
                 Td {} Tj ET",
                x0 + 1.0,
                y0 + 1.0,
                bw - 2.0,
                bh - 2.0,
                x0 + (bw - tw) / 2.0,
                y0 + (bh - size * 0.72) / 2.0,
                literal(&label)
            );
        }
        AnnotKind::Other(_) => {}
    }
    let mut resources = Dict::new();
    if a.opacity < 1.0 || multiply {
        let mut gs = Dict::new();
        gs.set("Type", Object::name("ExtGState"));
        gs.set("CA", Object::Real(f64::from(a.opacity)));
        gs.set("ca", Object::Real(f64::from(a.opacity)));
        if multiply {
            gs.set("BM", Object::name("Multiply"));
        }
        resources.set(
            "ExtGState",
            Dict(vec![(Name::new("GS0"), Object::Dict(gs))]),
        );
        if !c.starts_with("/GS0") {
            c.insert_str(0, "/GS0 gs\n");
        }
    }
    if font {
        let helvetica = |base: &str| {
            Object::Dict(Dict(vec![
                (Name::new("Type"), Object::name("Font")),
                (Name::new("Subtype"), Object::name("Type1")),
                (Name::new("BaseFont"), Object::name(base)),
                (Name::new("Encoding"), Object::name("WinAnsiEncoding")),
            ]))
        };
        resources.set(
            "Font",
            Dict(vec![
                (Name::new("Helv"), helvetica("Helvetica")),
                (Name::new("HeBo"), helvetica("Helvetica-Bold")),
            ]),
        );
    }
    let data = c.into_bytes();
    Object::Stream(Stream {
        dict: Dict(vec![
            (Name::new("Type"), Object::name("XObject")),
            (Name::new("Subtype"), Object::name("Form")),
            (Name::new("BBox"), reals(&a.rect)),
            (Name::new("Resources"), Object::Dict(resources)),
            (Name::new("Length"), Object::Int(data.len() as i64)),
        ]),
        data,
    })
}

/// What a stamp says: its name, split before capitals and capitalised
/// (`NotApproved` is "NOT APPROVED").
pub fn stamp_label(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.extend(ch.to_uppercase());
    }
    out
}

fn arrow_size(width: f32) -> f32 {
    (width * 4.0).max(6.0)
}

/// A quadrilateral's corners: top left, top right, bottom left, bottom
/// right.
fn corners(q: &[f32; 8]) -> [[f32; 2]; 4] {
    [[q[0], q[1]], [q[2], q[3]], [q[4], q[5]], [q[6], q[7]]]
}

fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn pt(p: [f32; 2]) -> String {
    format!("{:.2} {:.2}", p[0], p[1])
}

/// An ellipse in `b` as four Bézier curves.
fn ellipse(c: &mut String, b: [f32; 4]) {
    const K: f32 = 0.552_284_8;
    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let (rx, ry) = ((b[2] - b[0]) / 2.0, (b[3] - b[1]) / 2.0);
    let (kx, ky) = (rx * K, ry * K);
    let _ = write!(
        c,
        "{:.2} {cy:.2} m {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} \
         {cy:.2} c {:.2} {:.2} {:.2} {:.2} {cx:.2} {:.2} c {:.2} {:.2} {:.2} {:.2} {:.2} {cy:.2} c h",
        cx + rx,
        cx + rx,
        cy + ky,
        cx + kx,
        cy + ry,
        cy + ry,
        cx - kx,
        cy + ry,
        cx - rx,
        cy + ky,
        cx - rx,
        cx - rx,
        cy - ky,
        cx - kx,
        cy - ry,
        cy - ry,
        cx + kx,
        cy - ry,
        cx + rx,
        cy - ky,
        cx + rx,
    );
}

fn reals(values: &[f32]) -> Object {
    Object::Array(
        values
            .iter()
            .map(|v| Object::Real((f64::from(*v) * 100.0).round() / 100.0))
            .collect(),
    )
}

/// `r` with its corners in order: left, bottom, right, top.
fn normal(r: [f32; 4]) -> [f32; 4] {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

/// A name no other annotation has.
fn unique_name() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("katna-{nanos:x}-{}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// `time` as a PDF date in UTC (`D:YYYYMMDDHHmmSSZ`).
pub fn pdf_date(time: std::time::SystemTime) -> String {
    let secs = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "D:{year:04}{month:02}{day:02}{:02}{:02}{:02}Z",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_stamp_labels() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_327_845);
        assert_eq!(pdf_date(t), "D:20261018125045Z");
        assert_eq!(stamp_label("NotApproved"), "NOT APPROVED");
    }
}
