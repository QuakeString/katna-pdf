// SPDX-License-Identifier: GPL-3.0-or-later

//! Page marks (docs/PLAN.md §4.3, Phase 4): headers and footers,
//! watermarks, Bates numbers and backgrounds (a colour, a picture or
//! both) put on many pages at once, from a dialog opened on the edit
//! row (storyboard E12, E13): the settings on the left, the page as it
//! will look on the right. They are drawn into each page's own content
//! (`katna_pdf_cos::stamp`), so every reader shows and prints them. The
//! dialog lists the marks already on the pages, to change or take off,
//! and keeps the person's own settings as templates.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};
use katna_pdf_cos::{Background, Bates, MarkWhat, PageMark, Ref, Stamp, StampPlace, ranges};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};
use katna_ui::widgets::Tip;

use super::insert::INKS;
use super::sign::check_box;
use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// What kind of mark is being put on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MarkKind {
    Header,
    Footer,
    Watermark,
    Bates,
    Background,
}

/// The kinds, in the order the dialog's chips show them.
const KINDS: [(MarkKind, &str); 5] = [
    (MarkKind::Header, "marks-header"),
    (MarkKind::Footer, "marks-footer"),
    (MarkKind::Watermark, "marks-watermark"),
    (MarkKind::Bates, "marks-bates"),
    (MarkKind::Background, "marks-background"),
];

/// The standard fonts a mark can be in, and their names' ids.
const FONTS: [(&str, &str); 6] = [
    ("Helvetica", "marks-font-helvetica"),
    ("Helvetica-Bold", "marks-font-helvetica-bold"),
    ("Times-Roman", "marks-font-times"),
    ("Times-Bold", "marks-font-times-bold"),
    ("Courier", "marks-font-courier"),
    ("Courier-Bold", "marks-font-courier-bold"),
];

/// How a watermark is turned.
const TURNS: [(StampPlace, &str); 3] = [
    (StampPlace::Middle, "marks-turn-level"),
    (StampPlace::Across, "marks-turn-across"),
    (StampPlace::Upright, "marks-turn-upright"),
];

/// Background colours: pale, for text to stay easy to read over them.
const PAPERS: [[f32; 3]; 5] = [
    [1.0, 0.98, 0.9],
    [1.0, 0.96, 0.75],
    [0.88, 0.94, 1.0],
    [0.9, 0.97, 0.88],
    [0.93, 0.93, 0.93],
];

/// The preview's room, in points of screen.
const PREVIEW_W: f32 = 220.0;
const PREVIEW_H: f32 = 300.0;

/// A popup the dialog has open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Popup {
    Fonts,
    Templates,
}

/// The dialog's choices.
pub(super) struct Marking {
    kind: MarkKind,
    place: StampPlace,
    ink: usize,
    /// A colour of a mark being changed that is none of [`INKS`], kept
    /// until another is picked.
    own_ink: Option<[f32; 3]>,
    font: usize,
    under: bool,
    words: Entity<TextInput>,
    size: Entity<TextInput>,
    opacity: Entity<TextInput>,
    pages: Entity<TextInput>,
    prefix: Entity<TextInput>,
    start: Entity<TextInput>,
    /// A background's colour, from [`PAPERS`], and picture.
    paper: Option<usize>,
    picture: Option<PathBuf>,
    /// The picture stretched over the whole page.
    stretch: bool,
    /// The background's picture shown rather than its colours.
    picture_tab: bool,
    /// The marks already on the pages.
    existing: Vec<PageMark>,
    /// The mark being changed, by id: put on anew in its place.
    editing: Option<String>,
    /// A background's picture already in the file, kept when it is
    /// changed and no new picture is chosen.
    kept_picture: Option<Ref>,
    /// The person's own templates.
    templates: Vec<Template>,
    popup: Option<Popup>,
    /// The page as it will look, and its page.
    preview: Option<(usize, Arc<gpui::RenderImage>, f32)>,
    /// Bumped with each change, so an older preview is dropped.
    asked: u64,
    _inputs: Vec<Subscription>,
}

/// The dialog's settings kept to use again, named by their words.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Template {
    name: String,
    kind: MarkKind,
    place: StampPlace,
    ink: usize,
    under: bool,
    words: String,
    size: String,
    prefix: String,
    start: String,
    paper: Option<usize>,
    stretch: bool,
    font: usize,
    opacity: String,
}

impl MarkKind {
    fn name(self) -> &'static str {
        match self {
            MarkKind::Header => "header",
            MarkKind::Footer => "footer",
            MarkKind::Watermark => "watermark",
            MarkKind::Bates => "bates",
            MarkKind::Background => "background",
        }
    }

    fn from_name(name: &str) -> Option<MarkKind> {
        KINDS.into_iter().map(|(k, _)| k).find(|k| k.name() == name)
    }

    /// The words of the kind's chip in the list of marks.
    fn short(self) -> &'static str {
        match self {
            MarkKind::Header => "marks-header",
            MarkKind::Footer => "marks-footer",
            MarkKind::Watermark => "marks-watermark",
            MarkKind::Bates => "marks-bates-short",
            MarkKind::Background => "marks-background",
        }
    }
}

/// The kind a mark on the pages is.
fn kind_of(mark: &PageMark) -> MarkKind {
    match &mark.what {
        MarkWhat::Stamp(stamp) if stamp.bates.is_some() => MarkKind::Bates,
        MarkWhat::Stamp(stamp)
            if matches!(
                stamp.place,
                StampPlace::Across | StampPlace::Middle | StampPlace::Upright
            ) =>
        {
            MarkKind::Watermark
        }
        MarkWhat::Stamp(stamp) if is_top(stamp.place) => MarkKind::Header,
        MarkWhat::Stamp(_) => MarkKind::Footer,
        MarkWhat::Background(..) => MarkKind::Background,
    }
}

fn is_top(place: StampPlace) -> bool {
    matches!(
        place,
        StampPlace::TopLeft | StampPlace::TopCentre | StampPlace::TopRight
    )
}

/// Left, middle or right (0, 1, 2) of a place along an edge.
fn column(place: StampPlace) -> usize {
    match place {
        StampPlace::TopLeft | StampPlace::BottomLeft => 0,
        StampPlace::TopRight | StampPlace::BottomRight => 2,
        _ => 1,
    }
}

/// The place along the top or the foot, at `column`.
fn edge_place(top: bool, column: usize) -> StampPlace {
    match (top, column) {
        (true, 0) => StampPlace::TopLeft,
        (true, 2) => StampPlace::TopRight,
        (true, _) => StampPlace::TopCentre,
        (false, 0) => StampPlace::BottomLeft,
        (false, 2) => StampPlace::BottomRight,
        (false, _) => StampPlace::BottomCentre,
    }
}

/// What the dialog would put on, and on which pages.
enum Spec {
    Stamp(Stamp),
    Background(Background, Option<PathBuf>, Option<Ref>),
}

/// Where the templates are kept: `key=value` lines, a blank line between
/// templates.
fn templates_file() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("marks.txt"))
}

fn load_templates() -> Vec<Template> {
    let Some(text) = templates_file().and_then(|f| std::fs::read_to_string(f).ok()) else {
        return Vec::new();
    };
    text.split("\n\n").filter_map(parse_template).collect()
}

fn parse_template(block: &str) -> Option<Template> {
    let mut t = Template {
        name: String::new(),
        kind: MarkKind::Header,
        place: StampPlace::BottomCentre,
        ink: 0,
        under: false,
        words: String::new(),
        size: "10".into(),
        prefix: String::new(),
        start: "1".into(),
        paper: None,
        stretch: false,
        font: 0,
        opacity: "100".into(),
    };
    let mut any = false;
    for line in block.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        any = true;
        let value = value.replace("\\n", "\n");
        match key {
            "name" => t.name = value,
            "kind" => t.kind = MarkKind::from_name(&value)?,
            "place" => t.place = StampPlace::from_name(&value)?,
            "ink" => t.ink = value.parse().ok().filter(|i| *i < INKS.len())?,
            "under" => t.under = value == "yes",
            "words" => t.words = value,
            "size" => t.size = value,
            "prefix" => t.prefix = value,
            "start" => t.start = value,
            "paper" => t.paper = value.parse().ok().filter(|i| *i < PAPERS.len()),
            "stretch" => t.stretch = value == "yes",
            "font" => t.font = value.parse().ok().filter(|i| *i < FONTS.len())?,
            "opacity" => t.opacity = value,
            _ => {}
        }
    }
    // Footers were headers placed at the foot before they had a name.
    if t.kind == MarkKind::Header && !is_top(t.place) {
        t.kind = MarkKind::Footer;
    }
    (any && !t.name.is_empty()).then_some(t)
}

fn save_templates(templates: &[Template]) {
    let Some(file) = templates_file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let yes = |b: bool| if b { "yes" } else { "no" };
    let blocks: Vec<String> = templates
        .iter()
        .map(|t| {
            let mut lines = vec![
                format!("name={}", t.name.replace('\n', "\\n")),
                format!("kind={}", t.kind.name()),
                format!("place={}", t.place.name()),
                format!("ink={}", t.ink),
                format!("under={}", yes(t.under)),
                format!("words={}", t.words.replace('\n', "\\n")),
                format!("size={}", t.size),
                format!("prefix={}", t.prefix),
                format!("start={}", t.start),
                format!("stretch={}", yes(t.stretch)),
                format!("font={}", t.font),
                format!("opacity={}", t.opacity),
            ];
            if let Some(paper) = t.paper {
                lines.push(format!("paper={paper}"));
            }
            lines.join("\n")
        })
        .collect();
    let mut text = blocks.join("\n\n");
    text.push('\n');
    let _ = std::fs::write(file, text);
}

/// The colour in `set` nearest `colour`.
fn nearest(colour: [f32; 3], set: &[[f32; 3]]) -> usize {
    let distance = |c: &[f32; 3]| {
        (0..3)
            .map(|i| (c[i] - colour[i]) * (c[i] - colour[i]))
            .sum::<f32>()
    };
    (0..set.len())
        .min_by(|a, b| distance(&set[*a]).total_cmp(&distance(&set[*b])))
        .unwrap_or_default()
}

/// What a mark on the pages is called in the card's list.
fn mark_label(mark: &PageMark, count: usize) -> String {
    let what = match &mark.what {
        MarkWhat::Stamp(stamp) => {
            let text = stamp.text_for(mark.pages.first().copied().unwrap_or(0), count, 0);
            text.lines().next().unwrap_or_default().to_owned()
        }
        MarkWhat::Background(..) => t("marks-background", &[]),
    };
    let pages = if mark.pages.len() >= count {
        t("marks-all-pages", &[])
    } else {
        ranges::format(&mark.pages)
    };
    t("marks-on", &[("what", &what), ("pages", &pages)])
}

/// What the words stand for in the page marks' messages.
const WORDS: &[(&str, &str)] = &[("page", "{page}"), ("pages", "{pages}")];
/// How faint a watermark is drawn at first, in percent.
const WATERMARK_OPACITY: &str = "30";
/// How far from the page's edges, in points.
const MARGIN: f32 = 28.0;

impl Viewer {
    /// Opens the page marks dialog, or closes it.
    pub(super) fn toggle_marking(&mut self, cx: &mut Context<Self>) {
        if self.marking.take().is_some() {
            cx.notify();
            return;
        }
        self.pick_adding(None, cx);
        self.properties = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
        self.printing = None;
        self.exporting = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let field = |placeholder: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_accent(accent);
                input
            })
        };
        let words = field(t("marks-text", &[]), cx);
        let size = field(t("marks-size", &[]), cx);
        let opacity = field("100".into(), cx);
        let pages = field(t("marks-pages-all", &[]), cx);
        let prefix = field(t("marks-bates-prefix", &[]), cx);
        let start = field(t("marks-bates-start", &[]), cx);
        let inputs = [&words, &size, &opacity, &pages, &prefix, &start]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| match event {
                    InputEvent::Submit => this.put_marks(cx),
                    InputEvent::Changed => this.preview_marks(cx),
                    InputEvent::Cancel => {}
                })
            })
            .collect();
        self.marking = Some(Marking {
            kind: MarkKind::Footer,
            place: StampPlace::BottomCentre,
            ink: 0,
            own_ink: None,
            font: 0,
            under: false,
            words,
            size,
            opacity,
            pages,
            prefix,
            start,
            paper: Some(0),
            picture: None,
            stretch: false,
            picture_tab: false,
            existing: Vec::new(),
            editing: None,
            kept_picture: None,
            templates: load_templates(),
            popup: None,
            preview: None,
            asked: 0,
            _inputs: inputs,
        });
        self.set_mark_kind(MarkKind::Footer, cx);
        self.read_marks(cx);
    }

    /// Reads the marks already on the pages, for the dialog's list.
    fn read_marks(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref())?;
                    Ok::<_, String>(doc.page_marks())
                })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(marks) = read {
                    this.marks_changed(marks, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The file shown changed: the dialog lists its marks again, and
    /// draws its preview from the file as it is now.
    pub(super) fn marks_changed(&mut self, marks: Vec<PageMark>, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        marking.existing = marks;
        self.preview_marks(cx);
    }

    /// Something in the dialog changed: it shows again, and so does
    /// the preview.
    fn marks_touched(&mut self, cx: &mut Context<Self>) {
        self.preview_marks(cx);
        cx.notify();
    }

    /// Loads the mark `id` into the dialog, to be changed.
    fn edit_mark(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_ref() else {
            return;
        };
        let Some(mark) = marking.existing.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let count = self.page_count();
        let pages = if mark.pages.len() >= count {
            String::new()
        } else {
            ranges::format(&mark.pages)
        };
        self.set_mark_kind(kind_of(&mark), cx);
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        match &mark.what {
            MarkWhat::Stamp(stamp) => {
                marking.place = stamp.place;
                marking.ink = nearest(stamp.colour, &INKS);
                marking.own_ink = (INKS[marking.ink] != stamp.colour).then_some(stamp.colour);
                marking.font = FONTS
                    .iter()
                    .position(|(name, _)| *name == stamp.font)
                    .unwrap_or_default();
                marking.under = stamp.under;
                marking.kept_picture = None;
                let words = stamp.text.clone();
                marking.words.update(cx, |f, cx| f.set_text(words, cx));
                let size = stamp.size.to_string();
                marking.size.update(cx, |f, cx| f.set_text(size, cx));
                let opacity = ((stamp.opacity * 100.0).round() as u32).to_string();
                marking.opacity.update(cx, |f, cx| f.set_text(opacity, cx));
                if let Some(b) = &stamp.bates {
                    let prefix = b.prefix.clone();
                    marking.prefix.update(cx, |f, cx| f.set_text(prefix, cx));
                    let start = b.start.to_string();
                    marking.start.update(cx, |f, cx| f.set_text(start, cx));
                }
            }
            MarkWhat::Background(background, picture) => {
                marking.paper = background.colour.map(|c| nearest(c, &PAPERS));
                marking.picture = None;
                marking.kept_picture = *picture;
                marking.picture_tab = picture.is_some() && background.colour.is_none();
                marking.stretch = background.stretch;
            }
        }
        marking.pages.update(cx, |f, cx| f.set_text(pages, cx));
        marking.editing = Some(mark.id);
        self.marks_touched(cx);
    }

    /// Takes the mark `id` off the pages; the dialog stays open.
    fn remove_mark(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        let Some(mark) = marking.existing.iter().find(|m| m.id == id) else {
            return;
        };
        let shown = mark.pages.clone();
        if marking.editing.as_deref() == Some(id.as_str()) {
            marking.editing = None;
            marking.kept_picture = None;
        }
        self.change_document(
            shown,
            "marks-failed",
            move |doc| doc.remove_marks(&[id]).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Keeps the dialog's settings as a template, named by its words.
    fn save_template(&mut self, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        let read = |input: &Entity<TextInput>| input.read(cx).text().trim().to_owned();
        let words = read(&marking.words);
        let prefix = read(&marking.prefix);
        let name = match marking.kind {
            MarkKind::Bates => t("marks-template-bates", &[("prefix", &prefix)]),
            MarkKind::Background => t("marks-background", &[]),
            _ => words.lines().next().unwrap_or_default().to_owned(),
        };
        marking.popup = None;
        if name.is_empty() {
            cx.notify();
            return;
        }
        let template = Template {
            name,
            kind: marking.kind,
            place: marking.place,
            ink: marking.ink,
            under: marking.under,
            words,
            size: read(&marking.size),
            prefix,
            start: read(&marking.start),
            paper: marking.paper,
            stretch: marking.stretch,
            font: marking.font,
            opacity: read(&marking.opacity),
        };
        marking.templates.retain(|t| t.name != template.name);
        marking.templates.push(template);
        save_templates(&marking.templates);
        cx.notify();
    }

    /// Fills the dialog from template `i`.
    fn use_template(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(template) = self
            .marking
            .as_ref()
            .and_then(|m| m.templates.get(i))
            .cloned()
        else {
            return;
        };
        self.set_mark_kind(template.kind, cx);
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        marking.popup = None;
        marking.place = template.place;
        marking.ink = template.ink;
        marking.under = template.under;
        marking.paper = template.paper;
        marking.stretch = template.stretch;
        marking.font = template.font;
        let fields = [
            (marking.words.clone(), template.words),
            (marking.size.clone(), template.size),
            (marking.prefix.clone(), template.prefix),
            (marking.start.clone(), template.start),
            (marking.opacity.clone(), template.opacity),
        ];
        for (input, value) in fields {
            input.update(cx, |f, cx| f.set_text(value, cx));
        }
        self.marks_touched(cx);
    }

    /// Switches the kind of mark, with that kind's usual words and size.
    fn set_mark_kind(&mut self, kind: MarkKind, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        marking.kind = kind;
        let (words, size, place, ink, opacity) = match kind {
            MarkKind::Header => (
                t("marks-header-default", WORDS),
                "10",
                StampPlace::TopCentre,
                0,
                "100",
            ),
            MarkKind::Footer => (
                t("marks-header-default", WORDS),
                "10",
                StampPlace::BottomCentre,
                0,
                "100",
            ),
            MarkKind::Watermark => (
                t("marks-watermark-default", &[]),
                "60",
                StampPlace::Across,
                1,
                WATERMARK_OPACITY,
            ),
            MarkKind::Bates | MarkKind::Background => {
                (String::new(), "10", StampPlace::BottomRight, 0, "100")
            }
        };
        marking.place = place;
        marking.ink = ink;
        marking.own_ink = None;
        marking.font = if kind == MarkKind::Watermark { 1 } else { 0 };
        marking.under = false;
        marking.popup = None;
        marking.words.update(cx, |f, cx| f.set_text(words, cx));
        marking
            .size
            .update(cx, |f, cx| f.set_text(size.to_string(), cx));
        marking
            .opacity
            .update(cx, |f, cx| f.set_text(opacity.to_string(), cx));
        marking
            .start
            .update(cx, |f, cx| f.set_text("1".to_string(), cx));
        self.marks_touched(cx);
    }

    /// The pages the dialog names, from 0.
    fn mark_pages(&self, marking: &Marking, cx: &Context<Self>) -> Result<Vec<usize>, String> {
        let count = self.page_count();
        let spec = marking.pages.read(cx).text().trim().to_owned();
        if spec.is_empty() {
            Ok((0..count).collect())
        } else {
            katna_pdf_cos::ranges::parse(&spec, count)
        }
    }

    /// What the dialog would put on, or `None` when there is nothing
    /// to put on yet.
    fn mark_spec(&self, marking: &Marking, cx: &Context<Self>) -> Option<Spec> {
        let read = |input: &Entity<TextInput>| input.read(cx).text().trim().to_owned();
        if marking.kind == MarkKind::Background {
            let background = Background {
                colour: marking.paper.map(|i| PAPERS[i]),
                image: None,
                stretch: marking.stretch,
                opacity: 1.0,
            };
            let picture = marking.picture.clone();
            let kept = marking.kept_picture.filter(|_| picture.is_none());
            if background.colour.is_none() && picture.is_none() && kept.is_none() {
                return None;
            }
            return Some(Spec::Background(background, picture, kept));
        }
        let number = |input: &Entity<TextInput>| {
            read(input)
                .trim_end_matches('%')
                .trim()
                .replace(',', ".")
                .parse::<f32>()
                .ok()
                .filter(|s| s.is_finite())
        };
        let size = number(&marking.size)
            .filter(|s| *s > 0.0 && *s <= 500.0)
            .unwrap_or(10.0);
        let opacity = number(&marking.opacity)
            .map(|o| (o / 100.0).clamp(0.05, 1.0))
            .unwrap_or(1.0);
        let (words, bates) = match marking.kind {
            MarkKind::Bates => {
                let start = read(&marking.start).parse::<u64>().unwrap_or(1);
                let bates = Bates {
                    prefix: read(&marking.prefix),
                    suffix: String::new(),
                    start,
                    digits: 6,
                };
                ("{bates}".to_owned(), Some(bates))
            }
            _ => (read(&marking.words), None),
        };
        if words.is_empty() {
            return None;
        }
        Some(Spec::Stamp(Stamp {
            text: words,
            place: marking.place,
            font: FONTS[marking.font].0.into(),
            size,
            colour: marking.own_ink.unwrap_or(INKS[marking.ink]),
            opacity,
            margin: MARGIN,
            under: marking.under,
            bates,
        }))
    }

    /// Puts the marks on the pages, as the dialog says.
    fn put_marks(&mut self, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_ref() else {
            return;
        };
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let pages = match self.mark_pages(marking, cx) {
            Ok(pages) => pages,
            Err(error) => {
                self.notice = Some(super::said("marks-failed", &[("error", &error)]));
                cx.notify();
                return;
            }
        };
        let Some(spec) = self.mark_spec(marking, cx) else {
            return;
        };
        let editing = marking.editing.clone();
        self.marking = None;
        let shown = pages.clone();
        self.change_document(
            shown,
            "marks-failed",
            move |doc| marked(doc, editing, &pages, spec),
            cx,
        );
    }

    /// Draws the page shown (or the first page marked, when it is not
    /// one of them) as it will look, a moment after the last change.
    fn preview_marks(&mut self, cx: &mut Context<Self>) {
        let Some(marking) = self.marking.as_ref() else {
            return;
        };
        let Some(path) = self.path.clone() else {
            return;
        };
        let pages = self.mark_pages(marking, cx).unwrap_or_default();
        let current = self.current_page();
        let page = if pages.contains(&current) || pages.is_empty() {
            current
        } else {
            pages[0]
        };
        let mut spec = self.mark_spec(marking, cx);
        // Bates numbers count the pages marked, not the pages.
        if let Some(Spec::Stamp(Stamp {
            bates: Some(bates), ..
        })) = spec.as_mut()
        {
            let nth = pages.iter().position(|p| *p == page).unwrap_or(0);
            bates.start = bates.start.saturating_add(nth as u64);
        }
        if !pages.contains(&page) {
            spec = None;
        }
        let editing = marking.editing.clone();
        let current = self.unsaved();
        let key = self.key.clone();
        let Some(marking) = self.marking.as_mut() else {
            return;
        };
        marking.asked += 1;
        let asked = marking.asked;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let still = this
                .update(cx, |this, _| {
                    this.marking.as_ref().is_some_and(|m| m.asked == asked)
                })
                .unwrap_or(false);
            if !still {
                return;
            }
            let drawn = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let bytes = match spec {
                        Some(spec) => {
                            let doc = super::cos(bytes, key.as_deref()).ok()?;
                            marked(&doc, editing, &[page], spec).ok()?
                        }
                        None => bytes,
                    };
                    let doc = crate::engine::engine()
                        .open(Arc::new(bytes), key.as_deref().unwrap_or(""))
                        .ok()?;
                    let (w, h) = doc.page_size(page)?;
                    let scale = (PREVIEW_W / w).min(PREVIEW_H / h) * 2.0;
                    let image = super::draw(
                        doc.as_ref(),
                        page,
                        scale,
                        katna_pdf_render::recolor::PageColors::Normal,
                    )?;
                    Some((page, image, w / h))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(m) = this.marking.as_mut()
                    && m.asked == asked
                    && drawn.is_some()
                {
                    m.preview = drawn;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks for a picture to put under the pages.
    fn choose_background(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("marks-background-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(m) = this.marking.as_mut() {
                    m.picture = paths.into_iter().next();
                }
                this.marks_touched(cx);
            })
            .ok();
        })
        .detach();
    }
}

/// `doc` with the marks `spec` says on `pages`, in place of the mark
/// `editing` names when one is being changed.
fn marked(
    doc: &katna_pdf_cos::Document,
    editing: Option<String>,
    pages: &[usize],
    spec: Spec,
) -> Result<Vec<u8>, String> {
    let base = without(doc, editing)?;
    let doc = base.as_ref().unwrap_or(doc);
    match spec {
        Spec::Stamp(stamp) => doc.stamp(pages, &stamp).map_err(|e| e.to_string()),
        Spec::Background(mut background, picture, kept) => {
            if let Some(file) = picture {
                let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                background.image = Some(katna_pdf_sign::page_image(&bytes)?);
            }
            doc.background_with(pages, &background, kept)
                .map_err(|e| e.to_string())
        }
    }
}

impl Viewer {
    /// The page marks dialog (E12), while it is open: what to put on
    /// and where on the left, under the marks already on; the page as
    /// it will look on the right.
    pub(super) fn marks_card(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let marking = self.marking.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        let hint = |words: String| {
            div()
                .text_size(px(text::CAPTION))
                .text_color(th.dim)
                .child(words)
        };
        let column_of = |id: &str, el: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(label(id))
                .child(el)
        };
        let kinds = div().flex().flex_wrap().gap(px(space::S2)).children(
            KINDS.into_iter().enumerate().map(|(i, (kind, id))| {
                float::chip(("marks-kind", i), t(id, &[]), marking.kind == kind, &th).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.editing = None;
                            m.kept_picture = None;
                        }
                        this.set_mark_kind(kind, cx);
                    }),
                )
            }),
        );
        let text_mark = marking.kind != MarkKind::Background;
        let mut body = div().flex().flex_col().gap(px(space::S3)).child(kinds);
        if text_mark {
            body = body.child(self.mark_words(marking, window, cx));
            body = body.child(self.mark_looks(marking, window, cx));
            body = body.child(self.mark_place(marking, cx));
        } else {
            body = body.child(self.background_choices(marking, cx));
        }
        body = body.child(column_of(
            "marks-pages-label",
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(float::input_box(&marking.pages, window, cx, &th))
                .child(hint(t("marks-pages-hint", &[])))
                .into_any_element(),
        ));
        if let Some(list) = self.marks_list(marking, cx) {
            body = body.child(list);
        }
        let count = self.mark_pages(marking, cx).map(|p| p.len()).ok();
        let apply = match (count, marking.editing.is_some()) {
            (None | Some(0), _) => t("marks-apply", &[]),
            (Some(n), false) => t("marks-apply-n", &[("count", &n.to_string())]),
            (Some(n), true) => t("marks-change-n", &[("count", &n.to_string())]),
        };
        let ready = count.is_some_and(|n| n > 0);
        let templates = div()
            .relative()
            .child(
                float::button("marks-templates", t("marks-templates", &[]), false, &th)
                    .child(float::icon("chev", 12.0, th.dim))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.popup = match m.popup {
                                Some(Popup::Templates) => None,
                                _ => Some(Popup::Templates),
                            };
                        }
                        cx.notify();
                    })),
            )
            .when(marking.popup == Some(Popup::Templates), |d| {
                d.child(float::popover(
                    self.templates_menu(marking, cx),
                    true,
                    false,
                ))
            });
        let settings = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::S5))
                    .pt(px(space::S4))
                    .pb(px(space::S2))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("marks-title", &[])),
                    ),
            )
            .child(
                div()
                    .id("marks-settings")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(space::S5))
                    .py(px(space::S2))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S5))
                    .py(px(space::S3))
                    .border_t_1()
                    .border_color(th.line)
                    .child(templates)
                    .child(div().flex_1())
                    .child(
                        float::button("marks-cancel", t("marks-cancel", &[]), false, &th).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.marking = None;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        float::button("marks-apply", apply, true, &th)
                            .when(!ready, |d| d.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.put_marks(cx))),
                    ),
            );
        let height = (f32::from(window.viewport_size().height) - 80.0).clamp(360.0, 600.0);
        Some(
            div()
                .id("marks-card")
                .w(px(800.0))
                .h(px(height))
                .flex()
                .overflow_hidden()
                .rounded(px(12.0))
                .bg(th.card)
                .shadow(th.lifted())
                .when(th.dark, |d| d.border_1().border_color(th.line))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(settings)
                .child(div().w(px(1.0)).h_full().bg(th.line))
                .child(self.marks_preview(marking))
                .into_any_element(),
        )
    }

    /// The words of a header, footer or watermark, or a Bates number's
    /// start.
    fn mark_words(
        &self,
        marking: &Marking,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        if marking.kind == MarkKind::Bates {
            return div()
                .flex()
                .gap(px(space::S3))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("marks-bates-prefix-label"))
                        .child(float::input_box(&marking.prefix, window, cx, &th)),
                )
                .child(
                    div()
                        .w(px(120.0))
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("marks-bates-start-label"))
                        .child(float::input_box(&marking.start, window, cx, &th)),
                )
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(label("marks-text-label"))
            .child(float::input_box(&marking.words, window, cx, &th))
            .when(
                matches!(marking.kind, MarkKind::Header | MarkKind::Footer),
                |d| {
                    d.child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(t("marks-text-hint", WORDS)),
                    )
                },
            )
            .into_any_element()
    }

    /// Font, size, colour and how solid.
    fn mark_looks(
        &self,
        marking: &Marking,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        let fonts = div()
            .relative()
            .child(
                div()
                    .id("marks-font")
                    .h(px(36.0))
                    .w(px(144.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(th.line)
                    .bg(if th.dark { th.field } else { th.card })
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(t(FONTS[marking.font].1, &[])),
                    )
                    .child(float::icon("chev", 12.0, th.dim))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.popup = match m.popup {
                                Some(Popup::Fonts) => None,
                                _ => Some(Popup::Fonts),
                            };
                        }
                        cx.notify();
                    })),
            )
            .when(marking.popup == Some(Popup::Fonts), |d| {
                let mut menu = float::menu("marks-fonts", 180.0, &th).on_mouse_down_out(
                    cx.listener(|this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.popup = None;
                        }
                        cx.notify();
                    }),
                );
                for (i, (_, id)) in FONTS.into_iter().enumerate() {
                    let on = marking.font == i;
                    menu = menu.child(
                        float::menu_item(
                            ("marks-font-item", i),
                            on.then_some("check"),
                            t(id, &[]),
                            None,
                            &th,
                        )
                        .when(!on, |d| d.pl(px(36.0)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(m) = this.marking.as_mut() {
                                m.font = i;
                                m.popup = None;
                            }
                            this.marks_touched(cx);
                        })),
                    );
                }
                d.child(float::popover(menu, false, false))
            });
        let mut inks = div().h(px(36.0)).flex().items_center();
        for (i, ink) in INKS.into_iter().enumerate() {
            let on = marking.ink == i && marking.own_ink.is_none();
            inks = inks.child(
                float::swatch(("marks-ink", i), super::insert::colour(ink).into(), on, &th)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.ink = i;
                            m.own_ink = None;
                        }
                        this.marks_touched(cx);
                    })),
            );
        }
        let small = |id: &str, input: &Entity<TextInput>, unit: Option<&str>| {
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(label(id))
                .child(
                    float::input_box(input, window, cx, &th)
                        .w(px(if unit.is_some() { 76.0 } else { 64.0 }))
                        .children(unit.map(|u| {
                            div()
                                .pl(px(space::S1))
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child(u.to_owned())
                        })),
                )
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(space::S3))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("marks-font-label"))
                    .child(fonts),
            )
            .child(small("marks-size-label", &marking.size, None))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("marks-colour-label"))
                    .child(inks),
            )
            .child(small("marks-opacity-label", &marking.opacity, Some("%")))
            .into_any_element()
    }

    /// Where on the page: along an edge, or turned for a watermark;
    /// over the page's own drawing or under it.
    fn mark_place(&self, marking: &Marking, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        let place = marking.place;
        let pick = move |p: StampPlace| {
            move |this: &mut Viewer,
                  _: &gpui::ClickEvent,
                  _: &mut gpui::Window,
                  cx: &mut Context<Viewer>| {
                if let Some(m) = this.marking.as_mut() {
                    m.place = p;
                }
                this.marks_touched(cx);
            }
        };
        let item = |id: (&'static str, usize), words: String, on: bool| {
            float::seg_item(id, None, Some(words.into()), on, &th).flex_none()
        };
        let mut groups = div().flex().flex_wrap().gap(px(space::S4));
        if marking.kind == MarkKind::Watermark {
            let mut seg = float::seg(&th);
            for (i, (p, id)) in TURNS.into_iter().enumerate() {
                seg = seg.child(
                    item(("marks-turn", i), t(id, &[]), place == p).on_click(cx.listener(pick(p))),
                );
            }
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("marks-turned"))
                    .child(seg),
            );
        } else {
            let top = is_top(place);
            if marking.kind == MarkKind::Bates {
                let mut edge = float::seg(&th);
                for (i, (at_top, id)) in [(true, "marks-edge-top"), (false, "marks-edge-foot")]
                    .into_iter()
                    .enumerate()
                {
                    let to = edge_place(at_top, column(place));
                    edge = edge.child(
                        item(("marks-edge", i), t(id, &[]), top == at_top)
                            .on_click(cx.listener(pick(to))),
                    );
                }
                groups = groups.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("marks-edge"))
                        .child(edge),
                );
            }
            let mut along = float::seg(&th);
            for (i, id) in ["marks-left", "marks-centre", "marks-right"]
                .into_iter()
                .enumerate()
            {
                let to = edge_place(top, i);
                along = along.child(
                    item(("marks-along", i), t(id, &[]), column(place) == i)
                        .on_click(cx.listener(pick(to))),
                );
            }
            groups = groups.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("marks-along"))
                    .child(along),
            );
        }
        let under = marking.under;
        let mut layer = float::seg(&th);
        for (i, (below, id)) in [(false, "marks-over"), (true, "marks-under")]
            .into_iter()
            .enumerate()
        {
            layer = layer.child(
                item(("marks-layer", i), t(id, &[]), under == below).on_click(cx.listener(
                    move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.under = below;
                        }
                        this.marks_touched(cx);
                    },
                )),
            );
        }
        groups
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("marks-layer"))
                    .child(layer),
            )
            .into_any_element()
    }

    /// The marks already on the pages, each to change or take off.
    fn marks_list(&self, marking: &Marking, cx: &mut Context<Self>) -> Option<AnyElement> {
        if marking.existing.is_empty() {
            return None;
        }
        let th = self.theme;
        let count = self.page_count();
        let mut list = div()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .border_1()
            .border_color(th.line)
            .overflow_hidden();
        for (i, mark) in marking.existing.iter().enumerate() {
            let on = marking.editing.as_deref() == Some(mark.id.as_str());
            let edit_id = mark.id.clone();
            let remove_id = mark.id.clone();
            let ghost = |id: (&'static str, usize), words: String| {
                div()
                    .id(id)
                    .h(px(28.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .text_size(px(text::SMALL))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .child(words)
            };
            list =
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .min_h(px(48.0))
                        .px(px(space::S3))
                        .when(i > 0, |d| d.border_t_1().border_color(th.line))
                        .when(on, |d| d.bg(th.accent_soft.opacity(0.5)))
                        .child(
                            div()
                                .flex_none()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(if th.dark { th.line } else { th.sunk })
                                .text_size(px(text::MICRO))
                                .text_color(th.text2)
                                .child(t(kind_of(mark).short(), &[])),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(text::SMALL))
                                .text_color(th.text)
                                .child(mark_label(mark, count)),
                        )
                        .child(ghost(("marks-edit", i), t("marks-change", &[])).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.edit_mark(&edit_id, cx);
                            }),
                        ))
                        .child(
                            ghost(("marks-remove", i), t("marks-take-off", &[])).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.remove_mark(remove_id.clone(), cx);
                                }),
                            ),
                        ),
                );
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .pt(px(space::S2))
                        .text_size(px(text::MICRO))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.dim)
                        .child(t("marks-existing", &[]).to_uppercase()),
                )
                .child(list)
                .into_any_element(),
        )
    }

    /// The templates kept, to use or forget, and a way to keep these
    /// settings as another.
    fn templates_menu(&self, marking: &Marking, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let mut menu = float::menu("marks-templates-menu", 320.0, &th).on_mouse_down_out(
            cx.listener(|this, _, _, cx| {
                if let Some(m) = this.marking.as_mut() {
                    m.popup = None;
                }
                cx.notify();
            }),
        );
        for (i, template) in marking.templates.iter().enumerate() {
            menu = menu.child(
                float::menu_item(
                    ("marks-template", i),
                    Some("drop"),
                    template.name.clone(),
                    None,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.use_template(i, cx)))
                .child(
                    float::sized_ib(
                        ("marks-template-off", i),
                        "x",
                        22.0,
                        12.0,
                        t("marks-template-remove", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if let Some(m) = this.marking.as_mut().filter(|m| i < m.templates.len()) {
                            m.templates.remove(i);
                            save_templates(&m.templates);
                        }
                        cx.notify();
                    })),
                ),
            );
        }
        if marking.templates.is_empty() {
            menu = menu.child(
                div()
                    .px(px(10.0))
                    .py(px(space::S2))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("marks-templates-none", &[])),
            );
        }
        menu.child(float::menu_sep(&th))
            .child(
                float::menu_item(
                    "marks-template-save",
                    Some("plus"),
                    t("marks-template-save", &[]),
                    None,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.save_template(cx))),
            )
            .into_any_element()
    }

    /// The right side: the page as it will look.
    fn marks_preview(&self, marking: &Marking) -> AnyElement {
        let th = self.theme;
        let (page, image, aspect) = match &marking.preview {
            Some((page, image, aspect)) => (Some(*page), Some(image.clone()), *aspect),
            None => (None, None, 595.0 / 842.0),
        };
        let (w, h) = if aspect > PREVIEW_W / PREVIEW_H {
            (PREVIEW_W, PREVIEW_W / aspect)
        } else {
            (PREVIEW_H * aspect, PREVIEW_H)
        };
        div()
            .w(px(260.0))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(space::S3))
            .bg(th.sunk)
            .child(
                div()
                    .w(px(w))
                    .h(px(h))
                    .bg(gpui::white())
                    .shadow(th.lifted())
                    .children(
                        image.map(|image| gpui::img(gpui::ImageSource::Render(image)).size_full()),
                    ),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(match page {
                        Some(page) => t(
                            "marks-preview-page",
                            &[
                                ("page", &(page + 1).to_string()),
                                ("count", &self.page_count().to_string()),
                            ],
                        ),
                        None => t("print-preview-working", &[]),
                    }),
            )
            .into_any_element()
    }

    /// A background's colours, or its picture.
    fn background_choices(&self, marking: &Marking, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let mut tabs = float::seg(&th);
        for (i, (picture, id)) in [
            (false, "marks-background-colour"),
            (true, "marks-background-picture"),
        ]
        .into_iter()
        .enumerate()
        {
            tabs = tabs.child(
                float::seg_item(
                    ("marks-background-tab", i),
                    None,
                    Some(t(id, &[]).into()),
                    marking.picture_tab == picture,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(m) = this.marking.as_mut() {
                        m.picture_tab = picture;
                    }
                    cx.notify();
                })),
            );
        }
        let body = if marking.picture_tab {
            let name = match (&marking.picture, marking.kept_picture) {
                (Some(p), _) => super::file_name(p),
                (None, Some(_)) => t("marks-background-kept", &[]),
                (None, None) => t("marks-background-no-picture", &[]),
            };
            let stretch = marking.stretch;
            div()
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(name),
                        )
                        .when(
                            marking.picture.is_some() || marking.kept_picture.is_some(),
                            |row| {
                                row.child(
                                    float::button(
                                        "marks-picture-off",
                                        t("marks-background-no-picture-tip", &[]),
                                        false,
                                        &th,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            if let Some(m) = this.marking.as_mut() {
                                                m.picture = None;
                                                m.kept_picture = None;
                                            }
                                            this.marks_touched(cx);
                                        },
                                    )),
                                )
                            },
                        )
                        .child(
                            float::button(
                                "marks-picture",
                                t("marks-background-choose", &[]),
                                false,
                                &th,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.choose_background(cx))),
                        ),
                )
                .child(
                    div()
                        .id("marks-stretch")
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(m) = this.marking.as_mut() {
                                m.stretch = !m.stretch;
                            }
                            this.marks_touched(cx);
                        }))
                        .child(check_box(stretch, th))
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .child(t("marks-background-stretch", &[])),
                        ),
                )
        } else {
            let mut papers = div().flex().items_center().gap(px(space::S1)).child(
                float::swatch(
                    ("marks-paper", PAPERS.len()),
                    th.card,
                    marking.paper.is_none(),
                    &th,
                )
                .tip(t("marks-background-no-colour", &[]), &th.katna)
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(m) = this.marking.as_mut() {
                        m.paper = None;
                    }
                    this.marks_touched(cx);
                })),
            );
            for (i, paper) in PAPERS.into_iter().enumerate() {
                papers = papers.child(
                    float::swatch(
                        ("marks-paper", i),
                        super::insert::colour(paper).into(),
                        marking.paper == Some(i),
                        &th,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(m) = this.marking.as_mut() {
                            m.paper = Some(i);
                        }
                        this.marks_touched(cx);
                    })),
                );
            }
            div()
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .child(papers)
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(t("marks-background-hint", &[])),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(tabs)
            .child(body)
            .into_any_element()
    }
}

/// `doc` with the mark `editing` names taken off, when one is being
/// changed, so the new one takes its place.
fn without(
    doc: &katna_pdf_cos::Document,
    editing: Option<String>,
) -> Result<Option<katna_pdf_cos::Document>, String> {
    let Some(id) = editing else {
        return Ok(None);
    };
    let bytes = doc.remove_marks(&[id]).map_err(|e| e.to_string())?;
    katna_pdf_cos::Document::load(bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}
