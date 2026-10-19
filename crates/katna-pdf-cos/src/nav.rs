// SPDX-License-Identifier: GPL-3.0-or-later

//! Changing the ways around a document: the outline ("bookmarks",
//! ISO 32000-2 §12.3.3), the links on its pages (§12.5.6.5) and its named
//! destinations (§12.3.2.3). Each change is an incremental update.

use std::collections::{BTreeMap, HashSet};

use crate::forms::text_object;
use crate::links::Link;
use crate::outline::{Destination, OutlineItem, OutlineReader};
use crate::write::Update;
use crate::{Dict, Document, LinkTarget, Object, Ref, UpdateError, text_string};

/// Name tree nodes looked at before the rest are ignored.
const MAX_NODES: usize = 100_000;
const MAX_TREE_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NavError {
    Update(UpdateError),
    /// A destination names a page the document does not have.
    NoPage(usize),
    /// A link to take out is not there.
    NoLink(usize),
    /// A named destination with no name.
    EmptyName,
}

impl std::fmt::Display for NavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NavError::Update(e) => e.fmt(f),
            NavError::NoPage(p) => write!(f, "the document has no page {}", p + 1),
            NavError::NoLink(i) => write!(f, "the page has no link {}", i + 1),
            NavError::EmptyName => f.write_str("a named destination needs a name"),
        }
    }
}

impl std::error::Error for NavError {}

impl From<UpdateError> for NavError {
    fn from(e: UpdateError) -> Self {
        NavError::Update(e)
    }
}

/// A destination as written: `[page /XYZ null top null]`, or the whole
/// page (`/Fit`) when no height is given.
fn dest_array(pages: &[Ref], dest: &Destination) -> Result<Object, NavError> {
    let page = *pages.get(dest.page).ok_or(NavError::NoPage(dest.page))?;
    Ok(Object::Array(match dest.top {
        Some(top) => vec![
            Object::Ref(page),
            Object::name("XYZ"),
            Object::Null,
            Object::Real(f64::from(top)),
            Object::Null,
        ],
        None => vec![Object::Ref(page), Object::name("Fit")],
    }))
}

fn catalog_ref(doc: &Document) -> Result<Ref, NavError> {
    doc.trailer()
        .get("Root")
        .and_then(Object::as_ref)
        .ok_or(NavError::Update(UpdateError::Repaired))
}

/// The items shown when every open item is open: what `/Count` counts.
fn visible(items: &[OutlineItem]) -> i64 {
    items
        .iter()
        .map(|i| 1 + if i.open { visible(&i.children) } else { 0 })
        .sum()
}

impl Document {
    /// The document with `items` as its whole outline; an empty list
    /// takes the outline out. Items keep their titles, places and which
    /// are shown open; an item with no page to go to becomes a heading.
    pub fn set_outline(&self, items: &[OutlineItem]) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let root = catalog_ref(self)?;
        let pages = self.pages();
        let mut catalog = self.catalog().unwrap_or_default();
        if items.is_empty() {
            catalog.remove("Outlines");
            if catalog.name("PageMode") == Some(b"UseOutlines") {
                catalog.remove("PageMode");
            }
        } else {
            let outlines = up.add(Object::Null);
            let (first, last) = write_items(&mut up, &pages, items, outlines)?;
            let mut d = Dict::new();
            d.set("Type", Object::name("Outlines"));
            d.set("First", first);
            d.set("Last", last);
            d.set("Count", visible(items));
            up.replace(outlines, Object::Dict(d));
            catalog.set("Outlines", outlines);
        }
        up.replace(root, Object::Dict(catalog));
        Ok(up.finish())
    }

    /// The document with page `page`'s links number `remove` (as
    /// [`Document::links`] lists them) taken out and `add` put in.
    pub fn change_links(
        &self,
        page: usize,
        remove: &[usize],
        add: &[Link],
    ) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let pages = self.pages();
        let page_ref = *pages.get(page).ok_or(NavError::NoPage(page))?;
        let Some(Object::Dict(mut page_dict)) = self.get(page_ref) else {
            return Err(NavError::NoPage(page));
        };
        let own = page_dict.get("Annots").and_then(Object::as_ref);
        let annots = match self.resolve_key(&page_dict, "Annots") {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        // Which entries are the links, counted the way `links` counts.
        let mut reader = OutlineReader::new(self, self.catalog().unwrap_or_default());
        let mut is_link = Vec::new();
        for annot in &annots {
            is_link.push(match self.resolve(annot) {
                Object::Dict(a) => self.link(&a, &mut reader).is_some(),
                _ => false,
            });
        }
        let positions: Vec<usize> = (0..annots.len()).filter(|&i| is_link[i]).collect();
        let mut gone = HashSet::new();
        for &i in remove {
            gone.insert(*positions.get(i).ok_or(NavError::NoLink(i))?);
        }
        let mut kept: Vec<Object> = annots
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !gone.contains(i))
            .map(|(_, a)| a)
            .collect();
        for link in add {
            let mut d = Dict::new();
            d.set("Type", Object::name("Annot"));
            d.set("Subtype", Object::name("Link"));
            d.set(
                "Rect",
                link.rect
                    .iter()
                    .map(|&v| Object::Real(f64::from(v)))
                    .collect::<Vec<_>>(),
            );
            d.set(
                "Border",
                vec![Object::Int(0), Object::Int(0), Object::Int(0)],
            );
            d.set("F", 4i64);
            d.set("P", page_ref);
            match &link.target {
                LinkTarget::Page(dest) => d.set("Dest", dest_array(&pages, dest)?),
                LinkTarget::Uri(uri) => {
                    let mut a = Dict::new();
                    a.set("S", Object::name("URI"));
                    a.set(
                        "URI",
                        Object::String {
                            bytes: uri.as_bytes().to_vec(),
                            hex: false,
                        },
                    );
                    d.set("A", a);
                }
            }
            kept.push(Object::Ref(up.add(Object::Dict(d))));
        }
        match own {
            Some(list) => up.replace(list, Object::Array(kept)),
            None => {
                if kept.is_empty() {
                    page_dict.remove("Annots");
                } else {
                    page_dict.set("Annots", Object::Array(kept));
                }
                up.replace(page_ref, Object::Dict(page_dict));
            }
        }
        Ok(up.finish())
    }

    /// The named destinations that go to a page of this file, by name:
    /// those of the catalog's `/Dests` and of the `/Names` tree.
    pub fn named_destinations(&self) -> Vec<(String, Destination)> {
        if self.is_locked() {
            return Vec::new();
        }
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let mut reader = OutlineReader::new(self, catalog.clone());
        let mut out = BTreeMap::new();
        for (key, value) in self.raw_names(&catalog) {
            if let Some(dest) = reader.destination(&value, 0) {
                out.entry(text_string(&key)).or_insert(dest);
            }
        }
        out.into_iter().collect()
    }

    /// The document with `names` as its whole set of named destinations.
    /// Names the file already had keep their exact bytes, so links that
    /// use them still find them.
    pub fn set_named_destinations(
        &self,
        names: &[(String, Destination)],
    ) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let root = catalog_ref(self)?;
        let pages = self.pages();
        let mut catalog = self.catalog().unwrap_or_default();
        let old: Vec<Vec<u8>> = self
            .raw_names(&catalog)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let mut entries = BTreeMap::new();
        for (name, dest) in names {
            if name.trim().is_empty() {
                return Err(NavError::EmptyName);
            }
            let key = match old.iter().find(|k| text_string(k) == *name) {
                Some(k) => k.clone(),
                None => match text_object(name) {
                    Object::String { bytes, .. } => bytes,
                    _ => unreachable!(),
                },
            };
            entries.insert(key, dest_array(&pages, dest)?);
        }
        // The old-style dictionary is kept, with the same names, when the
        // file had one: readers that only know it still find them.
        let had_dict = matches!(self.resolve_key(&catalog, "Dests"), Object::Dict(_));
        if had_dict {
            let mut dests = Dict::new();
            for (k, v) in &entries {
                dests.0.push((crate::Name(k.clone()), v.clone()));
            }
            match catalog.get("Dests").and_then(Object::as_ref) {
                Some(r) => up.replace(r, Object::Dict(dests)),
                None => catalog.set("Dests", dests),
            }
        }
        let mut names_dict = match self.resolve_key(&catalog, "Names") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        if entries.is_empty() {
            names_dict.remove("Dests");
        } else {
            let mut pairs = Vec::new();
            for (k, v) in &entries {
                pairs.push(Object::String {
                    bytes: k.clone(),
                    hex: false,
                });
                pairs.push(v.clone());
            }
            let mut tree = Dict::new();
            tree.set("Names", pairs);
            names_dict.set("Dests", up.add(Object::Dict(tree)));
        }
        let names_own = catalog.get("Names").and_then(Object::as_ref);
        match names_own {
            Some(r) => up.replace(r, Object::Dict(names_dict)),
            None if names_dict.0.is_empty() => {
                catalog.remove("Names");
            }
            None => catalog.set("Names", names_dict),
        }
        up.replace(root, Object::Dict(catalog));
        Ok(up.finish())
    }

    /// Every named destination's key and value as stored.
    fn raw_names(&self, catalog: &Dict) -> Vec<(Vec<u8>, Object)> {
        let mut out = Vec::new();
        if let Object::Dict(dests) = self.resolve_key(catalog, "Dests") {
            for (k, v) in &dests.0 {
                out.push((k.as_bytes().to_vec(), self.resolve(v)));
            }
        }
        if let Object::Dict(names) = self.resolve_key(catalog, "Names")
            && let Object::Dict(root) = self.resolve_key(&names, "Dests")
        {
            let mut seen = HashSet::new();
            self.tree_pairs(&root, 0, &mut seen, &mut out);
        }
        out
    }

    fn tree_pairs(
        &self,
        node: &Dict,
        depth: usize,
        seen: &mut HashSet<Ref>,
        out: &mut Vec<(Vec<u8>, Object)>,
    ) {
        if depth > MAX_TREE_DEPTH || seen.len() > MAX_NODES {
            return;
        }
        if let Object::Array(pairs) = self.resolve_key(node, "Names") {
            for pair in pairs.as_chunks::<2>().0 {
                if let Object::String { bytes, .. } = self.resolve(&pair[0]) {
                    out.push((bytes, self.resolve(&pair[1])));
                }
            }
        }
        if let Object::Array(kids) = self.resolve_key(node, "Kids") {
            for kid in kids.iter().filter_map(Object::as_ref) {
                if seen.insert(kid)
                    && let Some(Object::Dict(kid)) = self.get(kid)
                {
                    self.tree_pairs(&kid, depth + 1, seen, out);
                }
            }
        }
    }
}

/// Writes `items` under `parent`; gives the first and last.
fn write_items(
    up: &mut Update,
    pages: &[Ref],
    items: &[OutlineItem],
    parent: Ref,
) -> Result<(Ref, Ref), NavError> {
    let refs: Vec<Ref> = items.iter().map(|_| up.add(Object::Null)).collect();
    for (i, item) in items.iter().enumerate() {
        let mut d = Dict::new();
        d.set("Title", text_object(&item.title));
        d.set("Parent", parent);
        if i > 0 {
            d.set("Prev", refs[i - 1]);
        }
        if let Some(&next) = refs.get(i + 1) {
            d.set("Next", next);
        }
        if let Some(dest) = &item.dest {
            d.set("Dest", dest_array(pages, dest)?);
        }
        if !item.children.is_empty() {
            let (first, last) = write_items(up, pages, &item.children, refs[i])?;
            d.set("First", first);
            d.set("Last", last);
            let n = visible(&item.children);
            d.set("Count", if item.open { n } else { -n });
        }
        up.replace(refs[i], Object::Dict(d));
    }
    Ok((refs[0], refs[refs.len() - 1]))
}
