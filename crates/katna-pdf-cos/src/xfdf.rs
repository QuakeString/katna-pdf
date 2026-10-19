// SPDX-License-Identifier: GPL-3.0-or-later

//! XFDF (ISO 19444-1), the XML form of a file's annotations, to share
//! comments without sharing the PDF (docs/PLAN.md §4.2). Written and
//! read here without an XML library: XFDF is small and plain, and what
//! is read is only what [`Annotation`] holds.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::annots::{AnnotChanges, AnnotKind, Annotation, MarkupStyle};
use crate::document::Document;

/// Why XFDF could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XfdfError {
    /// Not XFDF: no `<xfdf>` element.
    NotXfdf,
}

impl std::fmt::Display for XfdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file is not XFDF")
    }
}

impl std::error::Error for XfdfError {}

/// `annots` (each page's, as [`crate::Document::annotations`] gives
/// them) as XFDF, for the PDF named `file`. Annotations that are not one
/// of ours ([`AnnotKind::Other`]) are left out.
pub fn to_xfdf(annots: &[Vec<Annotation>], file: &str) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" \
         xml:space=\"preserve\">\n",
    );
    let _ = writeln!(out, "<f href=\"{}\"/>\n<annots>", escape(file));
    // Replies name what they reply to by its name.
    let names: HashMap<_, _> = annots
        .iter()
        .flatten()
        .filter_map(|a| Some((a.id?, a.name.as_str())))
        .collect();
    for a in annots.iter().flatten() {
        let (tag, extra) = match &a.kind {
            AnnotKind::Markup(style, quads) => (
                match style {
                    MarkupStyle::Highlight => "highlight",
                    MarkupStyle::Underline => "underline",
                    MarkupStyle::StrikeOut => "strikeout",
                    MarkupStyle::Squiggly => "squiggly",
                },
                format!(" coords=\"{}\"", numbers(quads.as_flattened())),
            ),
            AnnotKind::Note => ("text", String::new()),
            AnnotKind::FreeText { .. } => ("freetext", String::new()),
            AnnotKind::Ink(_) => ("ink", format!(" width=\"{}\"", num(a.width))),
            AnnotKind::Square { fill } | AnnotKind::Circle { fill } => (
                if matches!(a.kind, AnnotKind::Square { .. }) {
                    "square"
                } else {
                    "circle"
                },
                format!(
                    " width=\"{}\"{}",
                    num(a.width),
                    fill.map(|f| format!(" interior-color=\"{}\"", hex(f)))
                        .unwrap_or_default()
                ),
            ),
            AnnotKind::Line { from, to, arrow } => (
                "line",
                format!(
                    " start=\"{}\" end=\"{}\" head=\"None\" tail=\"{}\" width=\"{}\"",
                    numbers(from),
                    numbers(to),
                    if *arrow { "OpenArrow" } else { "None" },
                    num(a.width)
                ),
            ),
            AnnotKind::Polygon(_) => ("polygon", format!(" width=\"{}\"", num(a.width))),
            AnnotKind::PolyLine(_) => ("polyline", format!(" width=\"{}\"", num(a.width))),
            AnnotKind::Stamp(name) => ("stamp", format!(" icon=\"{}\"", escape(name))),
            AnnotKind::Other(_) => continue,
        };
        let _ = write!(
            out,
            "<{tag} page=\"{}\" rect=\"{}\" color=\"{}\"",
            a.page,
            numbers(&a.rect),
            hex(a.colour)
        );
        if a.opacity < 1.0 {
            let _ = write!(out, " opacity=\"{}\"", num(a.opacity));
        }
        for (key, value) in [
            ("name", a.name.as_str()),
            ("title", a.author.as_str()),
            ("date", a.modified.as_deref().unwrap_or("")),
            (
                "inreplyto",
                a.reply_to
                    .and_then(|r| names.get(&r).copied())
                    .unwrap_or(""),
            ),
        ] {
            if !value.is_empty() {
                let _ = write!(out, " {key}=\"{}\"", escape(value));
            }
        }
        out.push_str(&extra);
        out.push_str(">\n");
        if !a.contents.is_empty() {
            let _ = writeln!(out, "<contents>{}</contents>", escape(&a.contents));
        }
        match &a.kind {
            AnnotKind::FreeText { size } => {
                let [r, g, b] = a.colour;
                let _ = writeln!(
                    out,
                    "<defaultappearance>/Helv {} Tf {} {} {} rg</defaultappearance>",
                    num(*size),
                    num(r),
                    num(g),
                    num(b)
                );
            }
            AnnotKind::Ink(strokes) => {
                out.push_str("<inklist>");
                for s in strokes {
                    let _ = write!(out, "<gesture>{}</gesture>", points(s));
                }
                out.push_str("</inklist>\n");
            }
            AnnotKind::Polygon(p) | AnnotKind::PolyLine(p) => {
                let _ = writeln!(out, "<vertices>{}</vertices>", points(p));
            }
            _ => {}
        }
        let _ = writeln!(out, "</{tag}>");
    }
    out.push_str("</annots>\n</xfdf>\n");
    out
}

/// The annotations in `xml`, with their pages and names; none has an
/// object in a file yet. A reply's `reply_to` is unset: its
/// [`XfdfAnnotation::reply_to`] names what it replies to.
pub fn from_xfdf(xml: &str) -> Result<Vec<XfdfAnnotation>, XfdfError> {
    let elements = parse(xml);
    if !elements.iter().any(|e| e.name == "xfdf") {
        return Err(XfdfError::NotXfdf);
    }
    let mut out = Vec::new();
    for e in &elements {
        if e.depth != 2 {
            continue;
        }
        let attr = |k: &str| e.attrs.get(k).map(String::as_str);
        let nums = |k: &str| attr(k).map(parse_numbers).unwrap_or_default();
        let child = |name: &str| {
            elements
                .iter()
                .filter(|c| c.parent == Some(e.index) && c.name == name)
                .map(|c| c.text.clone())
                .next()
        };
        let pts = |text: &str| -> Vec<[f32; 2]> { parse_numbers(text).as_chunks::<2>().0.to_vec() };
        let markup = |style| AnnotKind::Markup(style, nums("coords").as_chunks::<8>().0.to_vec());
        let kind = match e.name.as_str() {
            "highlight" => markup(MarkupStyle::Highlight),
            "underline" => markup(MarkupStyle::Underline),
            "strikeout" => markup(MarkupStyle::StrikeOut),
            "squiggly" => markup(MarkupStyle::Squiggly),
            "text" => AnnotKind::Note,
            "freetext" => AnnotKind::FreeText {
                size: child("defaultappearance")
                    .and_then(|da| {
                        let words: Vec<&str> = da.split_whitespace().collect();
                        let at = words.iter().position(|w| *w == "Tf")?;
                        words.get(at.checked_sub(1)?)?.parse().ok()
                    })
                    .unwrap_or(12.0),
            },
            "ink" => {
                let gestures: Vec<Vec<[f32; 2]>> = elements
                    .iter()
                    .filter(|g| {
                        g.name == "gesture"
                            && g.parent
                                .is_some_and(|p| elements[p].parent == Some(e.index))
                    })
                    .map(|g| pts(&g.text))
                    .collect();
                AnnotKind::Ink(gestures)
            }
            "square" | "circle" => {
                let fill = attr("interior-color").and_then(parse_hex);
                if e.name == "square" {
                    AnnotKind::Square { fill }
                } else {
                    AnnotKind::Circle { fill }
                }
            }
            "line" => {
                let (from, to) = (nums("start"), nums("end"));
                let (Some(from), Some(to)) = (
                    from.as_chunks::<2>().0.first(),
                    to.as_chunks::<2>().0.first(),
                ) else {
                    continue;
                };
                AnnotKind::Line {
                    from: *from,
                    to: *to,
                    arrow: attr("tail").is_some_and(|t| t != "None")
                        || attr("head").is_some_and(|h| h != "None"),
                }
            }
            "polygon" => AnnotKind::Polygon(child("vertices").map(|v| pts(&v)).unwrap_or_default()),
            "polyline" => {
                AnnotKind::PolyLine(child("vertices").map(|v| pts(&v)).unwrap_or_default())
            }
            "stamp" => AnnotKind::Stamp(attr("icon").unwrap_or("Draft").to_owned()),
            _ => continue,
        };
        let Some(page) = attr("page").and_then(|p| p.parse().ok()) else {
            continue;
        };
        let mut a = Annotation::new(page, kind);
        if let [x0, y0, x1, y1] = nums("rect")[..] {
            a.rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
        }
        if let Some(c) = attr("color").and_then(parse_hex) {
            a.colour = c;
        }
        if let Some(o) = attr("opacity").and_then(|o| o.parse().ok()) {
            a.opacity = o;
        }
        if let Some(w) = attr("width").and_then(|w| w.parse().ok()) {
            a.width = w;
        }
        a.contents = child("contents").unwrap_or_default();
        a.author = attr("title").unwrap_or_default().to_owned();
        a.modified = attr("date").map(str::to_owned);
        a.name = attr("name").unwrap_or_default().to_owned();
        out.push(XfdfAnnotation {
            annotation: a,
            reply_to: attr("inreplyto").map(str::to_owned),
        });
    }
    Ok(out)
}

impl Document {
    /// The changes that bring the annotations in `xml` into this file:
    /// one with the name of an annotation here replaces it, the others
    /// are added. Replies keep to what they reply to when it is here.
    /// Annotations on pages the file does not have are left out.
    pub fn xfdf_changes(&self, xml: &str) -> Result<AnnotChanges, XfdfError> {
        let existing: Vec<Annotation> = self.annotations().into_iter().flatten().collect();
        let named = |name: &str| {
            existing
                .iter()
                .find(|e| !name.is_empty() && e.name == name)
                .and_then(|e| e.id)
        };
        let pages = self.pages().len();
        let mut put = Vec::new();
        for x in from_xfdf(xml)? {
            let mut a = x.annotation;
            if a.page >= pages {
                continue;
            }
            a.id = named(&a.name);
            a.reply_to = x.reply_to.as_deref().and_then(named);
            put.push(a);
        }
        Ok(AnnotChanges {
            put,
            remove: Vec::new(),
        })
    }
}

/// An annotation read from XFDF, and the name of the one it replies to.
#[derive(Debug, Clone, PartialEq)]
pub struct XfdfAnnotation {
    pub annotation: Annotation,
    pub reply_to: Option<String>,
}

/// One element: its name, attributes and text, and where it sits.
pub(crate) struct Element {
    index: usize,
    pub(crate) parent: Option<usize>,
    depth: usize,
    pub(crate) name: String,
    pub(crate) attrs: HashMap<String, String>,
    pub(crate) text: String,
}

/// The elements of `xml` in document order, with namespace prefixes
/// dropped. Comments, the declaration and processing instructions are
/// skipped; text is gathered into the innermost open element.
pub(crate) fn parse(xml: &str) -> Vec<Element> {
    let mut out: Vec<Element> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        if let Some(&top) = open.last() {
            out[top].text.push_str(&unescape(&rest[..lt]));
        }
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |end| &body[end + 3..]);
            continue;
        }
        if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>").unwrap_or(body.len());
            if let Some(&top) = open.last() {
                out[top].text.push_str(&body[..end]);
            }
            rest = body.get(end + 3..).unwrap_or("");
            continue;
        }
        let Some(gt) = tag_end(rest) else {
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let name = local(name.trim());
            if let Some(at) = open.iter().rposition(|&i| out[i].name == name) {
                open.truncate(at);
            }
            continue;
        }
        let closed = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let name_end = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
        let index = out.len();
        out.push(Element {
            index,
            parent: open.last().copied(),
            depth: open.len(),
            name: local(&tag[..name_end]).to_owned(),
            attrs: attributes(&tag[name_end..]),
            text: String::new(),
        });
        if !closed {
            open.push(index);
        }
    }
    out
}

/// Where the tag starting `s` ends: its `>`, not one inside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

fn attributes(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s;
    while let Some(eq) = rest.find('=') {
        let key = local(rest[..eq].trim()).to_owned();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(end) = after[1..].find(q) else {
            break;
        };
        out.insert(key, unescape(&after[1..1 + end]));
        rest = &after[end + 2..];
    }
    out
}

/// A name without its namespace prefix.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..semi];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
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

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

fn num(v: f32) -> String {
    let v = (f64::from(v) * 1000.0).round() / 1000.0;
    format!("{v}")
}

fn numbers(values: &[f32]) -> String {
    values.iter().map(|v| num(*v)).collect::<Vec<_>>().join(",")
}

fn points(p: &[[f32; 2]]) -> String {
    p.iter()
        .map(|[x, y]| format!("{},{}", num(*x), num(*y)))
        .collect::<Vec<_>>()
        .join(";")
}

fn parse_numbers(s: &str) -> Vec<f32> {
    s.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter_map(|n| n.parse().ok())
        .collect()
}

fn hex([r, g, b]: [f32; 3]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", byte(r), byte(g), byte(b))
}

fn parse_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let byte = |i: usize| {
        u8::from_str_radix(&s[i..i + 2], 16)
            .ok()
            .map(|b| f32::from(b) / 255.0)
    };
    Some([byte(0)?, byte(2)?, byte(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_is_read() {
        let els = parse(
            "<?xml version=\"1.0\"?><!-- c --><x:a k='1 &amp; 2'>t<b/><c>&lt;&#233;</c></x:a>",
        );
        let names: Vec<_> = els.iter().map(|e| (e.name.as_str(), e.depth)).collect();
        assert_eq!(names, [("a", 0), ("b", 1), ("c", 1)]);
        assert_eq!(els[0].attrs["k"], "1 & 2");
        assert_eq!(els[2].text, "<é");
        assert_eq!(els[2].parent, Some(0));
    }

    #[test]
    fn colours_and_numbers() {
        assert_eq!(hex([1.0, 0.85, 0.0]), "#FFD900");
        assert_eq!(parse_hex("#FF0000"), Some([1.0, 0.0, 0.0]));
        assert_eq!(parse_numbers("1,2;3 4"), [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(escape("a<\"b\">\n"), "a&lt;&quot;b&quot;&gt;&#10;");
    }
}
