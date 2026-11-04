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
use crate::measure::Scale;
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::outline::text_string;
use crate::write::UpdateError;

/// A note's icon, in points.
pub const NOTE_SIZE: f32 = 20.0;
/// The size of a measurement's value written beside it.
const LABEL_SIZE: f32 = 9.0;

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
    /// A file kept in the document (`/FileAttachment`): its name, and
    /// its bytes when it is being added (`None` once read; see
    /// [`Document::attached_file`]).
    FileAttachment { name: String, data: Option<Vec<u8>> },
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
    /// The name (`/NM`) of the annotation this one replies to, when that
    /// one is not in the file yet but added in the same change; the
    /// reply is written pointing at it.
    pub answers: Option<String>,
    /// Its name, unique in the file (`/NM`); empty for one not yet
    /// written, which is given one.
    pub name: String,
    /// For a line or shape that measures: the scale it measures in. Its
    /// comment is the length or area, shown beside it.
    pub measure: Option<Scale>,
    /// For free text that points at something (a callout): the line from
    /// what it points at, by a knee, to its box, in page space. Its
    /// `rect` is the text box alone; the line is drawn outside it.
    pub callout: Option<[[f32; 2]; 3]>,
    /// A polygon or rectangle drawn with a cloudy edge (`/BE`).
    pub cloudy: bool,
    /// Hidden, printed, locked (`/F`).
    pub flags: AnnotFlags,
    /// For a reply that sets its parent's review state: `Accepted`,
    /// `Rejected`, `Cancelled`, `Completed` or `None` (`/State`). Such a
    /// reply is not a comment to read.
    pub state: Option<String>,
}

/// An annotation's flags (`/F`): whether it is shown, printed and may
/// be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotFlags {
    /// Not drawn on screen (`Hidden` or `NoView`).
    pub hidden: bool,
    /// Printed with the page (`Print`).
    pub print: bool,
    /// Not to be moved, deleted or its comment changed (`Locked` and
    /// `LockedContents`).
    pub locked: bool,
}

impl Default for AnnotFlags {
    fn default() -> Self {
        AnnotFlags {
            hidden: false,
            print: true,
            locked: false,
        }
    }
}

impl AnnotFlags {
    const HIDDEN: i64 = 2;
    const PRINT: i64 = 4;
    const NO_VIEW: i64 = 32;
    const LOCKED: i64 = 128;
    const LOCKED_CONTENTS: i64 = 512;

    /// From the file's bits.
    pub fn from_bits(bits: i64) -> Self {
        AnnotFlags {
            hidden: bits & (Self::HIDDEN | Self::NO_VIEW) != 0,
            print: bits & Self::PRINT != 0,
            locked: bits & (Self::LOCKED | Self::LOCKED_CONTENTS) != 0,
        }
    }

    /// The file's bits, keeping those of `was` these flags do not cover.
    pub fn bits(self, was: i64) -> i64 {
        let keep = was
            & !(Self::HIDDEN | Self::NO_VIEW | Self::PRINT | Self::LOCKED | Self::LOCKED_CONTENTS);
        keep | if self.hidden { Self::HIDDEN } else { 0 }
            | if self.print { Self::PRINT } else { 0 }
            | if self.locked {
                Self::LOCKED | Self::LOCKED_CONTENTS
            } else {
                0
            }
    }
}

/// The review states a reply can set on what it replies to.
pub const REVIEW_STATES: [&str; 4] = ["Accepted", "Rejected", "Cancelled", "Completed"];

/// The review state of annotation `id`: that of its latest state reply,
/// if any sets one.
pub fn annotation_status(all: &[Vec<Annotation>], id: Ref) -> Option<&str> {
    all.iter()
        .flatten()
        .filter(|a| a.reply_to == Some(id))
        .filter_map(|a| Some((a.modified.as_deref().unwrap_or(""), a.state.as_deref()?)))
        .filter(|(_, s)| *s != "None")
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, s)| s)
}

/// The comments as text to read or paste: page by page, each
/// annotation with its kind, author, date, state and comment, its
/// replies indented under it.
pub fn comments_text(all: &[Vec<Annotation>]) -> String {
    let mut out = String::new();
    for (page, annots) in all.iter().enumerate() {
        let tops: Vec<&Annotation> = annots
            .iter()
            .filter(|a| a.reply_to.is_none() && a.state.is_none())
            .collect();
        if tops.is_empty() {
            continue;
        }
        let _ = writeln!(out, "Page {}", page + 1);
        for a in tops {
            comment_line(&mut out, a, 2);
            if let Some(status) = a.id.and_then(|id| annotation_status(all, id)) {
                let _ = writeln!(out, "    [{status}]");
            }
            for reply in annots
                .iter()
                .filter(|r| r.state.is_none() && r.reply_to.is_some() && r.reply_to == a.id)
            {
                comment_line(&mut out, reply, 4);
            }
        }
        out.push('\n');
    }
    out
}

fn comment_line(out: &mut String, a: &Annotation, indent: usize) {
    let _ = write!(out, "{:indent$}{}", "", a.kind.label());
    if !a.author.is_empty() {
        let _ = write!(out, " by {}", a.author);
    }
    if let Some(date) = a.modified.as_deref().and_then(plain_date) {
        let _ = write!(out, " on {date}");
    }
    if a.contents.is_empty() {
        out.push('\n');
    } else {
        let _ = writeln!(
            out,
            ": {}",
            a.contents.replace('\n', &format!("\n{:indent$}  ", ""))
        );
    }
}

/// `D:20261018123000Z` as `2026-10-18 12:30`.
fn plain_date(d: &str) -> Option<String> {
    let d = d.strip_prefix("D:").unwrap_or(d);
    if d.len() < 8 || !d.as_bytes()[..8].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut out = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]);
    if d.len() >= 12 && d.as_bytes()[8..12].iter().all(u8::is_ascii_digit) {
        let _ = write!(out, " {}:{}", &d[8..10], &d[10..12]);
    }
    Some(out)
}

impl AnnotKind {
    /// What kind it is, as a word.
    pub fn label(&self) -> String {
        match self {
            AnnotKind::Markup(style, _) => format!("{style:?}"),
            AnnotKind::Note => "Note".into(),
            AnnotKind::FreeText { .. } => "Text".into(),
            AnnotKind::Ink(_) => "Ink".into(),
            AnnotKind::Square { .. } => "Rectangle".into(),
            AnnotKind::Circle { .. } => "Ellipse".into(),
            AnnotKind::Line { arrow: true, .. } => "Arrow".into(),
            AnnotKind::Line { .. } => "Line".into(),
            AnnotKind::Polygon(_) => "Polygon".into(),
            AnnotKind::PolyLine(_) => "Polyline".into(),
            AnnotKind::Stamp(name) => format!("Stamp {}", stamp_label(name)),
            AnnotKind::FileAttachment { name, .. } => format!("Attachment {name}"),
            AnnotKind::Other(subtype) => subtype.clone(),
        }
    }
}

/// The standard stamps, by the names the file gives them.
pub const STAMPS: [&str; 14] = [
    "Approved",
    "NotApproved",
    "Draft",
    "Final",
    "Confidential",
    "ForComment",
    "ForPublicRelease",
    "NotForPublicRelease",
    "Departmental",
    "Experimental",
    "Expired",
    "AsIs",
    "Sold",
    "TopSecret",
];

/// The dynamic stamps: each shows who put it on and when, under its
/// word, as Acrobat's do (the names Acrobat gives them).
pub const DYNAMIC_STAMPS: [&str; 5] = [
    "#DApproved",
    "#DReviewed",
    "#DReceived",
    "#DRevised",
    "#DConfidential",
];

/// The name of a stamp showing `text` as typed.
pub fn custom_stamp(text: &str) -> String {
    format!("#C{text}")
}

/// Whether the stamp named `name` shows who put it on and when.
pub fn stamp_is_dynamic(name: &str) -> bool {
    name.starts_with("#D")
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
            answers: None,
            name: String::new(),
            measure: None,
            callout: None,
            cloudy: false,
            flags: AnnotFlags::default(),
            state: None,
        };
        a.rect = a.bounds();
        a
    }

    /// The box the annotation takes on the page: its `rect`, and for a
    /// callout the line too, with its arrowhead.
    pub fn outer_rect(&self) -> [f32; 4] {
        let r = normal(self.rect);
        let Some(line) = self.callout else {
            return r;
        };
        let grow = self.width * 0.5 + arrow_size(self.width);
        let mut b = r;
        for p in line {
            b = [
                b[0].min(p[0] - grow),
                b[1].min(p[1] - grow),
                b[2].max(p[0] + grow),
                b[3].max(p[1] + grow),
            ];
        }
        b
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
            AnnotKind::Polygon(_) if self.cloudy => self.width * 0.5 + cloud_radius(self.width),
            _ => self.width * 0.5 + 1.0,
        };
        let b = [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow];
        match self.label_box() {
            Some(l) => [
                b[0].min(l[0]),
                b[1].min(l[1]),
                b[2].max(l[2]),
                b[3].max(l[3]),
            ],
            None => b,
        }
    }

    /// Where a measurement's value is written: a box in page space, its
    /// text at [`LABEL_SIZE`] points.
    fn label_box(&self) -> Option<[f32; 4]> {
        if self.measure.is_none() || self.contents.is_empty() {
            return None;
        }
        let at = match &self.kind {
            AnnotKind::Line { from, to, .. } => lerp(*from, *to, 0.5),
            AnnotKind::PolyLine(points) => *points.get(points.len() / 2)?,
            AnnotKind::Polygon(points) if !points.is_empty() => {
                let n = points.len() as f32;
                let sum = points
                    .iter()
                    .fold([0.0, 0.0], |s, p| [s[0] + p[0], s[1] + p[1]]);
                [sum[0] / n, sum[1] / n]
            }
            _ => return None,
        };
        // Helvetica's figures are about 0.56 em wide.
        let w = self.contents.chars().count() as f32 * LABEL_SIZE * 0.56 + 4.0;
        let h = LABEL_SIZE + 4.0;
        // Above the line, centred; in the middle of a shape.
        let bottom = match self.kind {
            AnnotKind::Polygon(_) => at[1] - h / 2.0,
            _ => at[1] + self.width,
        };
        Some([at[0] - w / 2.0, bottom, at[0] + w / 2.0, bottom + h])
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

    /// The bytes of the file annotation `a` keeps.
    pub fn attached_file(&self, a: &Annotation) -> Option<Vec<u8>> {
        if let AnnotKind::FileAttachment {
            data: Some(data), ..
        } = &a.kind
        {
            return Some(data.clone());
        }
        let Some(Object::Dict(d)) = self.get(a.id?) else {
            return None;
        };
        let Object::Dict(fs) = self.resolve_key(&d, "FS") else {
            return None;
        };
        let Object::Dict(ef) = self.resolve_key(&fs, "EF") else {
            return None;
        };
        let Object::Stream(f) = ["UF", "F"]
            .into_iter()
            .map(|key| self.resolve_key(&ef, key))
            .find(|o| matches!(o, Object::Stream(_)))?
        else {
            return None;
        };
        crate::filter::decode(&f.dict, &f.data).ok()
    }

    pub(crate) fn read_annotation(&self, page: usize, obj: &Object) -> Option<Annotation> {
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
            "FileAttachment" => AnnotKind::FileAttachment {
                name: match self.resolve_key(&d, "FS") {
                    Object::Dict(fs) => ["UF", "F"]
                        .into_iter()
                        .find_map(|key| match self.resolve_key(&fs, key) {
                            Object::String { bytes, .. } => Some(text_string(&bytes)),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    _ => String::new(),
                },
                data: None,
            },
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
        let callout = match numbers("CL").as_slice() {
            [x0, y0, x1, y1, x2, y2] => Some([[*x0, *y0], [*x1, *y1], [*x2, *y2]]),
            [x0, y0, x1, y1] => Some([[*x0, *y0], lerp([*x0, *y0], [*x1, *y1], 0.5), [*x1, *y1]]),
            _ => None,
        }
        .filter(|_| matches!(kind, AnnotKind::FreeText { .. }));
        let cloudy = match self.resolve_key(&d, "BE") {
            Object::Dict(be) => be.name("S") == Some(b"C"),
            _ => false,
        };
        let mut rect = match numbers("Rect").as_slice() {
            [a, b, c, d] => normal([*a, *b, *c, *d]),
            _ => [0.0; 4],
        };
        // A callout's text box is the rectangle less the differences.
        if callout.is_some()
            && let [l, b, r, t] = numbers("RD").as_slice()
        {
            rect = normal([rect[0] + l, rect[1] + b, rect[2] - r, rect[3] - t]);
        }
        Some(Annotation {
            id: obj.as_ref(),
            page,
            rect,
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
            answers: None,
            name: text("NM").unwrap_or_default(),
            measure: match &kind {
                AnnotKind::Line { .. } | AnnotKind::Polygon(_) | AnnotKind::PolyLine(_) => {
                    d.get("Measure").and_then(|m| self.read_scale(m))
                }
                _ => None,
            },
            callout,
            cloudy,
            flags: AnnotFlags::from_bits(
                self.resolve_key(&d, "F").as_number().unwrap_or(4.0) as i64
            ),
            state: text("State").filter(|_| d.get("IRT").is_some()),
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
        // New annotations get their numbers first, so replies added with
        // what they answer can point at it; by name.
        let mut new_ids: Vec<Option<Ref>> = vec![None; changes.put.len()];
        let mut names: BTreeMap<&str, Ref> = existing
            .values()
            .filter(|a| !a.name.is_empty())
            .filter_map(|a| Some((a.name.as_str(), a.id?)))
            .collect();
        for (i, a) in changes.put.iter().enumerate() {
            if a.id.is_none()
                && pages.get(a.page).is_some()
                && !matches!(a.kind, AnnotKind::Other(_))
            {
                new_ids[i] = Some(update.add(Object::Null));
            }
            if let Some(id) = a.id.or(new_ids[i])
                && !a.name.is_empty()
            {
                names.insert(a.name.as_str(), id);
            }
        }
        // Each page's list of annotations, as it will be.
        let mut lists: BTreeMap<usize, Vec<Object>> = BTreeMap::new();
        for (i, annot) in changes.put.iter().enumerate() {
            let Some(&page_ref) = pages.get(annot.page) else {
                continue;
            };
            let mut annot = annot.clone();
            if annot.reply_to.is_none()
                && let Some(to) = annot.answers.as_deref().and_then(|n| names.get(n))
            {
                annot.reply_to = Some(*to);
            }
            annot.modified = Some(date.clone());
            if !matches!(
                annot.kind,
                AnnotKind::Note
                    | AnnotKind::FreeText { .. }
                    | AnnotKind::Square { .. }
                    | AnnotKind::Circle { .. }
                    | AnnotKind::Stamp(_)
                    | AnnotKind::FileAttachment { .. }
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
            if let AnnotKind::FileAttachment {
                name,
                data: Some(data),
            } = &annot.kind
            {
                // The file itself, as an embedded file stream in a file
                // specification.
                let mut params = Dict::new();
                params.set("Size", data.len() as i64);
                let mut ef_dict = Dict::new();
                ef_dict.set("Type", Object::name("EmbeddedFile"));
                ef_dict.set("Params", Object::Dict(params));
                let ef = update.add(Object::Stream(Stream {
                    dict: ef_dict,
                    data: data.clone(),
                }));
                let mut fs = Dict::new();
                fs.set("Type", Object::name("Filespec"));
                fs.set("F", text_object(name));
                fs.set("UF", text_object(name));
                fs.set("EF", Dict(vec![(Name::new("F"), Object::Ref(ef))]));
                dict.set("FS", Object::Dict(fs));
            }
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
                    let id = match new_ids[i] {
                        Some(id) => {
                            update.replace(id, Object::Dict(dict));
                            id
                        }
                        None => update.add(Object::Dict(dict)),
                    };
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
pub(crate) fn write_dict(dict: &mut Dict, a: &Annotation, page: Ref) {
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
        AnnotKind::FileAttachment { .. } => "FileAttachment",
        AnnotKind::Other(_) => "",
    };
    dict.set("Type", Object::name("Annot"));
    if !subtype.is_empty() {
        dict.set("Subtype", Object::name(subtype));
    }
    let outer = a.outer_rect();
    dict.set("Rect", reals(&outer));
    if a.callout.is_some() {
        let r = normal(a.rect);
        dict.set(
            "RD",
            reals(&[
                r[0] - outer[0],
                r[1] - outer[1],
                outer[2] - r[2],
                outer[3] - r[3],
            ]),
        );
    } else {
        dict.remove("RD");
    }
    dict.set("P", page);
    let was = dict.get("F").and_then(Object::as_number).unwrap_or(0.0) as i64;
    dict.set("F", a.flags.bits(was));
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
    match (&a.state, a.reply_to) {
        (Some(state), Some(_)) => {
            dict.set("State", Object::string(state));
            dict.set(
                "StateModel",
                Object::name(if state == "Marked" || state == "Unmarked" {
                    "Marked"
                } else {
                    "Review"
                }),
            );
        }
        _ => {
            dict.remove("State");
            dict.remove("StateModel");
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
            match a.callout {
                Some(line) => {
                    dict.set("IT", Object::name("FreeTextCallout"));
                    dict.set("CL", reals(line.as_flattened()));
                    dict.set("LE", Object::name("OpenArrow"));
                    border(dict);
                }
                None => {
                    for key in ["IT", "CL", "LE"] {
                        dict.remove(key);
                    }
                }
            }
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
        AnnotKind::FileAttachment { .. } => {
            dict.set("Name", Object::name("Paperclip"));
        }
        AnnotKind::Other(_) => {}
    }
    if a.cloudy && matches!(a.kind, AnnotKind::Polygon(_) | AnnotKind::Square { .. }) {
        dict.set(
            "BE",
            Dict(vec![
                (Name::new("S"), Object::name("C")),
                (Name::new("I"), Object::Int(1)),
            ]),
        );
    } else {
        dict.remove("BE");
    }
    match (&a.measure, &a.kind) {
        (Some(scale), AnnotKind::Line { .. } | AnnotKind::Polygon(_) | AnnotKind::PolyLine(_)) => {
            dict.set(
                "IT",
                Object::name(match a.kind {
                    AnnotKind::Line { .. } => "LineDimension",
                    AnnotKind::Polygon(_) => "PolygonDimension",
                    _ => "PolyLineDimension",
                }),
            );
            dict.set("Measure", scale.dict());
            if let AnnotKind::Line { .. } = a.kind {
                // The length is shown on the line.
                dict.set("Cap", true);
            }
        }
        (_, AnnotKind::FreeText { .. }) => {
            // Its intent is the callout's, set above.
            for key in ["Measure", "Cap"] {
                dict.remove(key);
            }
        }
        _ => {
            for key in ["IT", "Measure", "Cap"] {
                dict.remove(key);
            }
        }
    }
}

/// The appearance stream: a form drawn in page space, its box the
/// annotation's rectangle.
pub(crate) fn appearance(a: &Annotation) -> Object {
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
            if let Some([from, knee, to]) = a.callout {
                // The line from what it points at to the box, with an
                // arrowhead at what it points at, and the box's edge.
                let _ = writeln!(
                    c,
                    "{stroke} {w:.2} w 1 J 1 j {} m {} l {} l S",
                    pt(from),
                    pt(knee),
                    pt(to)
                );
                let len = dist(from, knee).max(0.001);
                let d = [(knee[0] - from[0]) / len, (knee[1] - from[1]) / len];
                let n = [-d[1], d[0]];
                let s = arrow_size(w);
                let back = [from[0] + d[0] * s, from[1] + d[1] * s];
                let left = [back[0] + n[0] * s * 0.5, back[1] + n[1] * s * 0.5];
                let right = [back[0] - n[0] * s * 0.5, back[1] - n[1] * s * 0.5];
                let _ = writeln!(c, "{} m {} l {} l S", pt(left), pt(from), pt(right));
                let _ = writeln!(
                    c,
                    "1 1 1 rg {x0:.2} {y0:.2} {:.2} {:.2} re B",
                    x1 - x0,
                    y1 - y0
                );
            }
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
            if a.cloudy && matches!(a.kind, AnnotKind::Square { .. }) {
                // Bumps along the inside of the box, so they stay in it.
                let r = cloud_radius(w);
                let [x0, y0, x1, y1] = [x0 + r, y0 + r, x1 - r, y1 - r];
                cloud(&mut c, &[[x0, y0], [x1, y0], [x1, y1], [x0, y1]], r);
                c.push_str(if inside.is_some() { " B\n" } else { " S\n" });
            } else if matches!(a.kind, AnnotKind::Square { .. }) {
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
        AnnotKind::Polygon(points) if a.cloudy && points.len() >= 3 => {
            let _ = write!(c, "{stroke} {w:.2} w 1 J 1 j ");
            cloud(&mut c, points, cloud_radius(w));
            c.push_str(" S\n");
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
            // A dynamic stamp says who put it on and when, under its word.
            let under = stamp_is_dynamic(name).then(|| {
                let date = a
                    .modified
                    .as_deref()
                    .and_then(plain_date)
                    .unwrap_or_default();
                match (a.author.is_empty(), date.is_empty()) {
                    (false, false) => format!("{}, {date}", a.author),
                    (false, true) => a.author.clone(),
                    _ => date,
                }
            });
            let under = under.filter(|u| !u.is_empty());
            let (word_h, word_y) = match &under {
                Some(_) => (bh * 0.42, y0 + bh * 0.42),
                None => (bh * 0.55, y0),
            };
            // Helvetica Bold capitals are about 0.72 em wide, mixed case
            // about 0.66.
            let em = if name.starts_with("#C") { 0.66 } else { 0.72 };
            let size =
                word_h.min((bw - 12.0).max(1.0) / (label.chars().count().max(1) as f32 * em));
            let tw = label.chars().count() as f32 * size * em;
            let _ = writeln!(
                c,
                "{stroke} 2 w {:.2} {:.2} {:.2} {:.2} re S BT /HeBo {size:.2} Tf {fill} {:.2} {:.2} \
                 Td {} Tj ET",
                x0 + 1.0,
                y0 + 1.0,
                bw - 2.0,
                bh - 2.0,
                x0 + (bw - tw) / 2.0,
                word_y + (bh - word_y + y0 - size * 0.72) / 2.0,
                literal(&label)
            );
            if let Some(under) = under {
                let size = (bh * 0.2)
                    .min((bw - 8.0).max(1.0) / (under.chars().count().max(1) as f32 * 0.55));
                let tw = under.chars().count() as f32 * size * 0.55;
                let _ = writeln!(
                    c,
                    "BT /Helv {size:.2} Tf {fill} {:.2} {:.2} Td {} Tj ET",
                    x0 + (bw - tw) / 2.0,
                    y0 + (bh * 0.42 - size * 0.72) / 2.0,
                    literal(&under)
                );
            }
        }
        AnnotKind::FileAttachment { .. } => {
            // A paper clip at the top left.
            let (x, top) = (a.rect[0], a.rect[3]);
            let s = NOTE_SIZE / 20.0;
            let at = |px: f32, py: f32| format!("{:.2} {:.2}", x + px * s, top - (20.0 - py) * s);
            let _ = writeln!(
                c,
                "{stroke} 1.5 w 1 J 1 j {} m {} l {} {} {} c {} l {} {} {} c {} l S",
                at(13.0, 17.0),
                at(13.0, 6.0),
                at(13.0, 2.0),
                at(7.0, 2.0),
                at(7.0, 6.0),
                at(7.0, 14.0),
                at(7.0, 17.0),
                at(10.0, 17.0),
                at(10.0, 14.0),
                at(10.0, 7.0)
            );
        }
        AnnotKind::Other(_) => {}
    }
    if let Some([x0, y0, x1, y1]) = a.label_box() {
        font = true;
        // On white, so it reads over the drawing.
        let _ = writeln!(
            c,
            "q 1 1 1 rg {x0:.2} {y0:.2} {:.2} {:.2} re f BT /Helv {LABEL_SIZE:.1} Tf {fill} \
             {:.2} {:.2} Td {} Tj ET Q",
            x1 - x0,
            y1 - y0,
            x0 + 2.0,
            y0 + 2.0 + LABEL_SIZE * 0.2,
            literal(&a.contents)
        );
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
            (Name::new("BBox"), reals(&a.outer_rect())),
            (Name::new("Resources"), Object::Dict(resources)),
            (Name::new("Length"), Object::Int(data.len() as i64)),
        ]),
        data,
    })
}

/// What a stamp says: its name, split before capitals and capitalised
/// (`NotApproved` is "NOT APPROVED").
pub fn stamp_label(name: &str) -> String {
    if let Some(text) = name.strip_prefix("#C") {
        return text.to_owned();
    }
    let name = name.strip_prefix("#D").unwrap_or(name);
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

/// How big a cloudy edge's bumps are, for a line `width` wide.
fn cloud_radius(width: f32) -> f32 {
    (width * 3.0).clamp(6.0, 16.0)
}

/// The closed path through `points` drawn as a cloud: bumps of radius
/// `r` along each side, one at each corner, bulging outward, each an
/// arc from where its circle meets the one before to where it meets
/// the one after. Ends with the path open for the paint operator.
fn cloud(c: &mut String, points: &[[f32; 2]], r: f32) {
    use std::f32::consts::{PI, TAU};
    // Which way round the points go, so the bumps bulge outward.
    let n = points.len();
    let twice_area: f32 = (0..n)
        .map(|i| {
            let (p, q) = (points[i], points[(i + 1) % n]);
            p[0] * q[1] - q[0] * p[1]
        })
        .sum();
    let turn = if twice_area > 0.0 { 1.0 } else { -1.0 };
    let step = r * 1.8;
    let mut centres = Vec::new();
    for i in 0..n {
        let (p, q) = (points[i], points[(i + 1) % n]);
        let bumps = ((dist(p, q) / step).round() as usize).max(1);
        for k in 0..bumps {
            centres.push(lerp(p, q, k as f32 / bumps as f32));
        }
    }
    let m = centres.len();
    for (i, &centre) in centres.iter().enumerate() {
        let prev = centres[(i + m - 1) % m];
        let next = centres[(i + 1) % m];
        let towards = |p: [f32; 2]| (p[1] - centre[1]).atan2(p[0] - centre[0]);
        // Where two circles this far apart meet, as an angle off the
        // line between their centres.
        let meet = |p: [f32; 2]| (dist(p, centre) / (2.0 * r)).min(1.0).acos();
        let from = towards(prev) + turn * meet(prev);
        let to = towards(next) - turn * meet(next);
        // The sweep from one to the other, the outward way round.
        let mut sweep = (to - from).rem_euclid(TAU);
        if turn < 0.0 {
            sweep -= TAU;
        }
        if sweep.abs() < 0.01 {
            sweep = turn * PI;
        }
        arc(c, centre, r, from, from + sweep, i == 0);
    }
    c.push_str(" h");
}

/// An arc of the circle round `centre` of radius `r` from angle `from`
/// to `to` (anticlockwise), as Bézier curves; a move first when `start`,
/// else a line to its start.
fn arc(c: &mut String, centre: [f32; 2], r: f32, from: f32, to: f32, start: bool) {
    let at = |t: f32| [centre[0] + r * t.cos(), centre[1] + r * t.sin()];
    let _ = write!(c, " {} {}", pt(at(from)), if start { "m" } else { "l" });
    let pieces = ((to - from).abs() / (std::f32::consts::FRAC_PI_2))
        .ceil()
        .max(1.0) as usize;
    let sweep = (to - from) / pieces as f32;
    let k = 4.0 / 3.0 * (sweep / 4.0).tan();
    for i in 0..pieces {
        let (a0, a1) = (from + sweep * i as f32, from + sweep * (i + 1) as f32);
        let p0 = at(a0);
        let p3 = at(a1);
        let p1 = [p0[0] - k * r * a0.sin(), p0[1] + k * r * a0.cos()];
        let p2 = [p3[0] + k * r * a1.sin(), p3[1] - k * r * a1.cos()];
        let _ = write!(c, " {} {} {} c", pt(p1), pt(p2), pt(p3));
    }
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

pub(crate) fn reals(values: &[f32]) -> Object {
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
