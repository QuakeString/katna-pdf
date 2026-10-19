// SPDX-License-Identifier: GPL-3.0-or-later

//! True redaction (docs/PLAN.md §4.3, "redaction"): what lies under the
//! areas marked is taken out of the file, not only covered. Glyphs under
//! an area leave the text (their place kept, so the text beside them
//! does not move), invisible text such as a scan's recognised words
//! included; pictures under one have those pixels blacked out in the
//! picture's own data, or are taken out whole when their data cannot be
//! read; shapes wholly under one are taken out; forms a page places are
//! redacted inside, as a copy; annotations and form fields over one are
//! removed. A filled box is drawn over each area. The file is then
//! written again whole, keeping only the objects it still uses, so no
//! older revision or dropped object still holds what was taken out.
//!
//! Areas can be marked by hand, or found: words, PAN and Aadhaar
//! numbers, phone numbers and email addresses ([`Pattern`]).

use std::collections::{BTreeMap, HashSet};

use regex::Regex;

use crate::content::{Op, ops, splice};
use crate::document::Document;
use crate::edit::{Area, Matrix};
use crate::filter::decode;
use crate::lock::{LockError, NewLock};
use crate::object::{Dict, Object, Ref, Stream};
use crate::pages::page_dict;
use crate::parse::{Parser, is_white};

/// Deepest forms within forms redacted inside; deeper ones are taken
/// out whole.
const MAX_DEPTH: usize = 8;
/// Pictures bigger than this many samples are taken out whole rather
/// than read.
const MOST_SAMPLES: u64 = 400_000_000;

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// How a redaction is done.
#[derive(Debug, Clone, PartialEq)]
pub struct RedactOptions {
    /// The colour of the boxes drawn over the areas, or none.
    pub fill: Option<[f32; 3]>,
    /// Also take out what the document says about itself (properties and
    /// XMP), its scripts, its attached files and other programs' private
    /// data.
    pub scrub: bool,
}

impl Default for RedactOptions {
    fn default() -> Self {
        RedactOptions {
            fill: Some([0.0; 3]),
            scrub: false,
        }
    }
}

/// Why a file could not be redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactError {
    NoPage(usize),
    Write(LockError),
}

impl std::fmt::Display for RedactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RedactError::NoPage(p) => write!(f, "there is no page {}", p + 1),
            RedactError::Write(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RedactError {}

/// Text to find and redact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// These words, in any case.
    Words(String),
    /// India's Permanent Account Numbers: `ABCDE1234F`.
    Pan,
    /// Aadhaar numbers: twelve digits, in fours or not.
    Aadhaar,
    /// Phone numbers: India's mobiles, and numbers written with a `+`.
    Phone,
    Email,
}

impl Pattern {
    fn regex(&self) -> Option<Regex> {
        let source = match self {
            Pattern::Words(w) if w.trim().is_empty() => return None,
            Pattern::Words(w) => format!("(?i){}", regex::escape(w.trim())),
            Pattern::Pan => r"\b[A-Z]{5}[0-9]{4}[A-Z]\b".into(),
            Pattern::Aadhaar => r"\b[2-9][0-9]{3}[ -]?[0-9]{4}[ -]?[0-9]{4}\b".into(),
            Pattern::Phone => {
                r"(?:\+[0-9]{1,3}[ -]?)?\(?\b[0-9]{2,5}\)?[ -]?[0-9]{3,5}[ -]?[0-9]{3,5}\b|\b[6-9][0-9]{4}[ -]?[0-9]{5}\b".into()
            }
            Pattern::Email => r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}".into(),
        };
        Regex::new(&source).ok()
    }

    /// Whether a match of a phone pattern is one: numbers without a `+`
    /// must be India's ten-digit mobiles, so dates and amounts are not
    /// taken for them.
    fn accepts(&self, found: &str) -> bool {
        match self {
            Pattern::Phone => {
                let digits: String = found.chars().filter(char::is_ascii_digit).collect();
                if found.trim_start().starts_with('+') {
                    (8..=15).contains(&digits.len())
                } else {
                    digits.len() == 10 && digits.starts_with(['6', '7', '8', '9'])
                }
            }
            _ => true,
        }
    }
}

/// Changes to the file's objects, made in memory before it is written
/// again whole.
struct Changes<'a> {
    doc: &'a Document,
    next: u32,
    objects: BTreeMap<u32, (u16, Object)>,
}

impl Changes<'_> {
    fn add(&mut self, obj: Object) -> Ref {
        let r = Ref::new(self.next, 0);
        self.next += 1;
        self.objects.insert(r.num, (0, obj));
        r
    }

    fn replace(&mut self, r: Ref, obj: Object) {
        self.objects.insert(r.num, (r.generation, obj));
    }

    fn get(&self, r: Ref) -> Option<Object> {
        match self.objects.get(&r.num) {
            Some((_, o)) => Some(o.clone()),
            None => self.doc.get(r),
        }
    }

    fn resolve(&self, obj: &Object) -> Object {
        match obj {
            Object::Ref(r) => self.get(*r).unwrap_or(Object::Null),
            other => other.clone(),
        }
    }
}

impl Document {
    /// Where on page `page` text matching `patterns` is, a box for each
    /// match in the page's own space (left, bottom, right, top), for
    /// [`Document::redact`]. Invisible text, such as a scan's recognised
    /// words, is searched too.
    pub fn find_redactions(&self, page: usize, patterns: &[Pattern]) -> Vec<[f32; 4]> {
        let finders: Vec<(&Pattern, Regex)> = patterns
            .iter()
            .filter_map(|p| p.regex().map(|r| (p, r)))
            .collect();
        let mut out = Vec::new();
        for (text, boxes) in self.line_boxes(page) {
            // Byte offsets to char offsets.
            let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
            let char_at = |byte: usize| starts.partition_point(|&s| s < byte);
            for (pattern, regex) in &finders {
                for m in regex.find_iter(&text) {
                    if !pattern.accepts(m.as_str()) {
                        continue;
                    }
                    let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                    for b in &boxes[char_at(m.start())..char_at(m.end()).min(boxes.len())] {
                        if b[0] > b[2] {
                            continue;
                        }
                        r = [
                            r[0].min(b[0]),
                            r[1].min(b[1]),
                            r[2].max(b[2]),
                            r[3].max(b[3]),
                        ];
                    }
                    if r[0] <= r[2] {
                        out.push(r.map(|v| v as f32));
                    }
                }
            }
        }
        out
    }

    /// The file with everything under `areas` taken out (each a page and
    /// a box in its own space: left, bottom, right, top), written again
    /// whole. A locked file stays locked with the same password.
    pub fn redact(
        &self,
        areas: &[(usize, [f32; 4])],
        options: &RedactOptions,
    ) -> Result<Vec<u8>, RedactError> {
        if self.is_locked() {
            return Err(RedactError::Write(LockError::Locked));
        }
        let pages = self.pages();
        let mut by_page: BTreeMap<usize, Vec<Area>> = BTreeMap::new();
        for (page, r) in areas {
            if *page >= pages.len() {
                return Err(RedactError::NoPage(*page));
            }
            let r = r.map(f64::from);
            let area = [
                r[0].min(r[2]),
                r[1].min(r[3]),
                r[0].max(r[2]),
                r[1].max(r[3]),
            ];
            if area[2] - area[0] > 0.0 && area[3] - area[1] > 0.0 {
                by_page.entry(*page).or_default().push(area);
            }
        }
        let mut changes = Changes {
            doc: self,
            next: self.size(),
            objects: BTreeMap::new(),
        };
        for (page, areas) in &by_page {
            self.redact_page(&mut changes, pages[*page], areas, options.fill);
        }
        if options.scrub {
            scrub(&mut changes, &pages);
        }
        self.write_whole(NewLock::Keep, true, &changes.objects)
            .map_err(RedactError::Write)
    }

    fn redact_page(
        &self,
        changes: &mut Changes,
        page_ref: Ref,
        areas: &[Area],
        fill: Option<[f32; 3]>,
    ) {
        let inherited = page_dict(self, page_ref);
        let mut dict = changes
            .get(page_ref)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        let resources = match inherited.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let mut content = Vec::new();
        let streams = match inherited.get("Contents").map(|c| self.resolve(c)) {
            Some(Object::Array(a)) => a.iter().map(|s| self.resolve(s)).collect(),
            Some(s @ Object::Stream(_)) => vec![s],
            _ => Vec::new(),
        };
        for s in streams {
            if let Object::Stream(s) = s {
                // Content that cannot be read is not kept: it might hold
                // what is under the areas.
                if let Ok(data) = decode(&s.dict, &s.data) {
                    content.extend(data);
                    content.push(b'\n');
                }
            }
        }
        // The content, and whatever it leaves open (a text object, saved
        // states with a clip) closed, so what follows is drawn as it is.
        let mut whole = b"q\n".to_vec();
        let (mut depth, mut in_text) = (0usize, false);
        for op in ops(&content) {
            match op.operator.as_slice() {
                b"q" => depth += 1,
                b"Q" => depth = depth.saturating_sub(1),
                b"BT" => in_text = true,
                b"ET" => in_text = false,
                _ => {}
            }
        }
        whole.extend(content);
        if in_text {
            whole.extend_from_slice(b"\nET");
        }
        whole.extend_from_slice(b"\n");
        whole.extend(std::iter::repeat_n(b"Q ".as_slice(), depth).flatten());
        whole.extend_from_slice(b"Q\n");
        // Annotations over the areas go; what they showed is drawn into
        // the page instead, redacted like the rest.
        let mut resources = resources;
        let shown = self.redact_annotations(changes, &mut dict, areas);
        if !shown.is_empty() {
            let mut xobjects = match self.resolve_key(&resources, "XObject") {
                Object::Dict(d) => d,
                _ => Dict::new(),
            };
            for (appearance, place) in shown {
                let key = free_name(&xobjects);
                xobjects.set(&key, appearance);
                whole.extend_from_slice(
                    format!(
                        "q {} {} {} {} {} {} cm /{key} Do Q\n",
                        num(place[0]),
                        num(place[1]),
                        num(place[2]),
                        num(place[3]),
                        num(place[4]),
                        num(place[5])
                    )
                    .as_bytes(),
                );
            }
            resources.set("XObject", Object::Dict(xobjects));
        }
        let (mut out, resources) =
            self.redact_content(changes, whole, resources, IDENTITY, areas, 0);
        if let Some([r, g, b]) = fill {
            out.extend_from_slice(
                format!(
                    "q {} {} {} rg\n",
                    num(r.into()),
                    num(g.into()),
                    num(b.into())
                )
                .as_bytes(),
            );
            for a in areas {
                out.extend_from_slice(
                    format!(
                        "{} {} {} {} re\n",
                        num(a[0]),
                        num(a[1]),
                        num(a[2] - a[0]),
                        num(a[3] - a[1])
                    )
                    .as_bytes(),
                );
            }
            out.extend_from_slice(b"f Q\n");
        }
        let stream = changes.add(Object::Stream(crate::dss::compressed(&out)));
        dict.set("Contents", Object::Ref(stream));
        dict.set("Resources", Object::Dict(resources));
        // A small picture of the page as it was, and other programs'
        // copies of what it held.
        dict.remove("Thumb");
        dict.remove("PieceInfo");
        changes.replace(page_ref, Object::Dict(dict));
    }

    /// `content`, drawn with `resources` from matrix `ctm`, with what is
    /// under `areas` taken out; and its resources, with the copies of
    /// pictures and forms it now draws.
    fn redact_content(
        &self,
        changes: &mut Changes,
        content: Vec<u8>,
        mut resources: Dict,
        ctm: Matrix,
        areas: &[Area],
        depth: usize,
    ) -> (Vec<u8>, Dict) {
        let read = ops(&content);
        let mut edits: BTreeMap<usize, Vec<u8>> = self
            .cut_text(content.clone(), &resources, ctm, areas)
            .into_iter()
            .collect();
        let text_cut: HashSet<usize> = edits.keys().copied().collect();
        let mut xobjects = match self.resolve_key(&resources, "XObject") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        let mut xobjects_changed = false;
        // Pictures and forms no longer drawn as they were.
        let mut replaced: Vec<Vec<u8>> = Vec::new();
        // Spans of operators taken out together: a path and its paint.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut m = ctm;
        let mut stack = Vec::new();
        let mut path: Option<(usize, Vec<(f64, f64)>)> = None;
        let mut clipping = false;
        // Marked content with text alternatives: where it starts.
        let mut marked: Vec<Option<usize>> = Vec::new();
        let mut cut_since: Vec<bool> = Vec::new();
        for (i, op) in read.iter().enumerate() {
            let n = |k: usize| op.number(k).unwrap_or(0.0);
            let mut point = |x: f64, y: f64| {
                let (_, points) = path.get_or_insert((i, Vec::new()));
                points.push(apply(&m, x, y));
            };
            match op.operator.as_slice() {
                b"q" => stack.push(m),
                b"Q" => m = stack.pop().unwrap_or(m),
                b"cm" if op.operands.len() == 6 => {
                    m = mul(&[n(0), n(1), n(2), n(3), n(4), n(5)], &m);
                }
                b"m" | b"l" => point(n(0), n(1)),
                b"c" => {
                    point(n(0), n(1));
                    point(n(2), n(3));
                    point(n(4), n(5));
                }
                b"v" | b"y" => {
                    point(n(0), n(1));
                    point(n(2), n(3));
                }
                b"re" => {
                    let (x, y, w, h) = (n(0), n(1), n(2), n(3));
                    point(x, y);
                    point(x + w, y);
                    point(x, y + h);
                    point(x + w, y + h);
                }
                b"W" | b"W*" => clipping = true,
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                    if let Some((start, points)) = path.take()
                        && !clipping
                        && !op.is("n")
                        && within(&points, areas)
                    {
                        spans.push((start, i));
                    }
                    clipping = false;
                }
                b"Do" => {
                    let Some(name) = op.operands.first().and_then(Object::as_name) else {
                        continue;
                    };
                    let Some(value) = entry(&xobjects, name).cloned() else {
                        continue;
                    };
                    let Object::Stream(s) = self.resolve(&value) else {
                        continue;
                    };
                    let new = match s.dict.name("Subtype") {
                        Some(b"Image") => {
                            if !touches(&unit_box(&m), areas) {
                                continue;
                            }
                            self.redact_image(changes, &s, &m, areas, Some(&resources))
                                .map(Object::Stream)
                        }
                        Some(b"Form") => {
                            let form_m = matrix_of(self, &s.dict);
                            let inner = mul(&form_m, &m);
                            let bbox = bbox_of(self, &s.dict);
                            if !bbox.is_none_or(|b| touches(&box_through(&b, &inner), areas)) {
                                continue;
                            }
                            if depth >= MAX_DEPTH {
                                None
                            } else {
                                self.redact_form(changes, &s, &resources, inner, areas, depth)
                                    .map(Object::Stream)
                            }
                        }
                        _ => continue,
                    };
                    replaced.push(name.to_vec());
                    match new {
                        Some(obj) => {
                            let r = changes.add(obj);
                            let key = free_name(&xobjects);
                            xobjects.set(&key, Object::Ref(r));
                            xobjects_changed = true;
                            edits.insert(i, format!("/{key} Do").into_bytes());
                        }
                        None => {
                            edits.insert(i, Vec::new());
                        }
                    }
                }
                b"BI" => {
                    if !touches(&unit_box(&m), areas) {
                        continue;
                    }
                    let new = self
                        .redact_inline(changes, &content[op.span.clone()], &m, areas, &resources)
                        .unwrap_or_default();
                    edits.insert(i, new);
                }
                b"BDC" | b"BMC" => {
                    let alt = matches!(op.operands.last(), Some(Object::Dict(d))
                        if ["ActualText", "Alt", "E"].iter().any(|k| d.get(k).is_some()));
                    marked.push(alt.then_some(i));
                    cut_since.push(false);
                }
                b"EMC" => {
                    if let (Some(start), Some(cut)) = (marked.pop(), cut_since.pop())
                        && let Some(start) = start
                        && cut
                    {
                        // Its words said again for screen readers.
                        let open = &read[start];
                        let mut operands = open.operands.clone();
                        if let Some(Object::Dict(d)) = operands.last_mut() {
                            d.remove("ActualText");
                            d.remove("Alt");
                            d.remove("E");
                        }
                        let mut bytes = Vec::new();
                        crate::content::write_op(&mut bytes, &Op::new("BDC", operands));
                        edits.insert(start, bytes);
                    }
                }
                _ => {}
            }
            if text_cut.contains(&i) {
                // Marked content it is nested in has lost words too.
                cut_since.iter_mut().for_each(|c| *c = true);
            }
        }
        let mut splices: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
        let mut in_span = HashSet::new();
        for (start, end) in spans {
            splices.push((read[start].span.start..read[end].span.end, Vec::new()));
            in_span.extend(start..=end);
        }
        for (i, bytes) in edits {
            if !in_span.contains(&i) {
                splices.push((read[i].span.clone(), bytes));
            }
        }
        let content = splice(&content, splices);
        if !replaced.is_empty() {
            // Those not drawn any more leave the resources, so the file
            // keeps no copy of them.
            let used: HashSet<Vec<u8>> = ops(&content)
                .iter()
                .filter(|op| op.is("Do"))
                .filter_map(|op| op.operands.first()?.as_name().map(<[u8]>::to_vec))
                .collect();
            xobjects.0.retain(|(k, _)| {
                !replaced.iter().any(|r| r == k.as_bytes()) || used.contains(k.as_bytes())
            });
            xobjects_changed = true;
        }
        if xobjects_changed {
            resources.set("XObject", Object::Dict(xobjects));
        }
        (content, resources)
    }

    /// A copy of form `form`, drawn from matrix `ctm`, redacted inside.
    fn redact_form(
        &self,
        changes: &mut Changes,
        form: &Stream,
        outer: &Dict,
        ctm: Matrix,
        areas: &[Area],
        depth: usize,
    ) -> Option<Stream> {
        let content = decode(&form.dict, &form.data).ok()?;
        let resources = match self.resolve_key(&form.dict, "Resources") {
            Object::Dict(d) => d,
            _ => outer.clone(),
        };
        let (content, resources) =
            self.redact_content(changes, content, resources, ctm, areas, depth + 1);
        let mut new = crate::dss::compressed(&content);
        let mut dict = form.dict.clone();
        dict.remove("DecodeParms");
        dict.remove("Length");
        dict.remove("PieceInfo");
        dict.remove("Filter");
        for (k, v) in new.dict.iter() {
            dict.set(&String::from_utf8_lossy(k.as_bytes()), v.clone());
        }
        dict.set("Resources", Object::Dict(resources));
        new.dict = dict;
        Some(new)
    }

    /// A copy of picture `image`, drawn from matrix `ctm`, with the
    /// pixels under `areas` blacked out; none when its data cannot be
    /// read and written again.
    fn redact_image(
        &self,
        changes: &mut Changes,
        image: &Stream,
        ctm: &Matrix,
        areas: &[Area],
        resources: Option<&Dict>,
    ) -> Option<Stream> {
        let d = &image.dict;
        let width = self.resolve_key(d, "Width").as_int()?;
        let height = self.resolve_key(d, "Height").as_int()?;
        if width <= 0 || height <= 0 {
            return None;
        }
        let (width, height) = (width as usize, height as usize);
        let mask = matches!(self.resolve_key(d, "ImageMask"), Object::Bool(true));
        let filters: Vec<Vec<u8>> = match self.resolve_key(d, "Filter") {
            Object::Name(n) => vec![n.as_bytes().to_vec()],
            Object::Array(a) => a
                .iter()
                .filter_map(|f| self.resolve(f).as_name().map(<[u8]>::to_vec))
                .collect(),
            _ => Vec::new(),
        };
        let picture = filters.last().map(Vec::as_slice);
        let mut set: Vec<(&str, Object)> = Vec::new();
        let (mut data, components, bits) = match picture {
            Some(b"DCTDecode" | b"JBIG2Decode" | b"CCITTFaxDecode" | b"JPXDecode") => {
                // Read by the pictures' own decoders, which take their
                // data as it is once any filters before theirs are undone.
                let last = filters.len() - 1;
                let all_params: Vec<Object> = match self.resolve_key(d, "DecodeParms") {
                    Object::Array(a) => a.iter().map(|p| self.resolve(p)).collect(),
                    Object::Null => Vec::new(),
                    one => vec![one],
                };
                let params = match all_params.get(last) {
                    Some(Object::Dict(p)) if all_params.len() == filters.len() => p.clone(),
                    Some(Object::Dict(p)) if last == 0 => p.clone(),
                    _ => Dict::new(),
                };
                let raw = if last == 0 {
                    image.data.clone()
                } else {
                    let mut before = Dict::new();
                    before.set(
                        "Filter",
                        Object::Array(
                            filters[..last]
                                .iter()
                                .map(|f| Object::name(&String::from_utf8_lossy(f)))
                                .collect(),
                        ),
                    );
                    if all_params.len() == filters.len() {
                        before.set("DecodeParms", Object::Array(all_params[..last].to_vec()));
                    }
                    decode(&before, &image.data).ok()?
                };
                let image = Stream {
                    dict: Dict::new(),
                    data: raw,
                };
                let (data, components, bits) = match picture {
                    Some(b"DCTDecode") => {
                        let (data, components) = jpeg(&image.data)?;
                        (data, components, 8)
                    }
                    Some(b"JBIG2Decode") => {
                        let globals = match self.resolve_key(&params, "JBIG2Globals") {
                            Object::Stream(g) => decode(&g.dict, &g.data).ok(),
                            _ => None,
                        };
                        (jbig2(&image.data, globals.as_deref())?, 1, 1)
                    }
                    Some(b"CCITTFaxDecode") => (ccitt(self, &image.data, &params, height)?, 1, 1),
                    _ => {
                        let own = self.resolve_key(d, "ColorSpace");
                        let (samples, channels, alpha) = jpx(&image.data, own == Object::Null)?;
                        let total = channels + usize::from(alpha);
                        let wanted = if own == Object::Null {
                            let cs = match channels {
                                1 => "DeviceGray",
                                3 => "DeviceRGB",
                                4 => "DeviceCMYK",
                                _ => return None,
                            };
                            set.push(("ColorSpace", Object::name(cs)));
                            channels
                        } else {
                            components(self, &own, resources)?
                        };
                        // What the picture calls alpha may be one of the
                        // colour space's own components.
                        let (data, channels, alpha) = if wanted == total {
                            (samples, total, None)
                        } else if wanted == channels && alpha {
                            let pixels = samples.chunks_exact(total);
                            let opacity: Vec<u8> = pixels.clone().map(|p| p[channels]).collect();
                            let colour =
                                pixels.flat_map(|p| p[..channels].iter().copied()).collect();
                            (colour, channels, Some(opacity))
                        } else if wanted == channels {
                            (samples, channels, None)
                        } else {
                            return None;
                        };
                        // Its own transparency becomes a soft mask.
                        if let Some(mut alpha) = alpha
                            && matches!(self.resolve_key(d, "SMaskInData"), Object::Int(1 | 2))
                        {
                            black_out(&mut alpha, (width, height), 8, ctm, areas)?;
                            let mut mask = crate::dss::compressed(&alpha);
                            mask.dict.set("Type", Object::name("XObject"));
                            mask.dict.set("Subtype", Object::name("Image"));
                            mask.dict.set("Width", Object::Int(width as i64));
                            mask.dict.set("Height", Object::Int(height as i64));
                            mask.dict.set("ColorSpace", Object::name("DeviceGray"));
                            mask.dict.set("BitsPerComponent", Object::Int(8));
                            let r = changes.add(Object::Stream(mask));
                            set.push(("SMask", Object::Ref(r)));
                        }
                        (data, channels, 8)
                    }
                };
                set.push(("BitsPerComponent", Object::Int(bits as i64)));
                (data, components, bits)
            }
            _ => {
                let components = if mask {
                    1
                } else {
                    components(self, &self.resolve_key(d, "ColorSpace"), resources)?
                };
                let bits = if mask {
                    1
                } else {
                    self.resolve_key(d, "BitsPerComponent").as_int()? as usize
                };
                if !matches!(bits, 1 | 2 | 4 | 8 | 16) {
                    return None;
                }
                (decode(d, &image.data).ok()?, components, bits)
            }
        };
        if (width as u64) * (height as u64) * (components as u64) > MOST_SAMPLES {
            return None;
        }
        let stride = black_out(&mut data, (width, height), components * bits, ctm, areas)?;
        let mut new = crate::dss::compressed(&data[..stride * height]);
        let mut dict = d.clone();
        for key in [
            "Filter",
            "DecodeParms",
            "Length",
            "Alternates",
            "OPI",
            "Metadata",
        ] {
            dict.remove(key);
        }
        for (key, value) in set {
            dict.set(key, value);
        }
        if picture == Some(b"JPXDecode") {
            dict.remove("SMaskInData");
        }
        // Its soft mask can hold the same shapes.
        for key in ["SMask", "Mask"] {
            if let Object::Stream(s) = self.resolve_key(d, key) {
                match self.redact_image(changes, &s, ctm, areas, resources) {
                    Some(m) => {
                        let r = changes.add(Object::Stream(m));
                        dict.set(key, Object::Ref(r));
                    }
                    None => {
                        dict.remove(key);
                    }
                }
            }
        }
        for (k, v) in new.dict.iter() {
            dict.set(&String::from_utf8_lossy(k.as_bytes()), v.clone());
        }
        new.dict = dict;
        Some(new)
    }

    /// Inline picture `bytes` (`BI` … `EI`) redacted, written again
    /// with its data in hex; none when it cannot be read.
    fn redact_inline(
        &self,
        changes: &mut Changes,
        bytes: &[u8],
        ctm: &Matrix,
        areas: &[Area],
        resources: &Dict,
    ) -> Option<Vec<u8>> {
        let mut p = Parser::new(bytes, 2);
        let mut dict = Dict::new();
        loop {
            p.skip_white();
            if p.pos >= bytes.len() {
                return None;
            }
            let at = p.pos;
            let Some(key) = p.object(8) else {
                if p.token() == b"ID" {
                    break;
                }
                if p.pos == at {
                    return None;
                }
                continue;
            };
            let value = p.object(8)?;
            let Object::Name(key) = key else {
                return None;
            };
            let key = full_key(key.as_bytes());
            dict.set(key, full_value(key, value));
        }
        let start = p.pos + 1;
        let mut end = bytes.len().checked_sub(2)?;
        while end > start && is_white(bytes[end - 1]) {
            end -= 1;
        }
        let image = Stream {
            dict,
            data: bytes.get(start..end)?.to_vec(),
        };
        let new = self.redact_image(changes, &image, ctm, areas, Some(resources))?;
        let data = decode(&new.dict, &new.data).ok()?;
        let mut out = b"BI".to_vec();
        for (k, v) in image.dict.iter() {
            if matches!(k.as_bytes(), b"Filter" | b"DecodeParms" | b"Length") {
                continue;
            }
            out.push(b' ');
            crate::write::write_object(&mut out, &Object::Name(k.clone()));
            out.push(b' ');
            crate::write::write_object(&mut out, v);
        }
        out.extend_from_slice(b" /BitsPerComponent ");
        out.extend_from_slice(
            new.dict
                .int("BitsPerComponent")
                .unwrap_or(8)
                .to_string()
                .as_bytes(),
        );
        out.extend_from_slice(b" /Filter /ASCIIHexDecode ID\n");
        for b in data {
            out.extend_from_slice(format!("{b:02x}").as_bytes());
        }
        out.extend_from_slice(b">\nEI");
        Some(out)
    }

    /// Page `dict` without the annotations over `areas`; form fields
    /// whose last widget goes are taken out of the form too. Returns what
    /// those shown on the page looked like: each one's appearance, and
    /// the matrix that puts it in its place.
    fn redact_annotations(
        &self,
        changes: &mut Changes,
        dict: &mut Dict,
        areas: &[Area],
    ) -> Vec<(Object, Matrix)> {
        let annots = match dict.get("Annots").map(|a| changes.resolve(a)) {
            Some(Object::Array(a)) => a,
            _ => return Vec::new(),
        };
        let mut shown = Vec::new();
        let mut gone: HashSet<Ref> = HashSet::new();
        let mut kept = Vec::new();
        let mut widgets = Vec::new();
        for a in &annots {
            let Object::Dict(d) = changes.resolve(a) else {
                continue;
            };
            let rect: Vec<f64> = match changes.resolve(d.get("Rect").unwrap_or(&Object::Null)) {
                Object::Array(r) => r
                    .iter()
                    .filter_map(|v| changes.resolve(v).as_number())
                    .collect(),
                _ => Vec::new(),
            };
            let over = match rect[..] {
                [a, b, c, e] => touches(&[a.min(c), b.min(e), a.max(c), b.max(e)], areas),
                _ => false,
            };
            if over {
                if let Some(place) = self.appearance(changes, &d, &rect) {
                    shown.push(place);
                }
                if let Some(r) = a.as_ref() {
                    gone.insert(r);
                    if d.name("Subtype") == Some(b"Widget") {
                        widgets.push((r, d.clone()));
                    }
                }
            } else {
                kept.push((a.clone(), d));
            }
        }
        if gone.is_empty() {
            return shown;
        }
        // A note's pop-up goes with it.
        let kept: Vec<Object> = kept
            .into_iter()
            .filter(|(_, d)| {
                !d.get("Parent")
                    .and_then(Object::as_ref)
                    .is_some_and(|p| gone.contains(&p))
            })
            .map(|(a, _)| a)
            .collect();
        dict.set("Annots", Object::Array(kept));
        for (widget, d) in widgets {
            self.remove_field(changes, widget, &d);
        }
        shown
    }

    /// The appearance annotation `d` shows over `rect`, and the matrix
    /// that maps its box there (ISO 32000-2 §12.5.5); none for one
    /// hidden, a pop-up, or one with no appearance.
    fn appearance(
        &self,
        changes: &mut Changes,
        d: &Dict,
        rect: &[f64],
    ) -> Option<(Object, Matrix)> {
        let [a, b, c, e] = rect[..] else {
            return None;
        };
        let rect = [a.min(c), b.min(e), a.max(c), b.max(e)];
        let flags = self.resolve_key(d, "F").as_int().unwrap_or(0);
        if flags & (2 | 32) != 0 || d.name("Subtype") == Some(b"Popup") {
            return None;
        }
        let Object::Dict(ap) = self.resolve_key(d, "AP") else {
            return None;
        };
        let normal = ap.get("N")?;
        let stream_ref = match self.resolve(normal) {
            Object::Stream(_) => normal.clone(),
            Object::Dict(states) => {
                let state = d.name("AS")?;
                entry(&states, state)?.clone()
            }
            _ => return None,
        };
        let Object::Stream(s) = self.resolve(&stream_ref) else {
            return None;
        };
        let bbox = bbox_of(self, &s.dict)?;
        let shown = box_through(&bbox, &matrix_of(self, &s.dict));
        let (w, h) = (shown[2] - shown[0], shown[3] - shown[1]);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let (sx, sy) = ((rect[2] - rect[0]) / w, (rect[3] - rect[1]) / h);
        // A field's appearance may name fonts only the form's own
        // resources hold.
        let stream_ref = match self.form_resources() {
            Some(defaults) if d.name("Subtype") == Some(b"Widget") => {
                let mut merged = defaults;
                if let Object::Dict(own) = self.resolve_key(&s.dict, "Resources") {
                    for (k, v) in own.iter() {
                        let key = String::from_utf8_lossy(k.as_bytes()).into_owned();
                        match (self.resolve(v), self.resolve_key(&merged, &key)) {
                            (Object::Dict(mine), Object::Dict(mut theirs)) => {
                                for (k2, v2) in mine.iter() {
                                    theirs.set(&String::from_utf8_lossy(k2.as_bytes()), v2.clone());
                                }
                                merged.set(&key, Object::Dict(theirs));
                            }
                            (mine, _) => merged.set(&key, mine),
                        }
                    }
                }
                let mut copy = s.clone();
                copy.dict.set("Resources", Object::Dict(merged));
                Object::Ref(changes.add(Object::Stream(copy)))
            }
            _ => stream_ref,
        };
        Some((
            stream_ref,
            [
                sx,
                0.0,
                0.0,
                sy,
                rect[0] - shown[0] * sx,
                rect[1] - shown[1] * sy,
            ],
        ))
    }

    /// Takes widget `widget` out of its field, and the field out of the
    /// form when it has no widget left.
    fn remove_field(&self, changes: &mut Changes, widget: Ref, d: &Dict) {
        let mut child = widget;
        let mut parent = if d.get("T").is_some() {
            d.get("Parent").and_then(Object::as_ref)
        } else {
            match d.get("Parent").and_then(Object::as_ref) {
                Some(field) => {
                    if !take_kid(changes, field, widget) {
                        return;
                    }
                    child = field;
                    changes
                        .get(field)
                        .and_then(|f| f.as_dict()?.get("Parent")?.as_ref())
                }
                None => None,
            }
        };
        // Up the field tree while what is left is empty.
        for _ in 0..MAX_DEPTH {
            match parent {
                Some(p) => {
                    if !take_kid(changes, p, child) {
                        return;
                    }
                    child = p;
                    parent = changes
                        .get(p)
                        .and_then(|f| f.as_dict()?.get("Parent")?.as_ref());
                }
                None => {
                    take_from_form(changes, child);
                    return;
                }
            }
        }
    }
}

impl Document {
    /// The form's default resources (`/DR`), its fields' fonts.
    fn form_resources(&self) -> Option<Dict> {
        let catalog = self.catalog()?;
        let Object::Dict(form) = self.resolve_key(&catalog, "AcroForm") else {
            return None;
        };
        match self.resolve_key(&form, "DR") {
            Object::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Takes `kid` out of `parent`'s `/Kids`; whether `parent` has none
/// left.
fn take_kid(changes: &mut Changes, parent: Ref, kid: Ref) -> bool {
    let Some(Object::Dict(mut p)) = changes.get(parent) else {
        return false;
    };
    let kids = match p.get("Kids").map(|k| changes.resolve(k)) {
        Some(Object::Array(k)) => k,
        _ => return true,
    };
    let left: Vec<Object> = kids
        .into_iter()
        .filter(|k| k.as_ref() != Some(kid))
        .collect();
    let empty = left.is_empty();
    match p.get("Kids").and_then(Object::as_ref) {
        Some(r) => changes.replace(r, Object::Array(left)),
        None => p.set("Kids", Object::Array(left)),
    }
    changes.replace(parent, Object::Dict(p));
    empty
}

/// Takes field `field` out of the form's `/Fields`.
fn take_from_form(changes: &mut Changes, field: Ref) {
    let Some(root) = changes.doc.trailer().get("Root").and_then(Object::as_ref) else {
        return;
    };
    let Some(Object::Dict(catalog)) = changes.get(root) else {
        return;
    };
    let form_ref = catalog.get("AcroForm").and_then(Object::as_ref);
    let Object::Dict(mut form) = changes.resolve(catalog.get("AcroForm").unwrap_or(&Object::Null))
    else {
        return;
    };
    let Object::Array(fields) = changes.resolve(form.get("Fields").unwrap_or(&Object::Null)) else {
        return;
    };
    let left: Vec<Object> = fields
        .into_iter()
        .filter(|f| f.as_ref() != Some(field))
        .collect();
    match form.get("Fields").and_then(Object::as_ref) {
        Some(r) => changes.replace(r, Object::Array(left)),
        None => form.set("Fields", Object::Array(left)),
    }
    match form_ref {
        Some(r) => changes.replace(r, Object::Dict(form)),
        None => {
            let mut catalog = catalog;
            catalog.set("AcroForm", Object::Dict(form));
            changes.replace(root, Object::Dict(catalog));
        }
    }
}

/// Takes out what the document says about itself, its scripts, its
/// attached files and other programs' private data.
fn scrub(changes: &mut Changes, pages: &[Ref]) {
    let doc = changes.doc;
    if let Some(info) = doc.trailer().get("Info").and_then(Object::as_ref) {
        changes.replace(info, Object::Dict(Dict::new()));
    }
    if let Some(root) = doc.trailer().get("Root").and_then(Object::as_ref)
        && let Some(Object::Dict(mut catalog)) = changes.get(root)
    {
        for key in ["Metadata", "AA", "PieceInfo"] {
            catalog.remove(key);
        }
        if let Object::Dict(action) =
            changes.resolve(catalog.get("OpenAction").unwrap_or(&Object::Null))
            && action.name("S") != Some(b"GoTo")
        {
            catalog.remove("OpenAction");
        }
        if let Object::Dict(mut names) =
            changes.resolve(catalog.get("Names").unwrap_or(&Object::Null))
        {
            names.remove("JavaScript");
            names.remove("EmbeddedFiles");
            catalog.set("Names", Object::Dict(names));
        }
        changes.replace(root, Object::Dict(catalog));
    }
    for page in pages {
        if let Some(Object::Dict(mut d)) = changes.get(*page) {
            for key in ["AA", "PieceInfo", "Metadata", "Thumb"] {
                d.remove(key);
            }
            changes.replace(*page, Object::Dict(d));
        }
    }
}

/// Blacks out the pixels of `data` (`size` pixels wide and high, rows
/// padded to a byte, `per_pixel` bits each) that a picture drawn with
/// matrix `ctm` shows under `areas`; returns the bytes in a row.
fn black_out(
    data: &mut Vec<u8>,
    size: (usize, usize),
    per_pixel: usize,
    ctm: &Matrix,
    areas: &[Area],
) -> Option<usize> {
    let (width, height) = size;
    let stride = (width * per_pixel).div_ceil(8);
    if data.len() < stride * height {
        data.resize(stride * height, 0);
    }
    let back = invert(ctm)?;
    for area in areas {
        let corners = [
            apply(&back, area[0], area[1]),
            apply(&back, area[2], area[1]),
            apply(&back, area[0], area[3]),
            apply(&back, area[2], area[3]),
        ];
        let (mut u0, mut v0, mut u1, mut v1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (u, v) in corners {
            u0 = u0.min(u);
            v0 = v0.min(v);
            u1 = u1.max(u);
            v1 = v1.max(v);
        }
        let (u0, u1) = (u0.clamp(0.0, 1.0), u1.clamp(0.0, 1.0));
        let (v0, v1) = (v0.clamp(0.0, 1.0), v1.clamp(0.0, 1.0));
        if u1 <= u0 || v1 <= v0 {
            continue;
        }
        let x0 = (u0 * width as f64).floor() as usize;
        let x1 = ((u1 * width as f64).ceil() as usize).min(width);
        // Row 0 is the picture's top.
        let y0 = ((1.0 - v1) * height as f64).floor() as usize;
        let y1 = (((1.0 - v0) * height as f64).ceil() as usize).min(height);
        // A turned or slanted picture: only the pixels whose middle
        // falls in the area, grown by a pixel so none half in it is
        // left.
        let turned = ctm[1].abs() > 1e-9 || ctm[2].abs() > 1e-9;
        let pixel = (ctm[0].hypot(ctm[1]) / width as f64).max(ctm[2].hypot(ctm[3]) / height as f64);
        let grown = [
            area[0] - pixel,
            area[1] - pixel,
            area[2] + pixel,
            area[3] + pixel,
        ];
        for y in y0..y1 {
            let row = &mut data[y * stride..(y + 1) * stride];
            for x in x0..x1 {
                if turned {
                    let u = (x as f64 + 0.5) / width as f64;
                    let v = 1.0 - (y as f64 + 0.5) / height as f64;
                    let (px, py) = apply(ctm, u, v);
                    if px < grown[0] || px > grown[2] || py < grown[1] || py > grown[3] {
                        continue;
                    }
                }
                for bit in x * per_pixel..(x + 1) * per_pixel {
                    row[bit / 8] &= !(0x80u8 >> (bit % 8));
                }
            }
        }
    }
    Some(stride)
}

/// The decoded samples of JPEG `data` and how many components each
/// pixel has: one (grey) or three (colour); none for others.
pub(crate) fn jpeg(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    use zune_jpeg::JpegDecoder;
    // Pictures as wide as long strips are, not only the decoder's usual.
    let wide = DecoderOptions::default()
        .set_max_width(1 << 16)
        .set_max_height(1 << 16);
    let mut probe = JpegDecoder::new_with_options(ZCursor::new(data), wide);
    probe.decode_headers().ok()?;
    let (out, components) = match probe.info()?.components {
        1 => (ColorSpace::Luma, 1),
        3 => (ColorSpace::RGB, 3),
        _ => return None,
    };
    let options = wide.jpeg_set_out_colorspace(out);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(data), options);
    Some((decoder.decode().ok()?, components))
}

/// The pixels of JBIG2 `data`, a bit each, 1 for white, rows padded to
/// a byte.
fn jbig2(data: &[u8], globals: Option<&[u8]>) -> Option<Vec<u8>> {
    let image = hayro_jbig2::Image::new_embedded(data, globals).ok()?;
    let (width, height) = (image.width() as usize, image.height() as usize);
    if (width as u64) * (height as u64) > MOST_SAMPLES {
        return None;
    }
    let mut bits = Bits::new(width, height);
    image.decode(&mut bits).ok()?;
    Some(bits.out)
}

/// The pixels of CCITT fax `data`, as [`jbig2`] gives them.
fn ccitt(doc: &Document, data: &[u8], params: &Dict, height: usize) -> Option<Vec<u8>> {
    use hayro_ccitt::{DecodeSettings, DecoderContext, EncodingMode};
    let int = |key: &str| doc.resolve_key(params, key).as_int();
    let flag = |key: &str, default: bool| match doc.resolve_key(params, key) {
        Object::Bool(b) => b,
        _ => default,
    };
    let k = int("K").unwrap_or(0);
    let columns = int("Columns").unwrap_or(1728).max(1) as usize;
    let rows = (int("Rows").unwrap_or(0).max(0) as usize).max(height);
    if (columns as u64) * (rows as u64) > MOST_SAMPLES {
        return None;
    }
    let settings = DecodeSettings {
        columns: columns as u32,
        rows: rows as u32,
        end_of_block: flag("EndOfBlock", true),
        end_of_line: flag("EndOfLine", false),
        rows_are_byte_aligned: flag("EncodedByteAlign", false),
        encoding: if k < 0 {
            EncodingMode::Group4
        } else if k == 0 {
            EncodingMode::Group3_1D
        } else {
            EncodingMode::Group3_2D { k: k as u32 }
        },
        invert_black: flag("BlackIs1", false),
    };
    let mut bits = Bits::new(columns, rows);
    let mut context = DecoderContext::new(settings);
    let read = hayro_ccitt::decode(data, &mut bits, &mut context);
    // Rows read before a fault are kept, as readers do.
    if read.is_err() && bits.row == 0 {
        return None;
    }
    Some(bits.out)
}

/// The 8-bit samples of JPEG 2000 `data`, how many colour components
/// each pixel has, and whether an alpha follows them.
fn jpx(data: &[u8], resolve_palette: bool) -> Option<(Vec<u8>, usize, bool)> {
    let settings = hayro_jpeg2000::DecodeSettings {
        resolve_palette_indices: resolve_palette,
        strict: false,
        target_resolution: None,
    };
    let image = hayro_jpeg2000::Image::new(data, &settings).ok()?;
    let channels = image.color_space().num_channels() as usize;
    let alpha = image.has_alpha();
    let total = (channels + usize::from(alpha)) as u64;
    if (image.width() as u64) * (image.height() as u64) * total > MOST_SAMPLES {
        return None;
    }
    let mut context = hayro_jpeg2000::DecoderContext::default();
    let samples = image.decode(&mut context).ok()?.data_u8();
    Some((samples, channels, alpha))
}

/// One-bit pixels gathered from the fax and JBIG2 decoders, white 1.
struct Bits {
    out: Vec<u8>,
    stride: usize,
    row: usize,
    x: usize,
}

impl Bits {
    fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(8);
        Bits {
            // White until drawn.
            out: vec![0xFF; stride * height],
            stride,
            row: 0,
            x: 0,
        }
    }

    fn push(&mut self, white: bool, count: usize) {
        if !white {
            let start = self.row * self.stride * 8 + self.x;
            let end = (start + count).min(self.out.len() * 8);
            for bit in start..end {
                self.out[bit / 8] &= !(0x80u8 >> (bit % 8));
            }
        }
        self.x += count;
    }

    fn line(&mut self) {
        self.row += 1;
        self.x = 0;
    }
}

impl hayro_jbig2::Decoder for Bits {
    fn push_pixel(&mut self, black: bool) {
        self.push(!black, 1);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        self.push(!black, chunk_count as usize * 8);
    }

    fn next_line(&mut self) {
        self.line();
    }
}

impl hayro_ccitt::Decoder for Bits {
    fn push_pixels(&mut self, white: bool, count: u32) {
        self.push(white, count as usize);
    }

    fn next_line(&mut self) {
        self.line();
    }
}

/// How many components a colour in colour space `cs` has.
fn components(doc: &Document, cs: &Object, resources: Option<&Dict>) -> Option<usize> {
    let cs = doc.resolve(cs);
    let name = match &cs {
        Object::Name(n) => n.as_bytes().to_vec(),
        Object::Array(a) => doc.resolve(a.first()?).as_name()?.to_vec(),
        _ => return None,
    };
    Some(match name.as_slice() {
        b"DeviceGray" | b"G" | b"CalGray" | b"Indexed" | b"I" | b"Separation" => 1,
        b"DeviceRGB" | b"RGB" | b"CalRGB" | b"Lab" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => {
            let Object::Array(a) = &cs else {
                return None;
            };
            let Object::Stream(s) = doc.resolve(a.get(1)?) else {
                return None;
            };
            doc.resolve_key(&s.dict, "N").as_int()? as usize
        }
        b"DeviceN" => {
            let Object::Array(a) = &cs else {
                return None;
            };
            doc.resolve(a.get(1)?).as_array()?.len()
        }
        other => {
            // A name the page's resources give a colour space.
            let Object::Name(_) = &cs else {
                return None;
            };
            let named = match doc.resolve_key(resources?, "ColorSpace") {
                Object::Dict(d) => entry(&d, other).cloned()?,
                _ => return None,
            };
            return components(doc, &named, None);
        }
    })
}

/// An inline image's abbreviated key in full.
fn full_key(key: &[u8]) -> &'static str {
    match key {
        b"BPC" | b"BitsPerComponent" => "BitsPerComponent",
        b"CS" | b"ColorSpace" => "ColorSpace",
        b"D" | b"Decode" => "Decode",
        b"DP" | b"DecodeParms" => "DecodeParms",
        b"F" | b"Filter" => "Filter",
        b"H" | b"Height" => "Height",
        b"W" | b"Width" => "Width",
        b"IM" | b"ImageMask" => "ImageMask",
        b"I" | b"Interpolate" => "Interpolate",
        b"L" | b"Length" => "Length",
        b"Intent" => "Intent",
        _ => "Unknown",
    }
}

/// An inline image's abbreviated value in full.
fn full_value(key: &str, value: Object) -> Object {
    let full = |n: &[u8]| -> Option<&'static str> {
        Some(match n {
            b"G" => "DeviceGray",
            b"RGB" => "DeviceRGB",
            b"CMYK" => "DeviceCMYK",
            b"I" if key == "ColorSpace" => "Indexed",
            b"AHx" => "ASCIIHexDecode",
            b"A85" => "ASCII85Decode",
            b"LZW" => "LZWDecode",
            b"Fl" => "FlateDecode",
            b"RL" => "RunLengthDecode",
            b"CCF" => "CCITTFaxDecode",
            b"DCT" => "DCTDecode",
            _ => return None,
        })
    };
    match value {
        Object::Name(n) => match full(n.as_bytes()) {
            Some(f) => Object::name(f),
            None => Object::Name(n),
        },
        Object::Array(items) => Object::Array(
            items
                .into_iter()
                .map(|i| match i {
                    Object::Name(n) => match full(n.as_bytes()) {
                        Some(f) => Object::name(f),
                        None => Object::Name(n),
                    },
                    other => other,
                })
                .collect(),
        ),
        other => other,
    }
}

/// A name not yet used in `dict`.
fn free_name(dict: &Dict) -> String {
    (0..)
        .map(|k| format!("KatnaR{k}"))
        .find(|n| dict.get(n).is_none())
        .unwrap_or_default()
}

fn matrix_of(doc: &Document, dict: &Dict) -> Matrix {
    match doc.resolve_key(dict, "Matrix") {
        Object::Array(m) => {
            let m: Vec<f64> = m
                .iter()
                .filter_map(|v| doc.resolve(v).as_number())
                .collect();
            <[f64; 6]>::try_from(m).unwrap_or(IDENTITY)
        }
        _ => IDENTITY,
    }
}

fn bbox_of(doc: &Document, dict: &Dict) -> Option<Area> {
    let Object::Array(a) = doc.resolve_key(dict, "BBox") else {
        return None;
    };
    let n: Vec<f64> = a
        .iter()
        .filter_map(|v| doc.resolve(v).as_number())
        .collect();
    match n[..] {
        [a, b, c, d] => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
        _ => None,
    }
}

/// Box `b` through matrix `m`, as a box.
fn box_through(b: &Area, m: &Matrix) -> Area {
    bounds(&[
        apply(m, b[0], b[1]),
        apply(m, b[2], b[1]),
        apply(m, b[0], b[3]),
        apply(m, b[2], b[3]),
    ])
}

/// Where a picture drawn with matrix `m` lands.
fn unit_box(m: &Matrix) -> Area {
    box_through(&[0.0, 0.0, 1.0, 1.0], m)
}

fn bounds(points: &[(f64, f64)]) -> Area {
    let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for &(x, y) in points {
        r[0] = r[0].min(x);
        r[1] = r[1].min(y);
        r[2] = r[2].max(x);
        r[3] = r[3].max(y);
    }
    r
}

/// Whether box `b` overlaps one of `areas`.
fn touches(b: &Area, areas: &[Area]) -> bool {
    areas
        .iter()
        .any(|a| b[0] < a[2] && b[2] > a[0] && b[1] < a[3] && b[3] > a[1])
}

/// Whether all of `points` lie within one area.
fn within(points: &[(f64, f64)], areas: &[Area]) -> bool {
    if points.is_empty() {
        return false;
    }
    let b = bounds(points);
    let slack = 0.5;
    areas.iter().any(|a| {
        b[0] >= a[0] - slack && b[2] <= a[2] + slack && b[1] >= a[1] - slack && b[3] <= a[3] + slack
    }) || {
        // A small shape, such as a letter drawn as one, goes when mostly
        // under an area, as text does.
        (b[2] - b[0]).max(b[3] - b[1]) < 40.0 && covered(&b, areas)
    }
}

/// Whether most of box `b` is covered by `areas`.
fn covered(b: &Area, areas: &[Area]) -> bool {
    let whole = ((b[2] - b[0]) * (b[3] - b[1])).max(1e-9);
    areas.iter().any(|a| {
        let w = (b[2].min(a[2]) - b[0].max(a[0])).max(0.0);
        let h = (b[3].min(a[3]) - b[1].max(a[1])).max(0.0);
        w * h / whole >= 0.5
    })
}

fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let a = m[3] / det;
    let b = -m[1] / det;
    let c = -m[2] / det;
    let d = m[0] / det;
    Some([a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)])
}

fn num(v: f64) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// The value under name `key`, given as bytes.
fn entry<'a>(dict: &'a Dict, key: &[u8]) -> Option<&'a Object> {
    dict.iter()
        .find(|(k, _)| k.as_bytes() == key)
        .map(|(_, v)| v)
}
