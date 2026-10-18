// SPDX-License-Identifier: GPL-3.0-or-later

//! Annotating (docs/PLAN.md §4.2): the tools, and the annotations they
//! make from what the pointer did. Places come in points from the top
//! left of the page shown, as the pointer gives them; annotations are in
//! the page's own space, which [`Inverse`] maps back to.

use katna_pdf_cos::{AnnotKind, Annotation, MarkupStyle, NOTE_SIZE};

/// A tool for annotating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Note,
    FreeText,
    Ink,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Stamp,
}

impl Tool {
    pub const ALL: [Tool; 12] = [
        Tool::Highlight,
        Tool::Underline,
        Tool::StrikeOut,
        Tool::Squiggly,
        Tool::Note,
        Tool::FreeText,
        Tool::Ink,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Stamp,
    ];

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Highlight => "highlight",
            Tool::Underline => "format-underline",
            Tool::StrikeOut => "format-strike",
            Tool::Squiggly => "format-squiggle",
            Tool::Note => "notes",
            Tool::FreeText => "format-text",
            Tool::Ink => "ink",
            Tool::Rectangle => "shape-square",
            Tool::Ellipse => "shape-circle",
            Tool::Line => "shape-line",
            Tool::Arrow => "shape-arrow",
            Tool::Stamp => "stamp",
        }
    }

    /// The message naming it.
    pub fn tip(self) -> &'static str {
        match self {
            Tool::Highlight => "tool-highlight",
            Tool::Underline => "tool-underline",
            Tool::StrikeOut => "tool-strikeout",
            Tool::Squiggly => "tool-squiggly",
            Tool::Note => "tool-note",
            Tool::FreeText => "tool-free-text",
            Tool::Ink => "tool-ink",
            Tool::Rectangle => "tool-rectangle",
            Tool::Ellipse => "tool-ellipse",
            Tool::Line => "tool-line",
            Tool::Arrow => "tool-arrow",
            Tool::Stamp => "tool-stamp",
        }
    }

    /// The text markup it makes, for the tools that mark selected text.
    pub fn markup(self) -> Option<MarkupStyle> {
        match self {
            Tool::Highlight => Some(MarkupStyle::Highlight),
            Tool::Underline => Some(MarkupStyle::Underline),
            Tool::StrikeOut => Some(MarkupStyle::StrikeOut),
            Tool::Squiggly => Some(MarkupStyle::Squiggly),
            _ => None,
        }
    }
}

/// The colours to pick from, red, green, blue: yellow, green, blue, pink,
/// red, black.
pub const COLOURS: [[f32; 3]; 6] = [
    [1.0, 0.85, 0.0],
    [0.3, 0.8, 0.35],
    [0.25, 0.55, 1.0],
    [1.0, 0.45, 0.75],
    [0.9, 0.15, 0.15],
    [0.0, 0.0, 0.0],
];

/// Line width of ink, shapes and lines, in points.
pub const WIDTH: f32 = 2.0;
/// Text size of free text, in points.
pub const TEXT_SIZE: f32 = 12.0;
/// A free text box or stamp placed with a click, not dragged out.
const FREE_TEXT_SIZE: [f32; 2] = [200.0, 40.0];
const STAMP_SIZE: [f32; 2] = [150.0, 50.0];
/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// The map from the page shown back to the page's own space: the
/// inverse of a page's transform ([`katna_pdf_render::map_point`]).
#[derive(Debug, Clone, Copy)]
pub struct Inverse([f64; 6]);

impl Inverse {
    pub fn of(t: [f64; 6]) -> Option<Self> {
        let [a, b, c, d, e, f] = t;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Inverse([
            ia,
            ib,
            ic,
            id,
            -(ia * e + ic * f),
            -(ib * e + id * f),
        ]))
    }

    pub fn map(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        let [a, b, c, d, e, f] = self.0;
        let (x, y) = (f64::from(x), f64::from(y));
        [(a * x + c * y + e) as f32, (b * x + d * y + f) as f32]
    }

    /// A box on the page shown (left, top, right, bottom) in page space
    /// (left, bottom, right, top).
    pub fn rect(&self, b: [f32; 4]) -> [f32; 4] {
        let corners = [[b[0], b[1]], [b[2], b[1]], [b[0], b[3]], [b[2], b[3]]].map(|p| self.map(p));
        corners.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |r, p| {
                [
                    r[0].min(p[0]),
                    r[1].min(p[1]),
                    r[2].max(p[0]),
                    r[3].max(p[1]),
                ]
            },
        )
    }
}

/// Text markup over the lines of selected text, each a box on the page
/// shown.
pub fn markup(
    style: MarkupStyle,
    page: usize,
    lines: &[[f32; 4]],
    inv: Inverse,
) -> Option<Annotation> {
    if lines.is_empty() {
        return None;
    }
    let quads = lines
        .iter()
        .map(|&[l, t, r, b]| {
            let [tl, tr, bl, br] = [[l, t], [r, t], [l, b], [r, b]].map(|p| inv.map(p));
            [tl[0], tl[1], tr[0], tr[1], bl[0], bl[1], br[0], br[1]]
        })
        .collect();
    Some(Annotation::new(page, AnnotKind::Markup(style, quads)))
}

/// What `tool` makes of the pointer's path on page `page`: pressed at the
/// first point, released at the last. `None` when it makes nothing (a
/// click with a tool that needs a drag).
pub fn drawn(tool: Tool, page: usize, path: &[[f32; 2]], inv: Inverse) -> Option<Annotation> {
    let (&from, &to) = (path.first()?, path.last()?);
    let click = (to[0] - from[0]).hypot(to[1] - from[1]) < CLICK;
    let boxed = |size: [f32; 2]| {
        if click {
            [from[0], from[1], from[0] + size[0], from[1] + size[1]]
        } else {
            [
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ]
        }
    };
    let kind = match tool {
        Tool::Highlight | Tool::Underline | Tool::StrikeOut | Tool::Squiggly => return None,
        Tool::Note => AnnotKind::Note,
        Tool::FreeText => AnnotKind::FreeText { size: TEXT_SIZE },
        Tool::Ink => AnnotKind::Ink(vec![path.iter().map(|p| inv.map(*p)).collect()]),
        Tool::Rectangle if !click => AnnotKind::Square { fill: None },
        Tool::Ellipse if !click => AnnotKind::Circle { fill: None },
        Tool::Line | Tool::Arrow if !click => AnnotKind::Line {
            from: inv.map(from),
            to: inv.map(to),
            arrow: tool == Tool::Arrow,
        },
        Tool::Stamp => AnnotKind::Stamp("Approved".into()),
        Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow => return None,
    };
    let mut a = Annotation::new(page, kind);
    a.width = WIDTH;
    a.rect = match tool {
        // The icon's top left where the click was.
        Tool::Note => inv.rect([from[0], from[1], from[0] + NOTE_SIZE, from[1] + NOTE_SIZE]),
        Tool::FreeText => inv.rect(boxed(FREE_TEXT_SIZE)),
        Tool::Stamp => inv.rect(boxed(STAMP_SIZE)),
        Tool::Rectangle | Tool::Ellipse => inv.rect(boxed([0.0, 0.0])),
        _ => a.bounds(),
    };
    Some(a)
}

/// The annotation at `point` (page space) among `annots`: the last
/// drawn, so the one on top.
pub fn at(annots: &[Annotation], point: [f32; 2]) -> Option<usize> {
    annots.iter().rposition(|a| {
        let r = a.rect;
        point[0] >= r[0] && point[0] <= r[2] && point[1] >= r[1] && point[1] <= r[3]
    })
}

/// What the comments list calls an annotation: its comment, or what
/// kind it is when it has none.
pub fn kind_message(kind: &AnnotKind) -> &'static str {
    match kind {
        AnnotKind::Markup(MarkupStyle::Highlight, _) => "tool-highlight",
        AnnotKind::Markup(MarkupStyle::Underline, _) => "tool-underline",
        AnnotKind::Markup(MarkupStyle::StrikeOut, _) => "tool-strikeout",
        AnnotKind::Markup(MarkupStyle::Squiggly, _) => "tool-squiggly",
        AnnotKind::Note => "tool-note",
        AnnotKind::FreeText { .. } => "tool-free-text",
        AnnotKind::Ink(_) => "tool-ink",
        AnnotKind::Square { .. } => "tool-rectangle",
        AnnotKind::Circle { .. } => "tool-ellipse",
        AnnotKind::Line { arrow: false, .. } => "tool-line",
        AnnotKind::Line { arrow: true, .. } => "tool-arrow",
        AnnotKind::Polygon(_) | AnnotKind::PolyLine(_) => "annot-shape",
        AnnotKind::Stamp(_) => "tool-stamp",
        AnnotKind::Other(_) => "annot-other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 600 × 800 page shown upright: y flips.
    fn upright() -> Inverse {
        Inverse::of([1.0, 0.0, 0.0, -1.0, 0.0, 800.0]).unwrap()
    }

    #[test]
    fn places_map_back_to_the_page() {
        let inv = upright();
        assert_eq!(inv.map([10.0, 30.0]), [10.0, 770.0]);
        // Turned a quarter turn: x on screen is y on the page.
        let turned = Inverse::of([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]).unwrap();
        assert_eq!(turned.map([5.0, 7.0]), [7.0, 5.0]);
        assert!(Inverse::of([0.0; 6]).is_none());
    }

    #[test]
    fn selected_lines_become_quads() {
        let a = markup(
            MarkupStyle::Highlight,
            2,
            &[[10.0, 20.0, 110.0, 32.0]],
            upright(),
        )
        .unwrap();
        assert_eq!(a.page, 2);
        assert_eq!(
            a.kind,
            AnnotKind::Markup(
                MarkupStyle::Highlight,
                vec![[10.0, 780.0, 110.0, 780.0, 10.0, 768.0, 110.0, 768.0]]
            )
        );
        assert!(markup(MarkupStyle::Underline, 0, &[], upright()).is_none());
    }

    #[test]
    fn drags_and_clicks_make_annotations() {
        let inv = upright();
        let drag = [[10.0, 10.0], [60.0, 50.0]];
        let click = [[10.0, 10.0], [11.0, 10.0]];
        let square = drawn(Tool::Rectangle, 0, &drag, inv).unwrap();
        assert_eq!(square.rect, [10.0, 750.0, 60.0, 790.0]);
        assert!(drawn(Tool::Rectangle, 0, &click, inv).is_none());
        assert!(drawn(Tool::Highlight, 0, &drag, inv).is_none());
        // A note's icon hangs from the click.
        let note = drawn(Tool::Note, 0, &click, inv).unwrap();
        assert_eq!(note.rect, [10.0, 770.0, 30.0, 790.0]);
        let stamp = drawn(Tool::Stamp, 0, &click, inv).unwrap();
        assert_eq!(stamp.rect, [10.0, 740.0, 160.0, 790.0]);
        let arrow = drawn(Tool::Arrow, 0, &drag, inv).unwrap();
        assert_eq!(
            arrow.kind,
            AnnotKind::Line {
                from: [10.0, 790.0],
                to: [60.0, 750.0],
                arrow: true
            }
        );
        let ink = drawn(Tool::Ink, 0, &[[1.0, 1.0], [2.0, 3.0], [4.0, 2.0]], inv).unwrap();
        assert_eq!(
            ink.kind,
            AnnotKind::Ink(vec![vec![[1.0, 799.0], [2.0, 797.0], [4.0, 798.0]]])
        );
    }

    #[test]
    fn the_top_annotation_is_hit() {
        let mut a = Annotation::new(0, AnnotKind::Note);
        a.rect = [0.0, 0.0, 50.0, 50.0];
        let mut b = a.clone();
        b.rect = [25.0, 25.0, 75.0, 75.0];
        let annots = [a, b];
        assert_eq!(at(&annots, [30.0, 30.0]), Some(1));
        assert_eq!(at(&annots, [10.0, 10.0]), Some(0));
        assert_eq!(at(&annots, [90.0, 90.0]), None);
    }
}
