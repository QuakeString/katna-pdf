// SPDX-License-Identifier: GPL-3.0-or-later

//! Links on the pages (ISO 32000-2 §12.5.6.5): the areas of each page
//! that go to another place in the document or open a web address.

use crate::outline::{Destination, OutlineReader};
use crate::{Dict, Document, Object, text_string};

/// Annotations looked at in the whole file before the rest are ignored.
const MAX_ANNOTATIONS: usize = 500_000;

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
    /// Encrypted files' addresses cannot be read yet, so they have none.
    pub fn links(&self) -> Vec<Vec<Link>> {
        let pages = self.pages();
        if self.is_encrypted() {
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

    fn link(&self, annot: &Dict, reader: &mut OutlineReader) -> Option<Link> {
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
}
