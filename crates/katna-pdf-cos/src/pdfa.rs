// SPDX-License-Identifier: GPL-3.0-or-later

//! Files made PDF/A-2b (ISO 19005-2), the archival form: self-contained
//! and the same wherever they are opened. The file is checked for what
//! keeps it from that (`pdfa_issues`), and made it (`to_pdfa`): an sRGB
//! output intent and XMP saying what it is are put in, every font drawn
//! from a reader's own copy is embedded from a font file that matches
//! it (Liberation, Arimo, DejaVu or the like, found on the computer),
//! scripts, actions on events and files kept in the document are taken
//! out, annotations are marked to print, the lock is taken off, and the
//! file is written whole with only what it still uses. What could not
//! be put right is listed.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::actions::FieldAction;
use crate::document::Document;
use crate::edit::{number, truetype_descriptor};
use crate::fonts::truetype::TrueType;
use crate::fonts::{Font, stand_in, standard_as_differences};
use crate::lock::{LockError, NewLock};
use crate::object::{Dict, Name, Object, Ref, Stream};
use crate::pages::page_dict;
use crate::scripts::{ActionPlace, FieldEvent};
use crate::write::Update;

/// Something that keeps a file from being PDF/A.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// It has a password.
    Locked,
    /// No output intent says what its colours mean.
    NoOutputIntent,
    /// Its XMP does not say it is PDF/A (or there is none).
    NotMarked,
    /// A font, by name, is not embedded.
    FontNotEmbedded(String),
    /// Scripts and actions on events, this many.
    Scripts(usize),
    /// Files kept in the document, this many.
    Attachments(usize),
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::Locked => write!(f, "the file has a password"),
            Issue::NoOutputIntent => write!(f, "no output intent says what its colours mean"),
            Issue::NotMarked => write!(f, "its metadata does not say it is PDF/A"),
            Issue::FontNotEmbedded(name) => write!(f, "font {name} is not embedded"),
            Issue::Scripts(n) => write!(f, "{n} scripts or actions on events"),
            Issue::Attachments(n) => write!(f, "{n} files kept in the document"),
        }
    }
}

/// A file made PDF/A, and what was done to it.
#[derive(Debug, Clone)]
pub struct Pdfa {
    pub bytes: Vec<u8>,
    /// The fonts embedded, by the names the file used.
    pub fonts_embedded: Vec<String>,
    pub scripts_removed: usize,
    pub attachments_removed: usize,
    /// What still keeps it from being PDF/A.
    pub left: Vec<Issue>,
}

#[derive(Debug)]
pub enum PdfaError {
    /// The file is locked and was not opened with the owner's password.
    Locked,
    Write(String),
}

impl fmt::Display for PdfaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdfaError::Locked => write!(f, "the file is locked; open it with the owner's password"),
            PdfaError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PdfaError {}

impl From<LockError> for PdfaError {
    fn from(e: LockError) -> Self {
        match e {
            LockError::Locked | LockError::NotOwner => PdfaError::Locked,
            other => PdfaError::Write(other.to_string()),
        }
    }
}

/// The kind of font a file wants embedded in a standard font's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FontWant {
    pub family: FontFamily,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontFamily {
    Sans,
    Serif,
    Mono,
}

/// Where the computer keeps its fonts.
pub fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    if let Some(windir) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(windir).join("Fonts"));
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs
}

/// The font files under `dirs`, by their lower-case file stem.
pub fn font_files(dirs: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut BTreeMap<String, PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 5 {
                    walk(&path, depth + 1, out);
                }
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ttf"))
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                out.entry(stem.to_ascii_lowercase()).or_insert(path);
            }
        }
    }
    let mut out = BTreeMap::new();
    for dir in dirs {
        walk(dir, 0, &mut out);
    }
    out
}

/// A font file among `files` (from `font_files`) for `want`: one with
/// Helvetica's, Times's or Courier's widths first (Liberation, Arimo,
/// Tinos, Cousine), else any of the family.
pub fn find_font(files: &BTreeMap<String, PathBuf>, want: FontWant) -> Option<Vec<u8>> {
    let families: &[&str] = match want.family {
        FontFamily::Sans => &[
            "liberationsans",
            "arimo",
            "dejavusans",
            "notosans",
            "freesans",
            "arial",
        ],
        FontFamily::Serif => &[
            "liberationserif",
            "tinos",
            "dejavuserif",
            "notoserif",
            "freeserif",
            "times",
        ],
        FontFamily::Mono => &[
            "liberationmono",
            "cousine",
            "dejavusansmono",
            "notosansmono",
            "freemono",
            "cour",
        ],
    };
    let styles: &[&str] = match (want.bold, want.italic) {
        (false, false) => &["-regular", "", "regular"],
        (true, false) => &["-bold", "bold", "bd"],
        (false, true) => &["-italic", "-oblique", "italic", "oblique", "i"],
        (true, true) => &[
            "-bolditalic",
            "-boldoblique",
            "bolditalic",
            "boldoblique",
            "bi",
        ],
    };
    for family in families {
        for style in styles {
            if let Some(path) = files.get(&format!("{family}{style}"))
                && let Ok(data) = std::fs::read(path)
            {
                return Some(data);
            }
        }
    }
    None
}

/// `find_font` over the computer's own fonts.
pub fn system_font(want: FontWant) -> Option<Vec<u8>> {
    find_font(&font_files(&font_dirs()), want)
}

impl Document {
    /// What keeps the file from being PDF/A.
    pub fn pdfa_issues(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        if self.is_encrypted() {
            out.push(Issue::Locked);
        }
        if self.is_locked() {
            return out;
        }
        let catalog = self.catalog().unwrap_or_default();
        let intents = match self.resolve_key(&catalog, "OutputIntents") {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        if !intents.iter().any(|i| {
            self.resolve(i)
                .as_dict()
                .is_some_and(|d| d.name("S") == Some(b"GTS_PDFA1"))
        }) {
            out.push(Issue::NoOutputIntent);
        }
        let xmp = match self.resolve_key(&catalog, "Metadata") {
            Object::Stream(s) => crate::filter::decode(&s.dict, &s.data).unwrap_or_default(),
            _ => Vec::new(),
        };
        if !String::from_utf8_lossy(&xmp).contains("pdfaid:part") {
            out.push(Issue::NotMarked);
        }
        for (_, dict) in self.all_fonts() {
            if !self.font_embedded(&dict) {
                let name =
                    String::from_utf8_lossy(dict.name("BaseFont").unwrap_or_default()).into_owned();
                let issue = Issue::FontNotEmbedded(name);
                if !out.contains(&issue) {
                    out.push(issue);
                }
            }
        }
        let scripts = self
            .document_actions()
            .iter()
            .filter(|a| {
                !matches!(
                    a.place,
                    ActionPlace::Open
                        | ActionPlace::Field {
                            event: FieldEvent::Click,
                            ..
                        }
                ) || !action_allowed(&a.action)
            })
            .count();
        if scripts > 0 {
            out.push(Issue::Scripts(scripts));
        }
        let attachments = self.attachments().len();
        if attachments > 0 {
            out.push(Issue::Attachments(attachments));
        }
        out
    }

    /// The file as PDF/A-2b, as far as it can be made so; `fonts` gives
    /// a TrueType file to embed for a font the file leaves to the reader.
    pub fn to_pdfa(&self, fonts: &dyn Fn(FontWant) -> Option<Vec<u8>>) -> Result<Pdfa, PdfaError> {
        if self.is_locked() {
            return Err(PdfaError::Locked);
        }
        if self.is_encrypted() {
            let plain = Document::load(self.without_lock()?)
                .map_err(|e| PdfaError::Write(e.to_string()))?;
            return plain.to_pdfa(fonts);
        }
        let mut up = self.update().map_err(|e| PdfaError::Write(e.to_string()))?;
        let mut edits = Edits::default();
        let mut left = Vec::new();
        let mut fonts_embedded = Vec::new();

        // Fonts.
        let mut loaded: BTreeMap<FontWant, Option<TrueType>> = BTreeMap::new();
        for (r, dict) in self.all_fonts() {
            if self.font_embedded(&dict) {
                continue;
            }
            let name =
                String::from_utf8_lossy(dict.name("BaseFont").unwrap_or_default()).into_owned();
            let issue = Issue::FontNotEmbedded(name.clone());
            let Some(r) = r else {
                if !left.contains(&issue) {
                    left.push(issue);
                }
                continue;
            };
            let subtype = dict.name("Subtype").unwrap_or_default().to_vec();
            if !matches!(&subtype[..], b"Type1" | b"TrueType" | b"MMType1") {
                left.push(issue);
                continue;
            }
            let (standard, _) = stand_in(&name);
            let want = want_of(&name, standard);
            let tt = loaded
                .entry(want)
                .or_insert_with(|| fonts(want).and_then(|data| TrueType::parse(data, 0)));
            let Some(tt) = tt else {
                left.push(issue);
                continue;
            };
            match embed_simple(self, &mut up, &dict, tt) {
                Some(new) => {
                    edits.set(r, new);
                    fonts_embedded.push(name);
                }
                None => left.push(issue),
            }
        }

        // Scripts, actions on events and files kept in the document.
        let root = self.trailer().get("Root").and_then(Object::as_ref);
        let mut scripts_removed = 0;
        let mut attachments_removed = 0;
        if let Some(root) = root {
            let catalog = edits.dict(self, root);
            let mut catalog = catalog.unwrap_or_default();
            if catalog.remove("AA").is_some() {
                scripts_removed += 1;
            }
            if catalog
                .get("OpenAction")
                .is_some_and(|a| self.is_javascript(a))
            {
                catalog.remove("OpenAction");
                scripts_removed += 1;
            }
            let names_ref = catalog.get("Names").and_then(Object::as_ref);
            let mut names = match self.resolve_key(&catalog, "Names") {
                Object::Dict(d) => d,
                _ => Dict::new(),
            };
            if names.remove("JavaScript").is_some() {
                scripts_removed += 1;
            }
            if names.remove("EmbeddedFiles").is_some() {
                attachments_removed = self.attachments().len();
            }
            match names_ref {
                Some(r) => edits.set(r, names),
                None if names.is_empty() => {
                    catalog.remove("Names");
                }
                None => catalog.set("Names", names),
            }
            // Forms: every field's actions on events, and no appearances
            // left to the reader.
            let form_ref = catalog.get("AcroForm").and_then(Object::as_ref);
            if let Object::Dict(mut form) = self.resolve_key(&catalog, "AcroForm") {
                form.remove("NeedAppearances");
                let mut seen = HashSet::new();
                let fields = match self.resolve_key(&form, "Fields") {
                    Object::Array(a) => a,
                    _ => Vec::new(),
                };
                for f in fields {
                    scripts_removed += self.strip_field(&f, &mut edits, &mut seen);
                }
                match form_ref {
                    Some(r) => edits.set(r, form),
                    None => catalog.set("AcroForm", form),
                }
            }
            edits.set(root, catalog);
        }
        for page in self.pages() {
            let mut dict = edits.dict(self, page).unwrap_or_default();
            if dict.remove("AA").is_some() {
                scripts_removed += 1;
            }
            let annots = match self.resolve_key(&dict, "Annots") {
                Object::Array(a) => a,
                _ => Vec::new(),
            };
            let mut kept = Vec::new();
            for a in annots {
                let Some(r) = a.as_ref() else {
                    kept.push(a);
                    continue;
                };
                let Some(mut annot) = edits.dict(self, r) else {
                    continue;
                };
                if annot.remove("AA").is_some() {
                    scripts_removed += 1;
                }
                if annot.get("A").is_some_and(|a| self.is_javascript(a)) {
                    annot.remove("A");
                    scripts_removed += 1;
                }
                // Drawn when printed, never only on screen.
                let flags = annot.int("F").unwrap_or(0);
                annot.set("F", (flags | 4) & !32);
                edits.set(r, annot);
                kept.push(a);
            }
            if dict.get("Annots").is_some() {
                dict.set("Annots", kept);
            }
            edits.set(page, dict);
        }

        // Colours: an sRGB output intent.
        let icc = up.add(Object::Stream(crate::dss::compressed_with(
            Dict(vec![(Name::new("N"), Object::Int(3))]),
            &srgb_profile(),
        )));
        let intent = Dict(vec![
            (Name::new("Type"), Object::name("OutputIntent")),
            (Name::new("S"), Object::name("GTS_PDFA1")),
            (
                Name::new("OutputConditionIdentifier"),
                Object::string("sRGB IEC61966-2.1"),
            ),
            (Name::new("Info"), Object::string("sRGB IEC61966-2.1")),
            (
                Name::new("RegistryName"),
                Object::string("http://www.color.org"),
            ),
            (Name::new("DestOutputProfile"), Object::Ref(icc)),
        ]);
        let intent = up.add(Object::Dict(intent));

        // Says what it is, in XMP that agrees with /Info.
        let now = crate::annots::pdf_date(std::time::SystemTime::now());
        let meta = self.metadata();
        let info_ref = self.trailer().get("Info").and_then(Object::as_ref);
        let mut info = match self.trailer().get("Info").map(|i| self.resolve(i)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        info.set("ModDate", Object::string(&now));
        info.remove("Trapped");
        match info_ref {
            Some(r) => up.replace(r, Object::Dict(info)),
            None => {
                let r = up.add(Object::Dict(info));
                up.trailer_mut().set("Info", Object::Ref(r));
            }
        }
        let packet = crate::meta::xmp(
            &meta,
            meta.created.as_deref(),
            &now,
            "<pdfaid:part>2</pdfaid:part><pdfaid:conformance>B</pdfaid:conformance>",
        );
        let xmp = up.add(Object::Stream(Stream {
            dict: Dict(vec![
                (Name::new("Type"), Object::name("Metadata")),
                (Name::new("Subtype"), Object::name("XML")),
                (Name::new("Length"), Object::Int(packet.len() as i64)),
            ]),
            data: packet.into_bytes(),
        }));
        if let Some(root) = root {
            let mut catalog = edits.dict(self, root).unwrap_or_default();
            catalog.set("OutputIntents", vec![Object::Ref(intent)]);
            catalog.set("Metadata", Object::Ref(xmp));
            edits.set(root, catalog);
        }

        for (r, dict) in edits.changed {
            up.replace(r, Object::Dict(dict));
        }
        let done = Document::load(up.finish()).map_err(|e| PdfaError::Write(e.to_string()))?;
        let bytes = done.write_whole(NewLock::Off, true, &BTreeMap::new())?;
        Ok(Pdfa {
            bytes,
            fonts_embedded,
            scripts_removed,
            attachments_removed,
            left,
        })
    }

    /// Whether `a` is an action PDF/A does not allow (or a chain with
    /// one): a script, a program run, a sound or film, a form reset,
    /// fields hidden, and so on.
    fn is_javascript(&self, a: &Object) -> bool {
        let mut seen = HashSet::new();
        let mut todo = vec![a.clone()];
        while let Some(a) = todo.pop() {
            if let Some(r) = a.as_ref()
                && !seen.insert(r)
            {
                continue;
            }
            let Some(dict) = self.resolve(&a).as_dict().cloned() else {
                continue;
            };
            if !matches!(
                dict.name("S").unwrap_or_default(),
                b"GoTo" | b"GoToR" | b"GoToE" | b"URI" | b"SubmitForm" | b"Named"
            ) {
                return true;
            }
            if dict.name("S") == Some(b"Named")
                && !matches!(
                    self.resolve_key(&dict, "N").as_name().unwrap_or_default(),
                    b"NextPage" | b"PrevPage" | b"FirstPage" | b"LastPage"
                )
            {
                return true;
            }
            match self.resolve_key(&dict, "Next") {
                Object::Array(a) => todo.extend(a),
                Object::Dict(d) => todo.push(Object::Dict(d)),
                _ => {}
            }
        }
        false
    }

    /// Takes the actions on events off field `f` and its kids; how
    /// many were taken off.
    fn strip_field(&self, f: &Object, edits: &mut Edits, seen: &mut HashSet<Ref>) -> usize {
        let Some(r) = f.as_ref() else {
            return 0;
        };
        if !seen.insert(r) {
            return 0;
        }
        let Some(mut dict) = edits.dict(self, r) else {
            return 0;
        };
        let mut n = 0;
        if dict.remove("AA").is_some() {
            n += 1;
            edits.set(r, dict.clone());
        }
        if let Object::Array(kids) = self.resolve_key(&dict, "Kids") {
            for kid in kids {
                n += self.strip_field(&kid, edits, seen);
            }
        }
        n
    }

    /// Whether font `dict` carries its program.
    fn font_embedded(&self, dict: &Dict) -> bool {
        let descriptor = |d: &Dict| match self.resolve_key(d, "FontDescriptor") {
            Object::Dict(fd) => ["FontFile", "FontFile2", "FontFile3"]
                .iter()
                .any(|k| fd.get(k).is_some()),
            _ => false,
        };
        if dict.name("Subtype") == Some(b"Type3") {
            return true;
        }
        if dict.name("Subtype") == Some(b"Type0") {
            let Object::Array(desc) = self.resolve_key(dict, "DescendantFonts") else {
                return false;
            };
            return desc
                .first()
                .and_then(|d| self.resolve(d).as_dict().cloned())
                .is_some_and(|d| descriptor(&d));
        }
        descriptor(dict)
    }

    /// Every font the pages, their forms and their annotations draw
    /// with: its reference, when it has one, and its dictionary.
    fn all_fonts(&self) -> Vec<(Option<Ref>, Dict)> {
        let mut out = Vec::new();
        let mut seen_fonts = HashSet::new();
        let mut seen_forms = HashSet::new();
        for page in self.pages() {
            let dict = page_dict(self, page);
            if let Object::Dict(res) = self.resolve_key(&dict, "Resources") {
                self.fonts_in(&res, &mut seen_fonts, &mut seen_forms, &mut out, 0);
            }
            let annots = match self.resolve_key(&dict, "Annots") {
                Object::Array(a) => a,
                _ => Vec::new(),
            };
            for a in annots {
                let Some(annot) = self.resolve(&a).as_dict().cloned() else {
                    continue;
                };
                let Object::Dict(ap) = self.resolve_key(&annot, "AP") else {
                    continue;
                };
                let mut streams = Vec::new();
                for (_, v) in ap.iter() {
                    match self.resolve(v) {
                        Object::Stream(s) => streams.push(s),
                        Object::Dict(states) => {
                            for (_, s) in states.iter() {
                                if let Object::Stream(s) = self.resolve(s) {
                                    streams.push(s);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                for s in streams {
                    if let Object::Dict(res) = self.resolve_key(&s.dict, "Resources") {
                        self.fonts_in(&res, &mut seen_fonts, &mut seen_forms, &mut out, 0);
                    }
                }
            }
        }
        out
    }

    fn fonts_in(
        &self,
        res: &Dict,
        seen_fonts: &mut HashSet<Ref>,
        seen_forms: &mut HashSet<Ref>,
        out: &mut Vec<(Option<Ref>, Dict)>,
        depth: usize,
    ) {
        if let Object::Dict(fonts) = self.resolve_key(res, "Font") {
            for (_, f) in fonts.iter() {
                let r = f.as_ref();
                if let Some(r) = r
                    && !seen_fonts.insert(r)
                {
                    continue;
                }
                if let Some(dict) = self.resolve(f).as_dict() {
                    out.push((r, dict.clone()));
                }
            }
        }
        if depth > 8 {
            return;
        }
        if let Object::Dict(xobjects) = self.resolve_key(res, "XObject") {
            for (_, x) in xobjects.iter() {
                if let Some(r) = x.as_ref()
                    && !seen_forms.insert(r)
                {
                    continue;
                }
                if let Object::Stream(s) = self.resolve(x)
                    && s.dict.name("Subtype") == Some(b"Form")
                    && let Object::Dict(inner) = self.resolve_key(&s.dict, "Resources")
                {
                    self.fonts_in(&inner, seen_fonts, seen_forms, out, depth + 1);
                }
            }
        }
    }
}

/// Whether PDF/A allows action `a` where a click or opening runs it.
fn action_allowed(a: &FieldAction) -> bool {
    match a {
        FieldAction::GoTo(_) | FieldAction::Uri(_) | FieldAction::Submit { .. } => true,
        FieldAction::Named(n) => {
            matches!(
                n.as_str(),
                "NextPage" | "PrevPage" | "FirstPage" | "LastPage"
            )
        }
        FieldAction::Reset { .. } | FieldAction::Hide { .. } | FieldAction::JavaScript(_) => false,
    }
}

/// Dictionaries changed, by reference, so that two changes to one land
/// together.
#[derive(Default)]
struct Edits {
    changed: BTreeMap<Ref, Dict>,
}

impl Edits {
    fn dict(&self, doc: &Document, r: Ref) -> Option<Dict> {
        if let Some(d) = self.changed.get(&r) {
            return Some(d.clone());
        }
        match doc.get(r)? {
            Object::Dict(d) => Some(d),
            _ => None,
        }
    }

    fn set(&mut self, r: Ref, dict: Dict) {
        self.changed.insert(r, dict);
    }
}

/// The font to embed for `name`, whose standard stand-in is `standard`.
fn want_of(name: &str, standard: &str) -> FontWant {
    let lower = name.to_ascii_lowercase();
    FontWant {
        family: if standard.starts_with("Courier") {
            FontFamily::Mono
        } else if standard.starts_with("Times") {
            FontFamily::Serif
        } else {
            FontFamily::Sans
        },
        bold: standard.contains("Bold")
            || ["bold", "black", "heavy", "semibold", "demi"]
                .iter()
                .any(|w| lower.contains(w)),
        italic: standard.contains("Oblique")
            || standard.contains("Italic")
            || lower.contains("italic")
            || lower.contains("oblique"),
    }
}

/// Simple font `dict` with `tt` embedded for it, keeping its encoding
/// and widths; `None` when `tt` lacks the glyphs it needs.
fn embed_simple(doc: &Document, up: &mut Update, dict: &Dict, tt: &TrueType) -> Option<Dict> {
    let font = Font::load(doc, dict);
    let mut chars: Vec<(u32, u16)> = Vec::new();
    let mut codes_mapped = 0;
    let mut codes_seen = 0;
    for code in 0..256u32 {
        let Some(text) = font.unicode(code) else {
            continue;
        };
        let Some(c) = text.chars().next() else {
            continue;
        };
        codes_seen += 1;
        if let Some(g) = tt.gid(c) {
            chars.push((c as u32, g));
            codes_mapped += 1;
        }
    }
    if codes_seen == 0 || codes_mapped * 10 < codes_seen * 9 {
        return None;
    }
    let mut gids: Vec<u16> = chars.iter().map(|(_, g)| *g).collect();
    gids.sort_unstable();
    gids.dedup();
    let file = tt.subset_with_cmap(&gids, &chars);
    let mut stream = crate::dss::compressed(&file);
    stream.dict.set("Length1", file.len() as i64);
    let file_ref = up.add(Object::Stream(stream));
    let mut hash: u32 = 2166136261;
    for g in &gids {
        hash = (hash ^ u32::from(*g)).wrapping_mul(16777619);
    }
    let tag: String = (0..6)
        .map(|i| char::from(b'A' + ((hash >> (5 * i)) % 26) as u8))
        .collect();
    let base = format!("{tag}+{}", tt.postscript_name);
    let mut descriptor = truetype_descriptor(tt, &base, file_ref);
    // Not a symbolic font: its codes mean what its encoding says.
    let flags = descriptor.int("Flags").unwrap_or(32) & !4 | 32;
    descriptor.set("Flags", flags);
    if let Object::Dict(old) = doc.resolve_key(dict, "FontDescriptor")
        && let Some(w) = old.get("MissingWidth")
    {
        descriptor.set("MissingWidth", w.clone());
    }
    let descriptor_ref = up.add(Object::Dict(descriptor));
    let mut new = dict.clone();
    new.set("Subtype", Object::name("TrueType"));
    new.set("BaseFont", Object::name(&base));
    new.set("FontDescriptor", Object::Ref(descriptor_ref));
    // The encoding it had by default, the standard one, said in terms
    // every reader follows for an embedded TrueType font.
    if new.get("Encoding").is_none() {
        new.set("Encoding", Object::Dict(standard_as_differences()));
    }
    if new.get("Widths").is_none() {
        let first = 32;
        let last = 255;
        new.set("FirstChar", first);
        new.set("LastChar", last);
        new.set(
            "Widths",
            (first..=last)
                .map(|code| number((font.width(code as u32) * 1000.0).round()))
                .collect::<Vec<_>>(),
        );
    }
    Some(new)
}

/// A small ICC profile for sRGB (version 2, display class), as an
/// output intent needs.
pub fn srgb_profile() -> Vec<u8> {
    fn fixed(v: f64) -> [u8; 4] {
        ((v * 65536.0).round() as i32).to_be_bytes()
    }
    fn xyz(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut t = b"XYZ \0\0\0\0".to_vec();
        t.extend_from_slice(&fixed(x));
        t.extend_from_slice(&fixed(y));
        t.extend_from_slice(&fixed(z));
        t
    }
    fn desc(text: &str) -> Vec<u8> {
        let mut t = b"desc\0\0\0\0".to_vec();
        t.extend_from_slice(&((text.len() + 1) as u32).to_be_bytes());
        t.extend_from_slice(text.as_bytes());
        t.push(0);
        t.extend_from_slice(&[0; 4]); // Unicode language code
        t.extend_from_slice(&[0; 4]); // Unicode count
        t.extend_from_slice(&[0; 2]); // ScriptCode code
        t.push(0); // ScriptCode count
        t.extend_from_slice(&[0; 67]);
        t
    }
    fn text(s: &str) -> Vec<u8> {
        let mut t = b"text\0\0\0\0".to_vec();
        t.extend_from_slice(s.as_bytes());
        t.push(0);
        t
    }
    // A curve of one entry is a gamma, in u8.8.
    let curve = {
        let mut t = b"curv\0\0\0\0".to_vec();
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&((2.2f64 * 256.0).round() as u16).to_be_bytes());
        t
    };
    let tags: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"desc", desc("sRGB IEC61966-2.1")),
        (*b"cprt", text("Public domain")),
        (*b"wtpt", xyz(0.9642, 1.0, 0.8249)),
        (*b"rXYZ", xyz(0.4361, 0.2225, 0.0139)),
        (*b"gXYZ", xyz(0.3851, 0.7169, 0.0971)),
        (*b"bXYZ", xyz(0.1431, 0.0606, 0.7141)),
        (*b"rTRC", curve.clone()),
        (*b"gTRC", curve.clone()),
        (*b"bTRC", curve),
    ];
    let mut body = Vec::new();
    let mut table = Vec::new();
    let mut offset = 128 + 4 + 12 * tags.len();
    for (sig, data) in &tags {
        table.extend_from_slice(sig);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 128 + 4 + 12 * tags.len() + body.len();
    }
    let size = 128 + 4 + 12 * tags.len() + body.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(&(size as u32).to_be_bytes());
    out.extend_from_slice(b"none");
    out.extend_from_slice(&0x0210_0000u32.to_be_bytes());
    out.extend_from_slice(b"mntr");
    out.extend_from_slice(b"RGB ");
    out.extend_from_slice(b"XYZ ");
    out.extend_from_slice(&[0x07, 0xEA, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]); // 2026-01-01
    out.extend_from_slice(b"acsp");
    out.extend_from_slice(&[0; 4]); // platform
    out.extend_from_slice(&[0; 4]); // flags
    out.extend_from_slice(&[0; 4]); // manufacturer
    out.extend_from_slice(&[0; 4]); // model
    out.extend_from_slice(&[0; 8]); // attributes
    out.extend_from_slice(&[0; 4]); // rendering intent
    out.extend_from_slice(&fixed(0.9642));
    out.extend_from_slice(&fixed(1.0));
    out.extend_from_slice(&fixed(0.8249));
    out.extend_from_slice(b"katn");
    out.extend_from_slice(&[0; 16]); // profile id
    out.extend_from_slice(&[0; 28]);
    debug_assert_eq!(out.len(), 128);
    out.extend_from_slice(&(tags.len() as u32).to_be_bytes());
    out.extend_from_slice(&table);
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_well_formed() {
        let p = srgb_profile();
        assert_eq!(
            u32::from_be_bytes([p[0], p[1], p[2], p[3]]) as usize,
            p.len()
        );
        assert_eq!(&p[36..40], b"acsp");
        assert_eq!(&p[16..20], b"RGB ");
        let count = u32::from_be_bytes([p[128], p[129], p[130], p[131]]) as usize;
        assert_eq!(count, 9);
        for i in 0..count {
            let at = 132 + 12 * i;
            let offset = u32::from_be_bytes([p[at + 4], p[at + 5], p[at + 6], p[at + 7]]) as usize;
            let size = u32::from_be_bytes([p[at + 8], p[at + 9], p[at + 10], p[at + 11]]) as usize;
            assert!(offset + size <= p.len());
            assert_eq!(offset % 4, 0);
        }
    }

    #[test]
    fn wants_follow_the_name() {
        let w = want_of("Arial-BoldMT", "Helvetica-Bold");
        assert_eq!(w.family, FontFamily::Sans);
        assert!(w.bold && !w.italic);
        let w = want_of("Courier-Oblique", "Courier-Oblique");
        assert_eq!(w.family, FontFamily::Mono);
        assert!(!w.bold && w.italic);
        let w = want_of("TimesNewRoman,BoldItalic", "Times-BoldItalic");
        assert_eq!(w.family, FontFamily::Serif);
        assert!(w.bold && w.italic);
    }
}
