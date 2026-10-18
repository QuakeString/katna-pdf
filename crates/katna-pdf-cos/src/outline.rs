// SPDX-License-Identifier: GPL-3.0-or-later

//! The document outline, the "bookmarks" (ISO 32000-2 §12.3.3), and the
//! destinations its items go to (§12.3.2).

use std::collections::{HashMap, HashSet};

use crate::{Dict, Document, Object, Ref};

/// How deep an outline may nest, and how many items it may hold, before
/// the rest is ignored: a broken or hostile file cannot make us loop.
const MAX_DEPTH: usize = 64;
const MAX_ITEMS: usize = 100_000;
/// How deep a name tree may nest.
const MAX_TREE_DEPTH: usize = 32;

/// One outline item and the items under it.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineItem {
    pub title: String,
    /// Where it goes, when that is a page of this file.
    pub dest: Option<Destination>,
    /// Shown open at first (a positive `/Count`).
    pub open: bool,
    pub children: Vec<OutlineItem>,
}

/// A place in the document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Destination {
    /// The page's index, from 0.
    pub page: usize,
    /// The height on the page, in default user space, that should be at
    /// the top of the view, when the destination gives one.
    pub top: Option<f32>,
}

impl Document {
    /// The outline, or nothing if the file has none. Encrypted files'
    /// titles cannot be read yet, so they have none either.
    pub fn outline(&self) -> Vec<OutlineItem> {
        if self.is_encrypted() {
            return Vec::new();
        }
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let Object::Dict(outlines) = self.resolve_key(&catalog, "Outlines") else {
            return Vec::new();
        };
        let mut reader = OutlineReader {
            doc: self,
            catalog,
            pages: None,
            seen: HashSet::new(),
            items: 0,
        };
        reader.children(&outlines, 0)
    }
}

struct OutlineReader<'a> {
    doc: &'a Document,
    catalog: Dict,
    /// Each page object's index, found on first need.
    pages: Option<HashMap<Ref, usize>>,
    seen: HashSet<Ref>,
    items: usize,
}

impl OutlineReader<'_> {
    /// The items under `parent`, following `/First` and each `/Next`.
    fn children(&mut self, parent: &Dict, depth: usize) -> Vec<OutlineItem> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        let mut next = parent.get("First").and_then(Object::as_ref);
        while let Some(r) = next {
            if self.items >= MAX_ITEMS || !self.seen.insert(r) {
                break;
            }
            let Some(Object::Dict(item)) = self.doc.get(r) else {
                break;
            };
            self.items += 1;
            let title = match self.doc.resolve_key(&item, "Title") {
                Object::String { bytes, .. } => text_string(&bytes),
                _ => String::new(),
            };
            let dest = self.item_destination(&item);
            let open = item.int("Count").is_some_and(|c| c > 0);
            let children = self.children(&item, depth + 1);
            out.push(OutlineItem {
                title,
                dest,
                open,
                children,
            });
            next = item.get("Next").and_then(Object::as_ref);
        }
        out
    }

    /// An item's `/Dest`, or the destination of its `/A` go-to action.
    fn item_destination(&mut self, item: &Dict) -> Option<Destination> {
        if let Some(dest) = item.get("Dest") {
            return self.destination(&self.doc.resolve(dest), 0);
        }
        let Object::Dict(action) = self.doc.resolve_key(item, "A") else {
            return None;
        };
        if action.name("S") != Some(b"GoTo") {
            return None;
        }
        let dest = self.doc.resolve_key(&action, "D");
        self.destination(&dest, 0)
    }

    /// An explicit destination array, or a named one looked up.
    fn destination(&mut self, dest: &Object, depth: usize) -> Option<Destination> {
        match dest {
            Object::Array(parts) => self.explicit(parts),
            // A named destination's value may be the array itself or a
            // dictionary holding it in `/D`.
            Object::Dict(d) if depth < 2 => {
                let inner = self.doc.resolve_key(d, "D");
                self.destination(&inner, depth + 1)
            }
            Object::Name(name) if depth < 2 => {
                let found = self.named_in_dests(name.as_bytes())?;
                self.destination(&found, depth + 1)
            }
            Object::String { bytes, .. } if depth < 2 => {
                let found = self.named_in_tree(bytes)?;
                self.destination(&found, depth + 1)
            }
            _ => None,
        }
    }

    /// `[page /XYZ left top zoom]` and the other forms (§12.3.2.2).
    fn explicit(&mut self, parts: &[Object]) -> Option<Destination> {
        let page = match parts.first()? {
            Object::Ref(r) => *self.page_indices().get(r)?,
            // Destinations in other files give a page number; some
            // writers use that form within the same file too.
            other => usize::try_from(other.as_int()?).ok()?,
        };
        let number = |i: usize| parts.get(i).and_then(Object::as_number);
        let top = match parts.get(1).and_then(Object::as_name) {
            Some(b"XYZ") => number(3),
            Some(b"FitH" | b"FitBH") => number(2),
            Some(b"FitR") => number(5),
            _ => None,
        };
        Some(Destination {
            page,
            top: top.map(|t| t as f32),
        })
    }

    fn page_indices(&mut self) -> &HashMap<Ref, usize> {
        self.pages.get_or_insert_with(|| {
            self.doc
                .pages()
                .into_iter()
                .enumerate()
                .map(|(i, r)| (r, i))
                .collect()
        })
    }

    /// A name in the catalog's `/Dests` dictionary (PDF 1.1).
    fn named_in_dests(&self, name: &[u8]) -> Option<Object> {
        let Object::Dict(dests) = self.doc.resolve_key(&self.catalog, "Dests") else {
            // Newer writers put names in the name tree whichever type
            // they use.
            return self.named_in_tree(name);
        };
        let value = dests.0.iter().find(|(k, _)| k.as_bytes() == name)?;
        Some(self.doc.resolve(&value.1))
    }

    /// A string in the `/Names /Dests` name tree (§7.9.6).
    fn named_in_tree(&self, key: &[u8]) -> Option<Object> {
        let Object::Dict(names) = self.doc.resolve_key(&self.catalog, "Names") else {
            return None;
        };
        let Object::Dict(root) = self.doc.resolve_key(&names, "Dests") else {
            return None;
        };
        self.find_in_tree(&root, key, 0, &mut HashSet::new())
    }

    fn find_in_tree(
        &self,
        node: &Dict,
        key: &[u8],
        depth: usize,
        seen: &mut HashSet<Ref>,
    ) -> Option<Object> {
        if depth > MAX_TREE_DEPTH {
            return None;
        }
        if let Object::Array(pairs) = self.doc.resolve_key(node, "Names") {
            for pair in pairs.as_chunks::<2>().0 {
                if let Object::String { bytes, .. } = self.doc.resolve(&pair[0])
                    && bytes == key
                {
                    return Some(self.doc.resolve(&pair[1]));
                }
            }
        }
        let Object::Array(kids) = self.doc.resolve_key(node, "Kids") else {
            return None;
        };
        for kid in kids.iter().filter_map(Object::as_ref) {
            if !seen.insert(kid) {
                continue;
            }
            let Some(Object::Dict(kid)) = self.doc.get(kid) else {
                continue;
            };
            // Each kid gives the first and last keys under it; skip
            // those that cannot hold ours. Keys compare as bytes.
            if let Object::Array(limits) = self.doc.resolve_key(&kid, "Limits")
                && let [lo, hi] = &limits[..]
                && let (Object::String { bytes: lo, .. }, Object::String { bytes: hi, .. }) =
                    (self.doc.resolve(lo), self.doc.resolve(hi))
                && (key < lo.as_slice() || key > hi.as_slice())
            {
                continue;
            }
            if let Some(found) = self.find_in_tree(&kid, key, depth + 1, seen) {
                return Some(found);
            }
        }
        None
    }
}

/// A text string (§7.9.2.2): UTF-16BE or UTF-8 after a byte-order mark,
/// otherwise PDFDocEncoding.
pub fn text_string(bytes: &[u8]) -> String {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_be_bytes(c))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| pdf_doc_char(b)).collect()
    };
    // Titles sometimes carry line breaks or other control characters.
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// PDFDocEncoding (Annex D.2): Latin-1, apart from 0x18–0x1F and
/// 0x80–0xA0.
fn pdf_doc_char(b: u8) -> char {
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{fffd}', '€',
    ];
    match b {
        0x18..=0x1f => LOW[usize::from(b - 0x18)],
        0x80..=0xa0 => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}
