// SPDX-License-Identifier: GPL-3.0-or-later

//! Rich text in a free text comment (§12.7.3.4, "rich text strings"):
//! the `/RC` XHTML Acrobat writes, read into paragraphs of styled spans
//! (bold, italic, underline, strikeout, colour, size and a family) and
//! written back, and laid out into the comment's appearance, wrapped to
//! its box and aligned, in the standard fonts readers have.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::fonts;

/// The family of a span's font: the standard fonts every reader has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Family {
    /// Helvetica.
    #[default]
    Sans,
    /// Times.
    Serif,
    /// Courier.
    Mono,
}

impl Family {
    fn base(self) -> &'static str {
        match self {
            Family::Sans => "Helvetica",
            Family::Serif => "Times",
            Family::Mono => "Courier",
        }
    }

    fn css(self) -> &'static str {
        match self {
            Family::Sans => "Helvetica,sans-serif",
            Family::Serif => "Times New Roman,serif",
            Family::Mono => "Courier New,monospace",
        }
    }

    /// The family a CSS `font-family` list names first that we know.
    fn of_css(list: &str) -> Family {
        let lower = list.to_ascii_lowercase();
        for name in lower.split(',') {
            let name = name.trim().trim_matches(['\'', '"']);
            if ["mono", "courier", "consol", "typewriter"]
                .iter()
                .any(|w| name.contains(w))
            {
                return Family::Mono;
            }
            if name == "serif"
                || ["times", "roman", "georgia", "garamond", "cambria", "minion"]
                    .iter()
                    .any(|w| name.contains(w))
            {
                return Family::Serif;
            }
            if !name.is_empty() {
                return Family::Sans;
            }
        }
        Family::Sans
    }
}

/// How a paragraph's lines sit in the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParaAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// How a span of text looks. `None` colour and size are the comment's
/// own (its `/DA`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub family: Family,
    pub colour: Option<[f32; 3]>,
    pub size: Option<f32>,
}

/// A stretch of a paragraph's text in one style.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichSpan {
    pub text: String,
    pub style: SpanStyle,
}

/// A paragraph of rich text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RichPara {
    pub spans: Vec<RichSpan>,
    pub align: ParaAlign,
}

impl RichPara {
    /// Its text, without styles.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    /// `text` added in `style`, joined to the last span when it looks
    /// the same.
    pub fn push(&mut self, text: &str, style: &SpanStyle) {
        if text.is_empty() {
            return;
        }
        match self.spans.last_mut() {
            Some(last) if last.style == *style => last.text.push_str(text),
            _ => self.spans.push(RichSpan {
                text: text.to_owned(),
                style: style.clone(),
            }),
        }
    }
}

/// The text of `paras`, a line a paragraph.
pub fn rich_plain_text(paras: &[RichPara]) -> String {
    paras
        .iter()
        .map(RichPara::text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` as paragraphs in one style: a line a paragraph.
pub fn plain_paras(text: &str, style: &SpanStyle, align: ParaAlign) -> Vec<RichPara> {
    text.split('\n')
        .map(|line| {
            let mut p = RichPara {
                spans: Vec::new(),
                align,
            };
            p.push(line.trim_end_matches('\r'), style);
            p
        })
        .collect()
}

/// Whether `paras` say more than their text: any span styled, or a
/// paragraph not to the left.
pub fn is_styled(paras: &[RichPara]) -> bool {
    paras.iter().any(|p| {
        p.align != ParaAlign::Left || p.spans.iter().any(|s| s.style != SpanStyle::default())
    })
}

/// A block's own style and alignment, from its tag and `style`.
#[derive(Clone)]
struct Level {
    style: SpanStyle,
    align: ParaAlign,
    block: bool,
}

/// Reads an `/RC` rich text string (XHTML) into paragraphs. The body's
/// style is the default for what it holds, as is every element's for
/// its own.
pub fn parse_rc(xml: &str) -> Vec<RichPara> {
    let mut paras: Vec<RichPara> = Vec::new();
    let mut current: Option<RichPara> = None;
    let mut stack: Vec<Level> = vec![Level {
        style: SpanStyle::default(),
        align: ParaAlign::Left,
        block: true,
    }];
    let mut rest = xml;
    // A line break was the last thing read: the paragraph it opened ends
    // with the block, and says nothing of its own.
    let mut after_break = false;
    // Each paragraph ends when its block does, or at a line break.
    let close = |current: &mut Option<RichPara>, paras: &mut Vec<RichPara>| {
        if let Some(p) = current.take() {
            paras.push(p);
        }
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            text_node(rest, &stack, &mut current);
            break;
        };
        if lt > 0 {
            text_node(&rest[..lt], &stack, &mut current);
            if current.as_ref().is_some_and(|p| !p.spans.is_empty()) {
                after_break = false;
            }
        }
        rest = &rest[lt..];
        // Comments, processing instructions and declarations.
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map_or("", |e| &rest[e + 3..]);
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
            continue;
        }
        let Some(gt) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if let Some(name) = tag.strip_prefix('/') {
            let name = local(name.trim());
            if stack.len() > 1 {
                let level = stack.pop();
                if level.is_some_and(|l| l.block) || is_block(name) {
                    if after_break && current.as_ref().is_some_and(|p| p.spans.is_empty()) {
                        current = None;
                    }
                    close(&mut current, &mut paras);
                    after_break = false;
                }
            }
            continue;
        }
        let empty = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, attrs) = match tag.find(char::is_whitespace) {
            Some(i) => (&tag[..i], &tag[i..]),
            None => (tag, ""),
        };
        let name = local(name);
        let parent = stack.last().cloned().unwrap_or(Level {
            style: SpanStyle::default(),
            align: ParaAlign::Left,
            block: true,
        });
        if name == "br" {
            // A line break ends the paragraph; the next keeps its look.
            let align = current.as_ref().map_or(parent.align, |p| p.align);
            let p = current.take().unwrap_or(RichPara {
                spans: Vec::new(),
                align,
            });
            paras.push(p);
            current = Some(RichPara {
                spans: Vec::new(),
                align,
            });
            after_break = true;
            continue;
        }
        let mut level = Level {
            style: parent.style.clone(),
            align: parent.align,
            block: is_block(name),
        };
        match name {
            "b" | "strong" => level.style.bold = true,
            "i" | "em" => level.style.italic = true,
            "u" => level.style.underline = true,
            "s" | "strike" | "del" => level.style.strike = true,
            _ => {}
        }
        if let Some(style) = attribute(attrs, "style") {
            // The body's size and colour are the comment's own (its
            // `/DA` says them too): spans keep none unless theirs differ.
            let (size, colour) = (level.style.size, level.style.colour);
            apply_css(&style, &mut level.style, &mut level.align);
            if name == "body" {
                level.style.size = size;
                level.style.colour = colour;
            }
        }
        if level.block {
            // A paragraph opened by an outer block and still empty was
            // only its start.
            if current.as_ref().is_some_and(|p| p.spans.is_empty()) && !after_break {
                current = None;
            }
            close(&mut current, &mut paras);
            after_break = false;
            if !empty && name != "body" {
                current = Some(RichPara {
                    spans: Vec::new(),
                    align: level.align,
                });
            }
        }
        if !empty {
            stack.push(level);
        }
    }
    if let Some(p) = current.filter(|p| !p.spans.is_empty() || !after_break) {
        paras.push(p);
    }
    paras
}

/// Text between tags, added to the paragraph open (one opened when
/// none is); white space between blocks is not text.
fn text_node(raw: &str, stack: &[Level], current: &mut Option<RichPara>) {
    let text = unescape(raw);
    let Some(level) = stack.last() else {
        return;
    };
    // Line breaks and indents laid out between tags are not text.
    if text.trim().is_empty() && raw.contains('\n') {
        return;
    }
    if current.is_none() {
        if text.trim().is_empty() {
            return;
        }
        *current = Some(RichPara {
            spans: Vec::new(),
            align: level.align,
        });
    }
    if let Some(p) = current.as_mut() {
        p.push(&text.replace(['\r', '\n'], ""), &level.style);
    }
}

/// A tag's name without its namespace prefix, in lower case.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn is_block(name: &str) -> bool {
    matches!(name, "p" | "div" | "body" | "li" | "h1" | "h2" | "h3")
}

/// The value of attribute `name` in a tag's attributes, unescaped.
fn attribute(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = after[1..].find(quote)?;
        let value = &after[1..1 + end];
        if local(key) == name {
            return Some(unescape(value));
        }
        rest = &after[end + 2..];
    }
    None
}

/// What a CSS declaration list says of a span and its paragraph.
fn apply_css(css: &str, style: &mut SpanStyle, align: &mut ParaAlign) {
    for part in css.split(';') {
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        match key.trim().to_ascii_lowercase().as_str() {
            "font-weight" => {
                style.bold = lower == "bold"
                    || lower == "bolder"
                    || lower.parse::<u32>().is_ok_and(|n| n >= 600);
            }
            "font-style" => style.italic = lower == "italic" || lower == "oblique",
            "text-decoration" | "text-decoration-line" => {
                style.underline = lower.contains("underline") || lower.contains("word");
                style.strike = lower.contains("line-through");
            }
            "color" => {
                if let Some(c) = css_colour(&lower) {
                    style.colour = Some(c);
                }
            }
            "font-size" => {
                if let Some(size) = css_size(&lower) {
                    style.size = Some(size);
                }
            }
            "font-family" => style.family = Family::of_css(value),
            "font" => {
                // The shorthand: style, weight, size and family.
                for word in lower.split_whitespace() {
                    match word {
                        "bold" => style.bold = true,
                        "italic" | "oblique" => style.italic = true,
                        _ => {
                            if let Some(size) = css_size(word.split('/').next().unwrap_or(word)) {
                                style.size = Some(size);
                            }
                        }
                    }
                }
                if let Some(i) = lower.find(|c: char| c.is_ascii_digit())
                    && let Some(space) = lower[i..].find(' ')
                {
                    style.family = Family::of_css(&value[i + space..]);
                }
            }
            "text-align" => {
                *align = match lower.as_str() {
                    "center" => ParaAlign::Center,
                    "right" | "end" => ParaAlign::Right,
                    _ => ParaAlign::Left,
                };
            }
            _ => {}
        }
    }
}

/// A CSS length as points: `12pt`, `16px`, `1.2em` (of 12 points).
fn css_size(s: &str) -> Option<f32> {
    let s = s.trim();
    let (number, factor) = if let Some(n) = s.strip_suffix("pt") {
        (n, 1.0)
    } else if let Some(n) = s.strip_suffix("px") {
        (n, 0.75)
    } else if let Some(n) = s.strip_suffix("em") {
        (n, 12.0)
    } else {
        (s, 1.0)
    };
    let n: f32 = number.trim().parse().ok()?;
    (n > 0.0 && n < 1000.0).then_some(n * factor)
}

/// A CSS colour: `#rgb`, `#rrggbb`, `rgb(r,g,b)` or a few names.
fn css_colour(s: &str) -> Option<[f32; 3]> {
    if let Some(hex) = s.strip_prefix('#') {
        let digit = |i: usize, n: usize| u8::from_str_radix(hex.get(i..i + n)?, 16).ok();
        return match hex.len() {
            6 => Some([
                f32::from(digit(0, 2)?) / 255.0,
                f32::from(digit(2, 2)?) / 255.0,
                f32::from(digit(4, 2)?) / 255.0,
            ]),
            3 => Some([
                f32::from(digit(0, 1)?) / 15.0,
                f32::from(digit(1, 1)?) / 15.0,
                f32::from(digit(2, 1)?) / 15.0,
            ]),
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<f32> = inner
            .split(',')
            .filter_map(|p| p.trim().parse::<f32>().ok())
            .collect();
        if let [r, g, b] = parts[..] {
            return Some([r / 255.0, g / 255.0, b / 255.0]);
        }
    }
    match s {
        "black" => Some([0.0; 3]),
        "white" => Some([1.0; 3]),
        "red" => Some([1.0, 0.0, 0.0]),
        "green" => Some([0.0, 0.5, 0.0]),
        "blue" => Some([0.0, 0.0, 1.0]),
        _ => None,
    }
}

/// XML text with its entities read.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|&i| i < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `text` safe in XML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn hex(c: [f32; 3]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// `paras` as an `/RC` rich text string, in the form Acrobat writes, with
/// `size` and `colour` (the comment's own) as the body's style.
pub fn to_rc(paras: &[RichPara], size: f32, colour: [f32; 3]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" \
         xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:APIVersion=\"Acrobat:11.0.0\" \
         xfa:spec=\"2.0.2\" ",
    );
    let _ = write!(
        out,
        "style=\"font-size:{size:.1}pt;text-align:left;color:{};font-weight:normal;\
         font-style:normal;font-family:Helvetica,sans-serif;font-stretch:normal\">",
        hex(colour)
    );
    for p in paras {
        out.push_str("<p dir=\"ltr\"");
        match p.align {
            ParaAlign::Left => {}
            ParaAlign::Center => out.push_str(" style=\"text-align:center\""),
            ParaAlign::Right => out.push_str(" style=\"text-align:right\""),
        }
        out.push('>');
        for s in &p.spans {
            let css = span_css(&s.style);
            if css.is_empty() {
                out.push_str(&escape(&s.text));
            } else {
                let _ = write!(out, "<span style=\"{css}\">{}</span>", escape(&s.text));
            }
        }
        out.push_str("</p>");
    }
    out.push_str("</body>");
    out
}

/// What a span's style says that the body's does not, as CSS.
fn span_css(s: &SpanStyle) -> String {
    let mut parts: Vec<String> = Vec::new();
    if s.bold {
        parts.push("font-weight:bold".into());
    }
    if s.italic {
        parts.push("font-style:italic".into());
    }
    match (s.underline, s.strike) {
        (true, true) => parts.push("text-decoration:underline line-through".into()),
        (true, false) => parts.push("text-decoration:underline".into()),
        (false, true) => parts.push("text-decoration:line-through".into()),
        _ => {}
    }
    if let Some(c) = s.colour {
        parts.push(format!("color:{}", hex(c)));
    }
    if let Some(size) = s.size {
        parts.push(format!("font-size:{size:.1}pt"));
    }
    if s.family != Family::Sans {
        parts.push(format!("font-family:{}", s.family.css()));
    }
    parts.join(";")
}

/// The name a standard font goes by in an appearance's resources (those
/// Acrobat's forms use).
pub(crate) fn resource_name(base: &str) -> &'static str {
    match base {
        "Helvetica-Bold" => "HeBo",
        "Helvetica-Oblique" => "HeOb",
        "Helvetica-BoldOblique" => "HeBO",
        "Times-Roman" => "TiRo",
        "Times-Bold" => "TiBo",
        "Times-Italic" => "TiIt",
        "Times-BoldItalic" => "TiBI",
        "Courier" => "Cour",
        "Courier-Bold" => "CoBo",
        "Courier-Oblique" => "CoOb",
        "Courier-BoldOblique" => "CoBO",
        _ => "Helv",
    }
}

/// A character laid out: what it is, its span's style, its width.
struct Glyph<'a> {
    c: char,
    style: &'a SpanStyle,
    width: f32,
}

/// Draws `paras` into the box `rect` (left, bottom, right, top, in
/// points), inset by `pad`, wrapped at spaces to its width and clipped
/// to it; `size` and `colour` are the text's unless a span says other.
/// Writes the operators to `c`, and returns the standard fonts used.
pub(crate) fn draw(
    c: &mut String,
    paras: &[RichPara],
    rect: [f32; 4],
    pad: f32,
    size: f32,
    colour: [f32; 3],
) -> BTreeSet<&'static str> {
    let mut used = BTreeSet::new();
    let [x0, y0, x1, y1] = rect;
    let left = x0 + pad;
    let room = (x1 - x0 - 2.0 * pad).max(1.0);
    let mut y = y1 - pad;
    let _ = writeln!(c, "q {x0:.2} {y0:.2} {:.2} {:.2} re W n", x1 - x0, y1 - y0);
    for p in paras {
        let glyphs: Vec<Glyph> = p
            .spans
            .iter()
            .flat_map(|s| {
                let (_, metrics) = font_of(&s.style);
                let sz = s.style.size.unwrap_or(size);
                s.text.chars().map(move |ch| Glyph {
                    c: ch,
                    style: &s.style,
                    width: char_width(metrics, ch) * sz,
                })
            })
            .collect();
        for line in wrap(&glyphs, room) {
            let line = &glyphs[line];
            let tallest = line
                .iter()
                .map(|g| g.style.size.unwrap_or(size))
                .fold(0.0f32, f32::max);
            let tallest = if tallest > 0.0 {
                tallest
            } else {
                p.spans.first().and_then(|s| s.style.size).unwrap_or(size)
            };
            let baseline = y - tallest;
            y -= tallest * 1.2;
            // Spaces at the end of a line take no room.
            let shown = line.iter().rposition(|g| g.c != ' ').map_or(0, |i| i + 1);
            let width: f32 = line[..shown].iter().map(|g| g.width).sum();
            let mut x = match p.align {
                ParaAlign::Left => left,
                ParaAlign::Center => left + (room - width) / 2.0,
                ParaAlign::Right => left + room - width,
            };
            let mut i = 0;
            while i < shown {
                let style = line[i].style;
                let mut j = i;
                let mut text = String::new();
                let mut w = 0.0;
                while j < shown && std::ptr::eq(line[j].style, style) {
                    text.push(line[j].c);
                    w += line[j].width;
                    j += 1;
                }
                let (base, _) = font_of(style);
                let name = resource_name(base);
                used.insert(base);
                let sz = style.size.unwrap_or(size);
                let [r, g, b] = style.colour.unwrap_or(colour);
                let _ = writeln!(
                    c,
                    "BT /{name} {sz:.2} Tf {r:.3} {g:.3} {b:.3} rg 1 0 0 1 {x:.2} {baseline:.2} Tm \
                     {} Tj ET",
                    crate::forms::literal(&text)
                );
                // Lines under and through, a twentieth of the size thick.
                for (on, rise) in [(style.underline, -0.12), (style.strike, 0.28)] {
                    if on {
                        let t = (sz * 0.05).max(0.5);
                        let _ = writeln!(
                            c,
                            "{r:.3} {g:.3} {b:.3} rg {x:.2} {:.2} {w:.2} {t:.2} re f",
                            baseline + rise * sz - t / 2.0
                        );
                    }
                }
                x += w;
                i = j;
            }
        }
    }
    c.push_str("Q\n");
    used
}

/// The standard font a span is drawn in, and its widths.
fn font_of(style: &SpanStyle) -> (&'static str, &'static phf::Map<&'static str, f32>) {
    fonts::standard_style(style.family.base(), style.bold, style.italic)
}

/// A character's width in a standard font, per point of size; one the
/// font's encoding lacks is drawn as `?`.
fn char_width(metrics: &phf::Map<&'static str, f32>, c: char) -> f32 {
    let code = fonts::win_ansi_code(c).map_or(b'?', |(code, _)| code);
    (fonts::win_ansi_width(metrics, code) / 1000.0) as f32
}

/// The lines `glyphs` break into within `room`: after the last space
/// that fits, or within a word longer than the line. An empty paragraph
/// is one empty line.
fn wrap(glyphs: &[Glyph], room: f32) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut start = 0;
    while start < glyphs.len() {
        let mut width = 0.0;
        let mut end = start;
        let mut space = None;
        while end < glyphs.len() {
            let g = &glyphs[end];
            if g.c == ' ' {
                space = Some(end);
            } else if width + g.width > room && end > start {
                break;
            }
            width += g.width;
            end += 1;
        }
        if end < glyphs.len() {
            // Broken at the last space, the space kept on this line.
            if let Some(s) = space.filter(|&s| s >= start) {
                end = s + 1;
            }
        }
        lines.push(start..end);
        start = end;
    }
    if lines.is_empty() {
        lines.push(0..0);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACROBAT: &str = "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" \
        xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:APIVersion=\"Acrobat:21.1.0\" \
        xfa:spec=\"2.0.2\" style=\"font-size:12.0pt;text-align:left;color:#000000;font-weight:normal;\
        font-style:normal;font-family:Helvetica,sans-serif;font-stretch:normal\"><p dir=\"ltr\">\
        <span style=\"font-family:Helvetica\">Plain </span><span style=\"font-weight:bold\">bold\
        </span><span style=\"color:#FF0000;font-style:italic\"> red &amp; italic</span></p>\
        <p dir=\"ltr\" style=\"text-align:center\"><span style=\"text-decoration:underline;\
        font-size:18pt\">Big</span></p></body>";

    #[test]
    fn acrobat_rich_text_is_read() {
        let paras = parse_rc(ACROBAT);
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text(), "Plain bold red & italic");
        assert_eq!(paras[0].spans.len(), 3);
        assert!(paras[0].spans[1].style.bold);
        assert_eq!(paras[0].spans[2].style.colour, Some([1.0, 0.0, 0.0]));
        assert!(paras[0].spans[2].style.italic);
        assert_eq!(paras[1].align, ParaAlign::Center);
        assert!(paras[1].spans[0].style.underline);
        assert_eq!(paras[1].spans[0].style.size, Some(18.0));
        // The body's colour and size are the comment's own.
        assert_eq!(paras[0].spans[0].style, SpanStyle::default());
    }

    #[test]
    fn written_rich_text_reads_back_the_same() {
        let mut p = RichPara::default();
        p.push("Hello ", &SpanStyle::default());
        p.push(
            "<world>",
            &SpanStyle {
                bold: true,
                strike: true,
                family: Family::Serif,
                colour: Some([0.0, 0.0, 1.0]),
                ..SpanStyle::default()
            },
        );
        let paras = vec![
            p,
            RichPara {
                spans: Vec::new(),
                align: ParaAlign::Right,
            },
            RichPara {
                spans: vec![RichSpan {
                    text: "end".into(),
                    style: SpanStyle {
                        size: Some(9.0),
                        ..SpanStyle::default()
                    },
                }],
                align: ParaAlign::Right,
            },
        ];
        let back = parse_rc(&to_rc(&paras, 12.0, [0.0; 3]));
        let plain = |paras: &[RichPara]| -> Vec<(String, bool, bool, Family, ParaAlign)> {
            paras
                .iter()
                .flat_map(|p| {
                    p.spans.iter().map(move |s| {
                        (
                            s.text.clone(),
                            s.style.bold,
                            s.style.strike,
                            s.style.family,
                            p.align,
                        )
                    })
                })
                .collect()
        };
        assert_eq!(plain(&back), plain(&paras));
        assert_eq!(back.len(), 3);
        assert_eq!(back[2].spans[0].style.size, Some(9.0));
        assert_eq!(rich_plain_text(&back), "Hello <world>\n\nend");
    }

    #[test]
    fn lines_wrap_at_spaces_and_inside_long_words() {
        let style = SpanStyle::default();
        let glyphs: Vec<Glyph> = "aa bb cccccc"
            .chars()
            .map(|c| Glyph {
                c,
                style: &style,
                width: 1.0,
            })
            .collect();
        let lines: Vec<String> = wrap(&glyphs, 5.0)
            .into_iter()
            .map(|r| glyphs[r].iter().map(|g| g.c).collect())
            .collect();
        assert_eq!(lines, ["aa bb ", "ccccc", "c"]);
    }

    #[test]
    fn drawn_text_uses_each_spans_font() {
        let paras = parse_rc(ACROBAT);
        let mut c = String::new();
        let used = draw(
            &mut c,
            &paras,
            [0.0, 0.0, 300.0, 100.0],
            2.0,
            12.0,
            [0.0; 3],
        );
        assert!(used.contains("Helvetica-Bold"));
        assert!(used.contains("Helvetica-Oblique"));
        assert!(c.contains("/HeBo 12.00 Tf"));
        assert!(c.contains("1.000 0.000 0.000 rg"));
        // The underline under "Big", centred.
        assert!(c.contains(" re f"));
        assert!(c.contains("(Big) Tj"));
    }
}
