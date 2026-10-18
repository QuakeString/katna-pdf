// SPDX-License-Identifier: GPL-3.0-or-later

//! What a page's fonts say about the text drawn with them, for editing
//! it (docs/PLAN.md §4.3): how a string's bytes split into character
//! codes, the Unicode each code stands for, how wide each is, and back
//! from text to codes for the glyphs the font has.

use std::collections::{HashMap, HashSet};

use hayro_cmap::{BfString, CMap};

use crate::document::Document;
use crate::filter::decode;
use crate::object::{Dict, Object};

mod glyph_names;
mod mac_roman;
mod metrics;
mod standard;
mod symbol;
mod win_ansi;
mod zapf_dings;

/// A font as a page's resources name it.
pub(crate) struct Font {
    /// Its name (`/BaseFont`), without a subset's tag.
    pub base: String,
    /// One byte per code, with glyph names from its encoding.
    simple: Option<Box<[Option<String>; 256]>>,
    /// A composite font's codes to glyph ids (CIDs).
    cmap: Option<CMap>,
    /// Whether codes are two bytes each, as `Identity-H` has them.
    identity: bool,
    to_unicode: Option<CMap>,
    /// A simple font's widths, by code, in glyph space.
    widths: HashMap<u32, f64>,
    /// A composite font's widths, by CID.
    cid_widths: HashMap<u32, f64>,
    missing: f64,
    /// Glyph space to text space: a thousandth, except in Type 3 fonts.
    pub scale: f64,
    /// The widths of a standard font that is not embedded.
    standard: Option<&'static phf::Map<&'static str, f32>>,
    /// Holds only some of its glyphs (a tag such as `ABCDEF+` before its
    /// name); which ones, when the file says (`/CharSet`, a Type 3 font's
    /// procedures).
    subset: bool,
    glyphs: Option<HashSet<String>>,
}

impl Font {
    pub fn load(doc: &Document, dict: &Dict) -> Font {
        let subtype = dict.name("Subtype").unwrap_or_default().to_vec();
        let raw_base =
            String::from_utf8_lossy(dict.name("BaseFont").unwrap_or_default()).into_owned();
        let subset = raw_base.len() > 7
            && raw_base.as_bytes()[6] == b'+'
            && raw_base.as_bytes()[..6].iter().all(u8::is_ascii_uppercase);
        let base = if subset {
            raw_base[7..].to_string()
        } else {
            raw_base
        };
        let to_unicode = match doc.resolve_key(dict, "ToUnicode") {
            Object::Stream(s) => decode(&s.dict, &s.data)
                .ok()
                .and_then(|data| CMap::parse(&data, |_| None)),
            _ => None,
        };
        let mut font = Font {
            base,
            simple: None,
            cmap: None,
            identity: false,
            to_unicode,
            widths: HashMap::new(),
            cid_widths: HashMap::new(),
            missing: 0.0,
            scale: 0.001,
            standard: None,
            subset,
            glyphs: None,
        };
        if subtype == b"Type0" {
            font.load_composite(doc, dict);
        } else {
            font.load_simple(doc, dict, &subtype);
        }
        font
    }

    fn load_composite(&mut self, doc: &Document, dict: &Dict) {
        match doc.resolve_key(dict, "Encoding") {
            Object::Name(n) if n.as_bytes() == b"Identity-H" || n.as_bytes() == b"Identity-V" => {
                self.identity = true;
                self.cmap = Some(CMap::identity_h());
            }
            Object::Stream(s) => {
                self.cmap = decode(&s.dict, &s.data)
                    .ok()
                    .and_then(|data| CMap::parse(&data, |_| None));
            }
            _ => {}
        }
        let descendant = match doc.resolve_key(dict, "DescendantFonts") {
            Object::Array(a) => a.first().map(|d| doc.resolve(d)),
            _ => None,
        };
        let Some(Object::Dict(cid_font)) = descendant else {
            return;
        };
        self.missing = doc
            .resolve_key(&cid_font, "DW")
            .as_number()
            .unwrap_or(1000.0);
        if let Object::Array(w) = doc.resolve_key(&cid_font, "W") {
            let w: Vec<Object> = w.iter().map(|o| doc.resolve(o)).collect();
            let mut i = 0;
            while i + 1 < w.len() {
                let Some(first) = w[i].as_int() else {
                    break;
                };
                match &w[i + 1] {
                    Object::Array(list) => {
                        for (k, width) in list.iter().enumerate() {
                            if let Some(width) = doc.resolve(width).as_number() {
                                self.cid_widths.insert(first as u32 + k as u32, width);
                            }
                        }
                        i += 2;
                    }
                    last => {
                        let (Some(last), Some(width)) =
                            (last.as_int(), w.get(i + 2).and_then(Object::as_number))
                        else {
                            break;
                        };
                        // Ranges this long are mistakes; keep within reason.
                        for cid in first..=last.min(first + 0xFFFF) {
                            self.cid_widths.insert(cid as u32, width);
                        }
                        i += 3;
                    }
                }
            }
        }
        if let Object::Dict(descriptor) = doc.resolve_key(&cid_font, "FontDescriptor") {
            self.missing_from(doc, &descriptor);
            if self.missing == 0.0 {
                self.missing = doc
                    .resolve_key(&cid_font, "DW")
                    .as_number()
                    .unwrap_or(1000.0);
            }
        }
    }

    fn missing_from(&mut self, doc: &Document, descriptor: &Dict) {
        if let Some(w) = doc.resolve_key(descriptor, "MissingWidth").as_number() {
            self.missing = w;
        }
    }

    fn load_simple(&mut self, doc: &Document, dict: &Dict, subtype: &[u8]) {
        let descriptor = match doc.resolve_key(dict, "FontDescriptor") {
            Object::Dict(d) => Some(d),
            _ => None,
        };
        let embedded = descriptor.as_ref().is_some_and(|d| {
            ["FontFile", "FontFile2", "FontFile3"]
                .iter()
                .any(|k| d.get(k).is_some())
        });
        if let Some(d) = &descriptor {
            self.missing_from(doc, d);
            if let Object::String { bytes, .. } = doc.resolve_key(d, "CharSet") {
                let names = String::from_utf8_lossy(&bytes)
                    .split('/')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect();
                self.glyphs = Some(names);
            }
        }
        let standard = if embedded || subtype == b"Type3" {
            None
        } else {
            standard_font(&self.base)
        };
        self.standard = standard.map(|(_, m)| m);
        if subtype == b"Type3" {
            if let Object::Array(m) = doc.resolve_key(dict, "FontMatrix")
                && let Some(a) = m.first().and_then(|a| doc.resolve(a).as_number())
            {
                self.scale = a;
            }
            if let Object::Dict(procs) = doc.resolve_key(dict, "CharProcs") {
                self.glyphs = Some(
                    procs
                        .iter()
                        .map(|(n, _)| String::from_utf8_lossy(n.as_bytes()).into_owned())
                        .collect(),
                );
                // Every glyph is listed, as if a subset.
                self.subset = true;
            }
        }
        if let (Some(first), Object::Array(widths)) = (
            doc.resolve_key(dict, "FirstChar").as_int(),
            doc.resolve_key(dict, "Widths"),
        ) {
            for (i, w) in widths.iter().enumerate() {
                if let Some(w) = doc.resolve(w).as_number() {
                    self.widths.insert((first + i as i64) as u32, w);
                }
            }
        }
        // The encoding: a base and its differences.
        let family = standard.map(|(name, _)| name).unwrap_or_default();
        let mut base: fn(u8) -> Option<&'static str> = match family {
            "Symbol" => symbol::get,
            "ZapfDingbats" => zapf_dings::get,
            _ => standard::get,
        };
        let mut differences = Vec::new();
        let encoding = doc.resolve_key(dict, "Encoding");
        let base_name = match &encoding {
            Object::Name(n) => Some(n.as_bytes().to_vec()),
            Object::Dict(d) => {
                if let Object::Array(diffs) = doc.resolve_key(d, "Differences") {
                    differences = diffs;
                }
                d.name("BaseEncoding").map(<[u8]>::to_vec)
            }
            _ => None,
        };
        match base_name.as_deref() {
            Some(b"WinAnsiEncoding") => base = win_ansi::get,
            Some(b"MacRomanEncoding") => base = mac_roman::get,
            Some(b"StandardEncoding") => base = standard::get,
            _ => {}
        }
        let mut names: Box<[Option<String>; 256]> = Box::new(std::array::from_fn(|code| {
            base(code as u8).map(str::to_string)
        }));
        let mut code = 0usize;
        for item in differences {
            match doc.resolve(&item) {
                Object::Int(c) => code = c.clamp(0, 255) as usize,
                Object::Name(n) => {
                    if code < 256 {
                        names[code] = Some(String::from_utf8_lossy(n.as_bytes()).into_owned());
                    }
                    code += 1;
                }
                _ => {}
            }
        }
        self.simple = Some(names);
    }

    /// `bytes` split into character codes, with each code's length.
    pub fn codes(&self, bytes: &[u8]) -> Vec<(u32, usize)> {
        if self.simple.is_some() || self.cmap.is_none() && !self.identity {
            if self.simple.is_some() {
                return bytes.iter().map(|b| (u32::from(*b), 1)).collect();
            }
            // A composite font with an encoding not read: two bytes each,
            // the commonest.
            return bytes
                .chunks(2)
                .map(|c| (c.iter().fold(0, |a, b| a << 8 | u32::from(*b)), c.len()))
                .collect();
        }
        if self.identity {
            return bytes
                .chunks(2)
                .map(|c| (c.iter().fold(0, |a, b| a << 8 | u32::from(*b)), c.len()))
                .collect();
        }
        let Some(cmap) = &self.cmap else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let mut code = 0u32;
            let mut found = None;
            for n in 0..4.min(bytes.len() - i) {
                code = code << 8 | u32::from(bytes[i + n]);
                if cmap.lookup_cid_code(code, n as u8 + 1).is_some() {
                    found = Some((code, n + 1));
                    break;
                }
            }
            let (code, len) = found.unwrap_or((u32::from(bytes[i]), 1));
            out.push((code, len));
            i += len;
        }
        out
    }

    fn glyph_name(&self, code: u32) -> Option<&str> {
        self.simple.as_ref()?.get(code as usize)?.as_deref()
    }

    /// The text code `code` stands for.
    pub fn unicode(&self, code: u32) -> Option<String> {
        if let Some(map) = &self.to_unicode
            && let Some(found) = map.lookup_bf_string(code)
        {
            return Some(match found {
                BfString::Char(c) => c.to_string(),
                BfString::String(s) => s,
            });
        }
        let name = self.glyph_name(code)?;
        glyph_unicode(name)
    }

    /// How far code `code` moves the pen, in text space at size 1.
    pub fn width(&self, code: u32) -> f64 {
        if self.simple.is_some() {
            if let Some(w) = self.widths.get(&code) {
                return w * self.scale;
            }
            if let (Some(metrics), Some(name)) = (self.standard, self.glyph_name(code))
                && let Some(w) = metrics.get(name)
            {
                return f64::from(*w) * self.scale;
            }
            return self.missing * self.scale;
        }
        let cid = if self.identity {
            Some(code)
        } else {
            self.cmap
                .as_ref()
                .and_then(|c| (1..=4u8).find_map(|n| c.lookup_cid_code(code, n)))
        };
        let w = cid
            .and_then(|cid| self.cid_widths.get(&cid).copied())
            .unwrap_or(self.missing);
        w * self.scale
    }

    /// Whether code `code` is one byte, as word spacing needs.
    pub fn single_byte(&self) -> bool {
        self.simple.is_some()
    }

    /// The bytes of code `code` in a string.
    pub fn code_bytes(&self, code: u32, out: &mut Vec<u8>) {
        let len = if self.simple.is_some() {
            1
        } else if self.identity {
            2
        } else {
            self.cmap
                .as_ref()
                .and_then(|c| (1..=4u8).find(|n| c.lookup_cid_code(code, *n).is_some()))
                .unwrap_or(2) as usize
        };
        for i in (0..len).rev() {
            out.push((code >> (8 * i)) as u8);
        }
    }

    /// The codes each character can be written with: those of glyphs the
    /// font holds. A subset font holds the glyphs `used` on the page and
    /// those its character set lists, no others for certain.
    pub fn encoder(&self, used: &HashSet<u32>) -> HashMap<char, u32> {
        let mut out = HashMap::new();
        let mut add = |code: u32, font: &Font| {
            if let Some(text) = font.unicode(code) {
                let mut chars = text.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    out.entry(c).or_insert(code);
                }
            }
        };
        let mut used_sorted: Vec<u32> = used.iter().copied().collect();
        used_sorted.sort_unstable();
        for code in &used_sorted {
            add(*code, self);
        }
        if self.simple.is_some() {
            for code in 0..256u32 {
                if used.contains(&code) {
                    continue;
                }
                let held = if self.subset {
                    self.glyph_name(code)
                        .is_some_and(|n| self.glyphs.as_ref().is_some_and(|g| g.contains(n)))
                } else {
                    self.standard.is_some() || self.widths.get(&code).is_some_and(|w| *w > 0.0)
                };
                if held {
                    add(code, self);
                }
            }
        } else if !self.subset && self.to_unicode.is_some() && self.identity {
            for code in 0..=0xFFFFu32 {
                add(code, self);
            }
        }
        out
    }
}

/// The text a glyph name stands for (Adobe's glyph list, and `uniXXXX`
/// or `uXXXX` names).
fn glyph_unicode(name: &str) -> Option<String> {
    if let Some(s) = glyph_names::get(name) {
        return Some(s.to_string());
    }
    let base = name.split('.').next().unwrap_or(name);
    if let Some(s) = glyph_names::get(base) {
        return Some(s.to_string());
    }
    let hex = base
        .strip_prefix("uni")
        .filter(|h| h.len() == 4)
        .or_else(|| {
            base.strip_prefix('u')
                .filter(|h| (4..=6).contains(&h.len()))
        })?;
    let c = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)?;
    Some(c.to_string())
}

/// The standard font `name` is, or stands in for (Arial for Helvetica,
/// and so on), with its widths.
pub(crate) fn standard_font(
    name: &str,
) -> Option<(&'static str, &'static phf::Map<&'static str, f32>)> {
    let lower = name.to_ascii_lowercase().replace([' ', '-', ','], "");
    let bold = lower.contains("bold") || lower.contains("black") || lower.contains("heavy");
    let italic = lower.contains("italic") || lower.contains("oblique");
    let family = if lower.starts_with("courier") {
        "Courier"
    } else if lower.starts_with("times") {
        "Times"
    } else if lower.starts_with("helvetica") || lower.starts_with("arial") {
        "Helvetica"
    } else if lower.starts_with("symbol") {
        return Some(("Symbol", &metrics::SYMBOL));
    } else if lower.starts_with("zapfdingbats") {
        return Some(("ZapfDingbats", &metrics::ZAPF_DING_BATS));
    } else {
        return None;
    };
    Some(style(family, bold, italic))
}

/// The standard font of `family` in a style, and its widths.
fn style(
    family: &str,
    bold: bool,
    italic: bool,
) -> (&'static str, &'static phf::Map<&'static str, f32>) {
    match (family, bold, italic) {
        ("Courier", false, false) => ("Courier", &metrics::COURIER),
        ("Courier", true, false) => ("Courier-Bold", &metrics::COURIER_BOLD),
        ("Courier", false, true) => ("Courier-Oblique", &metrics::COURIER_OBLIQUE),
        ("Courier", true, true) => ("Courier-BoldOblique", &metrics::COURIER_BOLD_OBLIQUE),
        ("Times", false, false) => ("Times-Roman", &metrics::TIMES_ROMAN),
        ("Times", true, false) => ("Times-Bold", &metrics::TIMES_BOLD),
        ("Times", false, true) => ("Times-Italic", &metrics::TIMES_ITALIC),
        ("Times", true, true) => ("Times-BoldItalic", &metrics::TIMES_BOLD_ITALIC),
        (_, false, false) => ("Helvetica", &metrics::HELVETICA),
        (_, true, false) => ("Helvetica-Bold", &metrics::HELVETICA_BOLD),
        (_, false, true) => ("Helvetica-Oblique", &metrics::HELVETICA_OBLIQUE),
        (_, true, true) => ("Helvetica-BoldOblique", &metrics::HELVETICA_BOLD_OBLIQUE),
    }
}

/// The standard font, with WinAnsi encoding, that best stands in for
/// `name` where its own glyphs run out: serif, fixed width, bold and
/// italic as the name says.
pub(crate) fn stand_in(name: &str) -> (&'static str, &'static phf::Map<&'static str, f32>) {
    if let Some(found) = standard_font(name).filter(|(n, _)| !n.starts_with(['S', 'Z'])) {
        return found;
    }
    let lower = name.to_ascii_lowercase();
    let bold = ["bold", "black", "heavy", "semibold", "demi"]
        .iter()
        .any(|w| lower.contains(w));
    let italic = lower.contains("italic") || lower.contains("oblique");
    let family = if ["mono", "courier", "code", "consol", "typewriter"]
        .iter()
        .any(|w| lower.contains(w))
    {
        "Courier"
    } else if [
        "sans",
        "arial",
        "helvet",
        "verdana",
        "tahoma",
        "calibri",
        "segoe",
        "dejavu",
        "liberationsans",
        "noto sans",
        "roboto",
        "open",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        "Helvetica"
    } else if [
        "serif", "times", "roman", "georgia", "cambria", "garamond", "book", "minion", "cmr",
        "lmroman", "palatino",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        "Times"
    } else {
        "Helvetica"
    };
    style(family, bold, italic)
}

/// `c` in WinAnsi encoding, with its glyph name.
pub(crate) fn win_ansi_code(c: char) -> Option<(u8, &'static str)> {
    let name = glyph_names::get_reverse(c)?;
    (0..=255u8).find_map(|code| (win_ansi::get(code) == Some(name)).then_some((code, name)))
}

/// The width of WinAnsi code `code` in a standard font, in glyph space.
pub(crate) fn win_ansi_width(metrics: &phf::Map<&'static str, f32>, code: u8) -> f64 {
    win_ansi::get(code)
        .and_then(|name| metrics.get(name))
        .map_or(0.0, |w| f64::from(*w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_names_to_text() {
        assert_eq!(glyph_unicode("A").as_deref(), Some("A"));
        assert_eq!(glyph_unicode("uni0915").as_deref(), Some("\u{915}"));
        assert_eq!(glyph_unicode("u1F600").as_deref(), Some("\u{1F600}"));
        assert_eq!(glyph_unicode("fi").as_deref(), Some("\u{FB01}"));
        assert_eq!(glyph_unicode("a.sc").as_deref(), Some("a"));
        assert_eq!(glyph_unicode("g123"), None);
    }

    #[test]
    fn stand_ins_follow_the_name() {
        assert_eq!(stand_in("ABCDEF+Arial-BoldMT").0, "Helvetica-Bold");
        assert_eq!(stand_in("TimesNewRomanPS-ItalicMT").0, "Times-Italic");
        assert_eq!(stand_in("CMR10").0, "Times-Roman");
        assert_eq!(stand_in("DejaVuSansMono").0, "Courier");
        assert_eq!(stand_in("Whatever").0, "Helvetica");
        assert_eq!(win_ansi_code('é'), Some((0xE9, "eacute")));
        assert_eq!(win_ansi_code('€'), Some((0x80, "Euro")));
        assert_eq!(win_ansi_code('क'), None);
    }
}
