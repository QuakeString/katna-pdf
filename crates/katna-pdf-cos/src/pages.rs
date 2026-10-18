// SPDX-License-Identifier: GPL-3.0-or-later

//! Page operations (docs/PLAN.md, Phase 2, "page operations"): a new file
//! whose pages are taken, in any order, from one or more documents, each
//! turned by quarter turns. Rotating, deleting, moving, duplicating and
//! inserting pages, merging files and splitting them are all this one
//! operation.
//!
//! The file is written whole, not as an incremental update: the pages
//! left out and everything only they used are dropped, so deleting pages
//! makes the file smaller. Everything else the base document's catalog
//! holds (its outline, form, named destinations, metadata) is kept; links
//! to pages left out go nowhere.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;

use crate::document::Document;
use crate::object::{Dict, Object, Ref};
use crate::write::write_object;

/// Attributes a page takes from the page tree above it when it lacks them
/// (ISO 32000-2 §7.7.3.4).
const INHERITED: [&str; 4] = ["Resources", "MediaBox", "CropBox", "Rotate"];

/// Deepest page tree walked for inherited attributes.
const MAX_DEPTH: usize = 64;

/// One page of the new file: page `page` of `doc`, turned clockwise by
/// `turns` quarter turns more than it already is, and cropped to `crop`
/// when given.
#[derive(Clone, Copy)]
pub struct PageSource<'a> {
    pub doc: &'a Document,
    pub page: usize,
    pub turns: i32,
    /// The part of the page kept (left, bottom, right, top, in its user
    /// space), within its media box; the media box itself uncrops it.
    /// Left out, the page's crop is kept.
    pub crop: Option<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    /// Copying out of an encrypted file needs its key (not in v0).
    Encrypted,
    /// A page that is not in its document.
    NoSuchPage(usize),
    /// No pages: a PDF needs at least one.
    Empty,
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::Encrypted => f.write_str("the PDF is encrypted"),
            ComposeError::NoSuchPage(p) => write!(f, "there is no page {}", p + 1),
            ComposeError::Empty => f.write_str("no pages"),
        }
    }
}

impl std::error::Error for ComposeError {}

/// A new file holding `pages`, in order. The catalog of `base` (its
/// outline, form, names and so on) is kept, and so is its information
/// dictionary; without one the file has only its pages.
pub fn compose(base: Option<&Document>, pages: &[PageSource<'_>]) -> Result<Vec<u8>, ComposeError> {
    if pages.is_empty() {
        return Err(ComposeError::Empty);
    }
    let mut docs: Vec<&Document> = base.into_iter().collect();
    for p in pages {
        if !docs.iter().any(|d| std::ptr::eq(*d, p.doc)) {
            docs.push(p.doc);
        }
    }
    if docs.iter().any(|d| d.is_encrypted()) {
        return Err(ComposeError::Encrypted);
    }
    let trees: Vec<Vec<Ref>> = docs.iter().map(|d| d.pages()).collect();
    let id = |doc: &Document| docs.iter().position(|d| std::ptr::eq(*d, doc)).unwrap_or(0);

    let mut copier = Copier::default();
    // Object 1 is the catalog, 2 the page tree.
    let catalog = copier.reserve();
    let root = copier.reserve();
    // Every page of every document is left out unless placed; the first
    // time a page is placed it keeps its identity, so links to it follow.
    for (d, tree) in trees.iter().enumerate() {
        for &page in tree {
            copier.map.insert((d, page), None);
        }
    }
    let mut kids = Vec::with_capacity(pages.len());
    let mut placed = HashSet::new();
    let mut new_pages = Vec::with_capacity(pages.len());
    for p in pages {
        let d = id(p.doc);
        let &old = trees[d]
            .get(p.page)
            .ok_or(ComposeError::NoSuchPage(p.page))?;
        let first = placed.insert((d, old));
        let num = copier.reserve();
        if first {
            copier.map.insert((d, old), Some(num));
        }
        kids.push(Object::Ref(Ref::new(num, 0)));
        new_pages.push((p, d, old, num, first));
    }
    for (p, d, old, num, first) in new_pages {
        let mut page = page_dict(p.doc, old);
        let rotate = page.int("Rotate").unwrap_or(0) + 90 * i64::from(p.turns);
        if rotate.rem_euclid(360) == 0 {
            page.remove("Rotate");
        } else {
            page.set("Rotate", rotate.rem_euclid(360));
        }
        if let Some(crop) = p.crop
            && let Some(info) = p.doc.page_info(p.page)
        {
            let m = info.media;
            let kept = [
                crop[0].max(m[0]),
                crop[1].max(m[1]),
                crop[2].min(m[2]),
                crop[3].min(m[3]),
            ];
            if kept == m || kept.iter().zip(&m).all(|(a, b)| (a - b).abs() < 0.01) {
                page.remove("CropBox");
            } else if kept[2] - kept[0] >= 1.0 && kept[3] - kept[1] >= 1.0 {
                page.set("CropBox", crate::annots::reals(&kept));
            }
        }
        // A page placed twice shares everything but its annotations: one
        // annotation belongs to one page.
        if !first {
            page.remove("Annots");
        }
        // The parent is set after copying: it is a number in the new
        // file, not the old one.
        let mut page = copier.copy_dict(&docs, d, &page);
        page.set("Parent", Ref::new(root, 0));
        copier.set(num, Object::Dict(page));
    }
    let mut tree = Dict::new();
    tree.set("Type", Object::name("Pages"));
    tree.set("Kids", kids);
    tree.set("Count", pages.len() as i64);
    copier.set(root, Object::Dict(tree));

    let mut cat = base
        .and_then(Document::catalog)
        .map(|c| {
            let mut c = c.clone();
            c.remove("Pages");
            match copier.copy(&docs, 0, &Object::Dict(c)) {
                Object::Dict(d) => d,
                _ => Dict::new(),
            }
        })
        .unwrap_or_default();
    cat.set("Type", Object::name("Catalog"));
    cat.set("Pages", Ref::new(root, 0));
    copier.set(catalog, Object::Dict(cat));

    let mut trailer = Dict::new();
    trailer.set("Root", Ref::new(catalog, 0));
    if let Some(base) = base {
        if let Some(info) = base.trailer().get("Info") {
            let info = copier.copy(&docs, 0, info);
            trailer.set("Info", info);
        }
        if let Some(Object::Array(ids)) = base.trailer().get("ID")
            && let Some(first) = ids.first()
        {
            trailer.set("ID", vec![first.clone(), first.clone()]);
        }
    }
    copier.drain(&docs);
    Ok(copier.write(trailer))
}

/// A page's boxes and turn, as its dictionary and the page tree above it
/// give them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageInfo {
    /// The media box: left, bottom, right, top, in points.
    pub media: [f32; 4],
    /// The crop box (the media box when it has none).
    pub crop: [f32; 4],
    /// Clockwise quarter turns, in degrees: 0, 90, 180 or 270.
    pub rotate: i64,
}

impl PageInfo {
    /// The size shown, in points: the crop box, turned.
    pub fn size(&self) -> (f32, f32) {
        let (w, h) = (
            (self.crop[2] - self.crop[0]).abs(),
            (self.crop[3] - self.crop[1]).abs(),
        );
        if self.rotate % 180 == 0 {
            (w, h)
        } else {
            (h, w)
        }
    }
}

impl Document {
    /// Page `page`'s boxes and turn; a page without a media box is US
    /// Letter, as readers take it.
    pub fn page_info(&self, page: usize) -> Option<PageInfo> {
        let dict = page_dict(self, *self.pages().get(page)?);
        let rect = |key: &str| -> Option<[f32; 4]> {
            let Object::Array(a) = self.resolve_key(&dict, key) else {
                return None;
            };
            let n: Vec<f32> = a
                .iter()
                .filter_map(|v| self.resolve(v).as_number())
                .map(|v| v as f32)
                .collect();
            match n[..] {
                [a, b, c, d] => Some([a.min(c), b.min(d), a.max(c), b.max(d)]),
                _ => None,
            }
        };
        let media = rect("MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]);
        Some(PageInfo {
            media,
            crop: rect("CropBox").unwrap_or(media),
            rotate: dict.int("Rotate").unwrap_or(0).rem_euclid(360) / 90 * 90,
        })
    }
}

/// Page `page` of `doc` as a dictionary standing alone: the attributes it
/// inherits copied in, its parent dropped.
fn page_dict(doc: &Document, page: Ref) -> Dict {
    let mut dict = doc
        .get(page)
        .and_then(|o| o.as_dict().cloned())
        .unwrap_or_default();
    let mut parent = dict.get("Parent").and_then(Object::as_ref);
    let mut seen = HashSet::new();
    for _ in 0..MAX_DEPTH {
        let Some(p) = parent.filter(|p| seen.insert(*p)) else {
            break;
        };
        let Some(node) = doc.get(p).and_then(|o| o.as_dict().cloned()) else {
            break;
        };
        for key in INHERITED {
            if dict.get(key).is_none()
                && let Some(value) = node.get(key)
            {
                dict.set(key, value.clone());
            }
        }
        parent = node.get("Parent").and_then(Object::as_ref);
    }
    dict.remove("Parent");
    // Resolve the rotation, which may be indirect, to a number.
    if let Some(rotate) = dict.get("Rotate").map(|r| doc.resolve(r).as_int()) {
        match rotate {
            Some(r) => dict.set("Rotate", r),
            None => {
                dict.remove("Rotate");
            }
        }
    }
    dict.set("Type", Object::name("Page"));
    dict
}

/// Copies objects out of documents into the new file, numbering them
/// afresh. Each document's objects are copied once, however often they
/// are reached.
#[derive(Default)]
struct Copier {
    /// The new file's objects; object `n` is at `n - 1`.
    objects: Vec<Object>,
    /// (document, old reference) to its new number, or `None` for an
    /// object left out (pages not placed), which becomes `null`.
    map: HashMap<(usize, Ref), Option<u32>>,
    /// Objects numbered but not yet copied.
    pending: VecDeque<(usize, Ref, u32)>,
}

impl Copier {
    fn reserve(&mut self) -> u32 {
        self.objects.push(Object::Null);
        self.objects.len() as u32
    }

    fn set(&mut self, num: u32, obj: Object) {
        self.objects[num as usize - 1] = obj;
    }

    /// `obj` from document `d` with its references renumbered; what they
    /// point to is copied by [`Copier::drain`].
    fn copy(&mut self, docs: &[&Document], d: usize, obj: &Object) -> Object {
        match obj {
            Object::Ref(r) => match self.map.get(&(d, *r)) {
                Some(Some(num)) => Object::Ref(Ref::new(*num, 0)),
                Some(None) => Object::Null,
                None => {
                    // Page tree nodes are not copied: the new file has its
                    // own, and only a page's parent points at them.
                    let target = docs[d].get(*r);
                    if target
                        .as_ref()
                        .and_then(Object::as_dict)
                        .is_some_and(|t| t.name("Type") == Some(b"Pages"))
                        || target.is_none()
                    {
                        self.map.insert((d, *r), None);
                        return Object::Null;
                    }
                    let num = self.reserve();
                    self.map.insert((d, *r), Some(num));
                    self.pending.push_back((d, *r, num));
                    Object::Ref(Ref::new(num, 0))
                }
            },
            Object::Array(items) => {
                Object::Array(items.iter().map(|i| self.copy(docs, d, i)).collect())
            }
            Object::Dict(dict) => Object::Dict(self.copy_dict(docs, d, dict)),
            Object::Stream(s) => {
                let mut s = s.clone();
                s.dict = self.copy_dict(docs, d, &s.dict);
                Object::Stream(s)
            }
            other => other.clone(),
        }
    }

    fn copy_dict(&mut self, docs: &[&Document], d: usize, dict: &Dict) -> Dict {
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            // A stream's length is written from its data.
            if key.as_bytes() == b"Length" && matches!(value, Object::Ref(_)) {
                continue;
            }
            out.0.push((key.clone(), self.copy(docs, d, value)));
        }
        out
    }

    /// Copies everything numbered but not yet copied, and what that
    /// reaches in turn.
    fn drain(&mut self, docs: &[&Document]) {
        while let Some((d, old, num)) = self.pending.pop_front() {
            let obj = docs[d].get(old).unwrap_or(Object::Null);
            let obj = self.copy(docs, d, &obj);
            self.set(num, obj);
        }
    }

    /// The file: header, objects, a classic cross-reference table.
    fn write(self, mut trailer: Dict) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (i, obj) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            let _ = writeln!(out, "{} 0 obj", i + 1);
            write_object(&mut out, obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let _ = write!(
            out,
            "xref\n0 {}\n0000000000 65535 f\r\n",
            self.objects.len() + 1
        );
        for offset in offsets {
            let _ = write!(out, "{offset:010} 00000 n\r\n");
        }
        trailer.set("Size", self.objects.len() as i64 + 1);
        out.extend_from_slice(b"trailer\n");
        write_object(&mut out, &Object::Dict(trailer));
        let _ = write!(out, "\nstartxref\n{xref}\n%%EOF\n");
        out
    }
}
