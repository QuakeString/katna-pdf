// SPDX-License-Identifier: GPL-3.0-or-later

//! Links on the pages (ISO 32000-2 §12.5.6.5): the areas of each page
//! that go to another place in the document or open a web address.

use crate::outline::{Destination, OutlineReader};
use crate::{Dict, Document, Object, text_string};

/// Annotations looked at in the whole file before the rest are ignored.
const MAX_ANNOTATIONS: usize = 500_000;

/// How a link to a page shows it, and how the link looks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LinkStyle {
    pub fit: LinkFit,
    /// A thin line is drawn round the link.
    pub boxed: bool,
    /// The named place it goes to, kept by its name.
    pub named: Option<String>,
}

/// How the page a link goes to is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkFit {
    /// The whole page (`/Fit`).
    #[default]
    Page,
    /// Its width (`/FitH`).
    Width,
    /// At the zoom the reader has now (`/XYZ` with no zoom).
    AsNow,
}

/// One link: its area on the page and where it goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// Left, bottom, right, top in the page's default user space.
    pub rect: [f32; 4],
    pub target: LinkTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkTarget {
    /// A place in this document.
    Page(Destination),
    /// A web (or mail) address.
    Uri(String),
}

impl Document {
    /// Each page's links, in page order.
    /// A locked file not opened with its password has none.
    pub fn links(&self) -> Vec<Vec<Link>> {
        let pages = self.pages();
        if self.is_locked() {
            return vec![Vec::new(); pages.len()];
        }
        let Some(catalog) = self.catalog() else {
            return vec![Vec::new(); pages.len()];
        };
        let mut reader = OutlineReader::new(self, catalog);
        let mut left = MAX_ANNOTATIONS;
        pages
            .iter()
            .map(|&page| {
                let Some(Object::Dict(page)) = self.get(page) else {
                    return Vec::new();
                };
                let Object::Array(annots) = self.resolve_key(&page, "Annots") else {
                    return Vec::new();
                };
                let n = annots.len().min(left);
                left -= n;
                annots[..n]
                    .iter()
                    .filter_map(|annot| match self.resolve(annot) {
                        Object::Dict(annot) => self.link(&annot, &mut reader),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    pub(crate) fn link(&self, annot: &Dict, reader: &mut OutlineReader) -> Option<Link> {
        if annot.name("Subtype") != Some(b"Link") {
            return None;
        }
        let Object::Array(rect) = self.resolve_key(annot, "Rect") else {
            return None;
        };
        let [x0, y0, x1, y1] = rect
            .iter()
            .map(|n| self.resolve(n).as_number())
            .collect::<Option<Vec<_>>>()?[..]
        else {
            return None;
        };
        let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)].map(|v| v as f32);
        let target = match self.resolve_key(annot, "A") {
            Object::Dict(action) if action.name("S") == Some(b"URI") => {
                match self.resolve_key(&action, "URI") {
                    // URIs are 7-bit ASCII; text strings are accepted too.
                    Object::String { bytes, .. } => LinkTarget::Uri(text_string(&bytes)),
                    _ => return None,
                }
            }
            _ => LinkTarget::Page(reader.item_destination(annot)?),
        };
        Some(Link { rect, target })
    }

    /// How each link on page `page`, in the order of `links()`, shows
    /// the page it goes to and looks.
    pub fn link_styles(&self, page: usize) -> Vec<LinkStyle> {
        let Some(&page_ref) = self.pages().get(page) else {
            return Vec::new();
        };
        let Some(Object::Dict(page)) = self.get(page_ref) else {
            return Vec::new();
        };
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let mut reader = OutlineReader::new(self, catalog);
        let Object::Array(annots) = self.resolve_key(&page, "Annots") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for annot in annots.iter().take(MAX_ANNOTATIONS) {
            let Object::Dict(a) = self.resolve(annot) else {
                continue;
            };
            if self.link(&a, &mut reader).is_none() {
                continue;
            }
            let width = |o: Object| match o {
                Object::Array(b) => b.get(2).and_then(|w| self.resolve(w).as_number()),
                Object::Dict(bs) => self.resolve_key(&bs, "W").as_number(),
                _ => None,
            };
            let boxed = width(self.resolve_key(&a, "Border"))
                .or_else(|| width(self.resolve_key(&a, "BS")))
                .is_some_and(|w| w > 0.0);
            let dest = match a.get("Dest") {
                Some(d) => self.resolve(d),
                None => match self.resolve_key(&a, "A") {
                    Object::Dict(action) if action.name("S") == Some(b"GoTo") => {
                        self.resolve_key(&action, "D")
                    }
                    _ => Object::Null,
                },
            };
            let named = match &dest {
                Object::Name(n) => Some(text_string(n.as_bytes())),
                Object::String { bytes, .. } => Some(text_string(bytes)),
                _ => None,
            };
            let array = match &dest {
                Object::Array(_) => Some(dest.clone()),
                Object::Name(_) | Object::String { .. } => {
                    reader.raw_destination(&dest).map(|d| self.resolve(&d))
                }
                _ => None,
            };
            let fit = match array {
                Some(Object::Array(parts)) => {
                    match parts
                        .get(1)
                        .map(|p| self.resolve(p))
                        .as_ref()
                        .and_then(Object::as_name)
                    {
                        Some(b"FitH" | b"FitBH") => LinkFit::Width,
                        Some(b"XYZ") => LinkFit::AsNow,
                        _ => LinkFit::Page,
                    }
                }
                _ => LinkFit::Page,
            };
            out.push(LinkStyle { fit, boxed, named });
        }
        out
    }
}
