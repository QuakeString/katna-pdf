// SPDX-License-Identifier: GPL-3.0-or-later

//! A comment's look (colour, line width, opacity) kept as the default
//! for new comments made with its tool, as Acrobat's "Make current
//! properties default". The looks are kept in `comment-looks.txt` in
//! the state folder, a line each: the tool, red, green, blue, width and
//! opacity.

use std::collections::BTreeMap;
use std::path::PathBuf;

use gpui::Context;
use katna_pdf_cos::{AnnotKind, Annotation, MarkupStyle};

use super::Viewer;
use crate::annotate::{COLOURS, Tool};

/// How a comment looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Look {
    pub colour: [f32; 3],
    pub width: f32,
    pub opacity: f32,
}

impl Look {
    fn of(a: &Annotation) -> Self {
        Look {
            colour: a.colour,
            width: a.width,
            opacity: a.opacity,
        }
    }

    /// Whether `a` looks like this, to what the file keeps.
    fn same(&self, a: &Annotation) -> bool {
        let near = |x: f32, y: f32| (x - y).abs() < 0.005;
        self.colour.iter().zip(a.colour).all(|(x, y)| near(*x, y))
            && near(self.width, a.width)
            && near(self.opacity, a.opacity)
    }
}

/// The looks kept, by tool.
#[derive(Debug, Default)]
pub(super) struct Looks(BTreeMap<Tool, Look>);

impl Looks {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|text| Self::read(&text))
            .unwrap_or_default()
    }

    fn read(text: &str) -> Self {
        let mut looks = BTreeMap::new();
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(tool) = words
                .next()
                .and_then(|name| Tool::ALL.into_iter().find(|t| format!("{t:?}") == name))
            else {
                continue;
            };
            let numbers: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
            if let [r, g, b, width, opacity] = numbers[..] {
                looks.insert(
                    tool,
                    Look {
                        colour: [r, g, b].map(|c| c.clamp(0.0, 1.0)),
                        width: width.max(0.0),
                        opacity: opacity.clamp(0.0, 1.0),
                    },
                );
            }
        }
        Looks(looks)
    }

    fn write(&self) -> String {
        self.0
            .iter()
            .map(|(tool, l)| {
                let [r, g, b] = l.colour;
                format!(
                    "{tool:?} {r:.3} {g:.3} {b:.3} {:.2} {:.2}\n",
                    l.width, l.opacity
                )
            })
            .collect()
    }

    fn save(&self) {
        let Some(file) = file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, self.write());
    }

    /// The look kept for new comments made with `tool`.
    pub fn get(&self, tool: Tool) -> Option<&Look> {
        self.0.get(&key(tool))
    }
}

fn file() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("comment-looks.txt"))
}

/// The tool whose look `tool` shares: the typewriter makes the same
/// text comments as the text box.
fn key(tool: Tool) -> Tool {
    match tool {
        Tool::Typewriter => Tool::FreeText,
        t => t,
    }
}

/// The tool that makes comments like `a`; none for measurements, which
/// keep their own look, and for what no tool makes.
pub(super) fn tool_of(a: &Annotation) -> Option<Tool> {
    if a.measure.is_some() || a.reply_to.is_some() {
        return None;
    }
    Some(match &a.kind {
        AnnotKind::Markup(MarkupStyle::Highlight, _) => Tool::Highlight,
        AnnotKind::Markup(MarkupStyle::Underline, _) => Tool::Underline,
        AnnotKind::Markup(MarkupStyle::StrikeOut, _) => Tool::StrikeOut,
        AnnotKind::Markup(MarkupStyle::Squiggly, _) => Tool::Squiggly,
        AnnotKind::Note => Tool::Note,
        AnnotKind::FreeText { .. } if a.callout.is_some() => Tool::Callout,
        AnnotKind::FreeText { .. } => Tool::FreeText,
        AnnotKind::Ink(_) => Tool::Ink,
        AnnotKind::Square { .. } => Tool::Rectangle,
        AnnotKind::Circle { .. } => Tool::Ellipse,
        AnnotKind::Line { arrow: false, .. } => Tool::Line,
        AnnotKind::Line { arrow: true, .. } => Tool::Arrow,
        AnnotKind::Polygon(_) if a.cloudy => Tool::Cloud,
        AnnotKind::Polygon(_) => Tool::Polygon,
        AnnotKind::PolyLine(_) => Tool::Polyline,
        AnnotKind::Stamp(_) => Tool::Stamp,
        AnnotKind::FileAttachment { .. } => Tool::Attach,
        AnnotKind::Other(_) => return None,
    })
}

impl Viewer {
    /// The colour what is drawn next takes: the one picked, or else the
    /// look kept for the tool in hand.
    pub(super) fn drawing_colour(&self) -> [f32; 3] {
        match self.kept_look() {
            Some(look) => look.colour,
            None => COLOURS[self.colour],
        }
    }

    /// The look kept for the tool in hand, while no colour has been
    /// picked since the tool was.
    pub(super) fn kept_look(&self) -> Option<Look> {
        if self.colour_chosen {
            return None;
        }
        self.tool.and_then(|t| self.looks.get(t)).copied()
    }

    /// `a`, new, given the look kept for its tool: the colour unless one
    /// was picked since the tool was, its width and its opacity.
    pub(super) fn with_kept_look(&self, a: &mut Annotation) {
        let Some(look) = tool_of(a).and_then(|t| self.looks.get(t)) else {
            return;
        };
        if !self.colour_chosen {
            a.colour = look.colour;
        }
        a.width = look.width;
        a.opacity = look.opacity;
    }

    /// Whether the look kept for `a`'s tool is `a`'s own.
    pub(super) fn look_is_kept(&self, a: &Annotation) -> bool {
        tool_of(a)
            .and_then(|t| self.looks.get(t))
            .is_some_and(|l| l.same(a))
    }

    /// Keeps the chosen comment's look for new comments made with its
    /// tool, or, when it is already the one kept, lets it go.
    pub(super) fn toggle_kept_look(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_annotation() else {
            return;
        };
        let Some(tool) = tool_of(a) else {
            return;
        };
        let name = crate::text::t(tool.name(), &[]);
        if self.look_is_kept(a) {
            self.looks.0.remove(&key(tool));
            self.notice = Some(super::said("comment-look-dropped", &[("tool", &name)]));
        } else {
            self.looks.0.insert(key(tool), Look::of(a));
            self.notice = Some(super::said("comment-look-kept", &[("tool", &name)]));
        }
        self.looks.save();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_read_back_as_written() {
        let mut looks = Looks::default();
        looks.0.insert(
            Tool::Ink,
            Look {
                colour: [0.1, 0.2, 0.3],
                width: 4.0,
                opacity: 0.5,
            },
        );
        let again = Looks::read(&looks.write());
        assert_eq!(again.get(Tool::Ink), looks.get(Tool::Ink));
        assert!(Looks::read("Nonsense 1 2 3 4 5\nInk 1 2\n").0.is_empty());
    }

    #[test]
    fn typewriter_shares_the_text_box_look() {
        let mut looks = Looks::default();
        let look = Look {
            colour: [0.0; 3],
            width: 0.0,
            opacity: 1.0,
        };
        looks.0.insert(Tool::FreeText, look);
        assert_eq!(looks.get(Tool::Typewriter), Some(&look));
    }

    #[test]
    fn measurements_have_no_tool_look() {
        let mut a = Annotation::new(0, AnnotKind::Polygon(vec![[0.0, 0.0]; 3]));
        a.cloudy = true;
        assert_eq!(tool_of(&a), Some(Tool::Cloud));
        a.measure = Some(katna_pdf_cos::Scale::default());
        assert_eq!(tool_of(&a), None);
    }
}
