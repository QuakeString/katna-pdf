// SPDX-License-Identifier: GPL-3.0-or-later

//! Files kept in the document itself, apart from any page: the
//! catalog's `/Names` `/EmbeddedFiles` tree of file specifications
//! (§7.11). They are listed, read out, added, renamed and taken off;
//! every change writes the tree anew, flat and sorted, as an
//! incremental update. (A file attached as a comment on a page is a
//! `/FileAttachment` annotation, `annots.rs`.)

use std::collections::HashSet;

use crate::document::Document;
use crate::forms::text_object;
use crate::nav::{NavError, catalog_ref};
use crate::object::{Dict, Object, Stream};
use crate::outline::text_string;

/// A file kept in the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// Its name, as shown.
    pub name: String,
    pub description: String,
    /// Its size in bytes, when the file says.
    pub size: Option<u64>,
    /// When it was last changed, as the file says it (a PDF date).
    pub modified: Option<String>,
    /// When it was made, as the file says it (a PDF date).
    pub created: Option<String>,
    /// Its size as kept in the document, packed.
    pub packed: Option<u64>,
    /// A portfolio's values for it (`/CI`), by the schema's keys.
    pub item: Vec<(String, ItemValue)>,
    /// Its key in the tree, as stored.
    key: Vec<u8>,
    /// Its file specification, as stored: a reference or the dictionary.
    spec: Object,
}

/// A value a portfolio gives a file in one of its columns.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemValue {
    Text(String),
    Number(f64),
    /// A PDF date (`D:YYYYMMDD…`).
    Date(String),
}

impl ItemValue {
    /// How two values sort: numbers by size, dates by time, text without
    /// regard to case.
    fn compare(&self, other: &ItemValue) -> std::cmp::Ordering {
        use ItemValue::*;
        match (self, other) {
            (Number(a), Number(b)) => a.total_cmp(b),
            (Date(a), Date(b)) => a.cmp(b),
            (a, b) => a.text().to_lowercase().cmp(&b.text().to_lowercase()),
        }
    }

    /// The value as it is written in the file.
    pub fn text(&self) -> String {
        match self {
            ItemValue::Text(t) | ItemValue::Date(t) => t.clone(),
            ItemValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            ItemValue::Number(n) => format!("{n}"),
        }
    }
}

/// What a portfolio column shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    /// A value of the file's own (`/CI`), text, a date or a number.
    Text,
    Date,
    Number,
    /// The file's name.
    Name,
    Description,
    Modified,
    Created,
    Size,
    /// Its size packed in the document.
    Packed,
}

/// A column a portfolio lists its files in.
#[derive(Debug, Clone, PartialEq)]
pub struct PortfolioColumn {
    /// Its key in the schema, which `/CI` and `/Sort` name it by.
    pub key: String,
    /// Its heading.
    pub name: String,
    pub kind: ColumnKind,
    /// Whether it is shown at first.
    pub visible: bool,
}

/// How a portfolio asks to be shown at first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortfolioView {
    /// A list with a row a file.
    Details,
    /// Small pictures of the files.
    Tiles,
    /// Its own pages first, the files out of the way.
    Hidden,
    /// Its own Flash or HTML navigator, which nothing plays now.
    Custom,
}

/// A portfolio (a "PDF package"): a document whose point is the files
/// kept in it, listed in columns of their own.
#[derive(Debug, Clone, PartialEq)]
pub struct Portfolio {
    /// In the order shown.
    pub columns: Vec<PortfolioColumn>,
    /// The columns the files are sorted by, by key, and whether upwards.
    pub sort: Vec<(String, bool)>,
    /// The name of the file shown first.
    pub initial: Option<String>,
    pub view: PortfolioView,
}

impl Portfolio {
    /// The column with key `key`.
    pub fn column(&self, key: &str) -> Option<&PortfolioColumn> {
        self.columns.iter().find(|c| c.key == key)
    }

    /// `files`' indices in the order the portfolio sorts them, or by
    /// `by` (a column's key, and upwards) when given.
    pub fn order(&self, files: &[Attachment], by: Option<(&str, bool)>) -> Vec<usize> {
        let keys: Vec<(&PortfolioColumn, bool)> = match by {
            Some((key, up)) => self.column(key).map(|c| (c, up)).into_iter().collect(),
            None => self
                .sort
                .iter()
                .filter_map(|(k, up)| self.column(k).map(|c| (c, *up)))
                .collect(),
        };
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by(|&a, &b| {
            for (column, up) in &keys {
                let (x, y) = (files[a].value(column), files[b].value(column));
                let o = match (x, y) {
                    (Some(x), Some(y)) => x.compare(&y),
                    // Files without a value go last either way.
                    (Some(_), None) => return std::cmp::Ordering::Less,
                    (None, Some(_)) => return std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                let o = if *up { o } else { o.reverse() };
                if o.is_ne() {
                    return o;
                }
            }
            std::cmp::Ordering::Equal
        });
        order
    }
}

impl Attachment {
    /// Its value in `column`.
    pub fn value(&self, column: &PortfolioColumn) -> Option<ItemValue> {
        let size = |n: Option<u64>| n.map(|n| ItemValue::Number(n as f64));
        match column.kind {
            ColumnKind::Name => Some(ItemValue::Text(self.name.clone())),
            ColumnKind::Description => {
                (!self.description.is_empty()).then(|| ItemValue::Text(self.description.clone()))
            }
            ColumnKind::Modified => self.modified.clone().map(ItemValue::Date),
            ColumnKind::Created => self.created.clone().map(ItemValue::Date),
            ColumnKind::Size => size(self.size),
            ColumnKind::Packed => size(self.packed),
            ColumnKind::Text | ColumnKind::Date | ColumnKind::Number => self
                .item
                .iter()
                .find(|(k, _)| *k == column.key)
                .map(|(_, v)| v.clone()),
        }
    }
}

/// Whether `s` reads as a PDF date.
fn is_pdf_date(s: &str) -> bool {
    let digits = s.strip_prefix("D:").unwrap_or("");
    digits.len() >= 4 && digits.as_bytes()[..4].iter().all(u8::is_ascii_digit)
}

impl Document {
    /// The files kept in the document, in the tree's order.
    pub fn attachments(&self) -> Vec<Attachment> {
        if self.is_locked() {
            return Vec::new();
        }
        self.raw_attachments()
            .into_iter()
            .filter_map(|(key, spec)| {
                let dict = self.resolve(&spec);
                let dict = dict.as_dict()?;
                let text = |k: &str| match self.resolve_key(dict, k) {
                    Object::String { bytes, .. } => Some(text_string(&bytes)),
                    _ => None,
                };
                let name = text("UF")
                    .or_else(|| text("F"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| text_string(&key));
                let stream = self.embedded_stream(dict);
                let params =
                    stream
                        .as_ref()
                        .and_then(|s| match self.resolve_key(&s.dict, "Params") {
                            Object::Dict(p) => Some(p),
                            _ => None,
                        });
                let size = params
                    .as_ref()
                    .and_then(|p| self.resolve_key(p, "Size").as_number())
                    .map(|n| n.max(0.0) as u64)
                    .or_else(|| {
                        stream.as_ref().and_then(|s| {
                            crate::filter::decode(&s.dict, &s.data)
                                .ok()
                                .map(|d| d.len() as u64)
                        })
                    });
                let date = |key: &str| {
                    params
                        .as_ref()
                        .and_then(|p| match self.resolve_key(p, key) {
                            Object::String { bytes, .. } => Some(text_string(&bytes)),
                            _ => None,
                        })
                };
                let packed = stream.as_ref().map(|s| s.data.len() as u64);
                let item = match self.resolve_key(dict, "CI") {
                    Object::Dict(ci) => ci
                        .iter()
                        .filter_map(|(k, v)| {
                            let value = self.item_value(&self.resolve(v))?;
                            Some((String::from_utf8_lossy(k.as_bytes()).into_owned(), value))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                Some(Attachment {
                    name,
                    description: text("Desc").unwrap_or_default(),
                    size,
                    modified: date("ModDate"),
                    created: date("CreationDate"),
                    packed,
                    item,
                    key,
                    spec,
                })
            })
            .collect()
    }

    /// One value of a collection item: a string, a number or a date,
    /// or a dictionary of one (`/D`) with a prefix shown before it (`/P`).
    fn item_value(&self, v: &Object) -> Option<ItemValue> {
        match v {
            Object::String { bytes, .. } => {
                let text = text_string(bytes);
                Some(if is_pdf_date(&text) {
                    ItemValue::Date(text)
                } else {
                    ItemValue::Text(text)
                })
            }
            Object::Int(_) | Object::Real(_) => v.as_number().map(ItemValue::Number),
            Object::Dict(d) => {
                let value = self.item_value(&self.resolve_key(d, "D"))?;
                match (self.resolve_key(d, "P"), value) {
                    (Object::String { bytes, .. }, ItemValue::Text(t)) => {
                        Some(ItemValue::Text(format!("{}{t}", text_string(&bytes))))
                    }
                    (_, value) => Some(value),
                }
            }
            Object::Name(n) => Some(ItemValue::Text(
                String::from_utf8_lossy(n.as_bytes()).into_owned(),
            )),
            _ => None,
        }
    }

    /// The document as a portfolio (§12.3.5, `/Collection`): the columns
    /// its files are listed in, how they are sorted, and which comes up
    /// first. `None` when it is not one.
    pub fn portfolio(&self) -> Option<Portfolio> {
        if self.is_locked() {
            return None;
        }
        let catalog = self.catalog()?;
        let Object::Dict(c) = self.resolve_key(&catalog, "Collection") else {
            return None;
        };
        let mut columns = Vec::new();
        if let Object::Dict(schema) = self.resolve_key(&c, "Schema") {
            for (key, field) in schema.iter() {
                let Object::Dict(f) = self.resolve(field) else {
                    continue;
                };
                let kind = match f.name("Subtype") {
                    Some(b"S") => ColumnKind::Text,
                    Some(b"D") => ColumnKind::Date,
                    Some(b"N") => ColumnKind::Number,
                    Some(b"F") => ColumnKind::Name,
                    Some(b"Desc") => ColumnKind::Description,
                    Some(b"ModDate") => ColumnKind::Modified,
                    Some(b"CreationDate") => ColumnKind::Created,
                    Some(b"Size") => ColumnKind::Size,
                    Some(b"CompressedSize") => ColumnKind::Packed,
                    _ => continue,
                };
                let key = String::from_utf8_lossy(key.as_bytes()).into_owned();
                let name = match self.resolve_key(&f, "N") {
                    Object::String { bytes, .. } => text_string(&bytes),
                    _ => key.clone(),
                };
                let visible = !matches!(self.resolve_key(&f, "V"), Object::Bool(false));
                let order = self.resolve_key(&f, "O").as_number().unwrap_or(f64::MAX);
                columns.push((
                    order,
                    PortfolioColumn {
                        key,
                        name,
                        kind,
                        visible,
                    },
                ));
            }
        }
        // Stable, so columns without an order keep the schema's.
        columns.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut columns: Vec<PortfolioColumn> = columns.into_iter().map(|(_, c)| c).collect();
        if !columns.iter().any(|c| c.kind == ColumnKind::Name) {
            columns.insert(
                0,
                PortfolioColumn {
                    key: String::new(),
                    name: String::new(),
                    kind: ColumnKind::Name,
                    visible: true,
                },
            );
        }
        let mut sort = Vec::new();
        if let Object::Dict(s) = self.resolve_key(&c, "Sort") {
            let keys: Vec<String> = match self.resolve_key(&s, "S") {
                Object::Name(n) => vec![String::from_utf8_lossy(n.as_bytes()).into_owned()],
                Object::Array(a) => a
                    .iter()
                    .filter_map(|o| match self.resolve(o) {
                        Object::Name(n) => Some(String::from_utf8_lossy(n.as_bytes()).into_owned()),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let up: Vec<bool> = match self.resolve_key(&s, "A") {
                Object::Bool(b) => vec![b],
                Object::Array(a) => a
                    .iter()
                    .map(|o| !matches!(self.resolve(o), Object::Bool(false)))
                    .collect(),
                _ => Vec::new(),
            };
            for (i, key) in keys.into_iter().enumerate() {
                let ascending = up.get(i).or(up.last()).copied().unwrap_or(true);
                sort.push((key, ascending));
            }
        }
        let initial = match self.resolve_key(&c, "D") {
            Object::String { bytes, .. } => Some(text_string(&bytes)),
            _ => None,
        };
        let view = match c.name("View") {
            Some(b"T") => PortfolioView::Tiles,
            Some(b"H") => PortfolioView::Hidden,
            Some(b"C") => PortfolioView::Custom,
            _ => PortfolioView::Details,
        };
        Some(Portfolio {
            columns,
            sort,
            initial,
            view,
        })
    }

    /// The bytes of attachment `a`.
    pub fn attachment_data(&self, a: &Attachment) -> Option<Vec<u8>> {
        let dict = self.resolve(&a.spec);
        let stream = self.embedded_stream(dict.as_dict()?)?;
        crate::filter::decode(&stream.dict, &stream.data).ok()
    }

    /// The embedded file stream a file specification points to.
    fn embedded_stream(&self, spec: &Dict) -> Option<Stream> {
        let Object::Dict(ef) = self.resolve_key(spec, "EF") else {
            return None;
        };
        ["UF", "F"]
            .into_iter()
            .find_map(|key| match self.resolve_key(&ef, key) {
                Object::Stream(s) => Some(s),
                _ => None,
            })
    }

    /// The document with `files` kept in it too, each its name, bytes
    /// and description. A file with the name of one already there takes
    /// its place.
    pub fn attach_files(&self, files: &[(String, Vec<u8>, String)]) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let mut entries = self.raw_attachments();
        for (name, data, description) in files {
            let name = name.trim();
            if name.is_empty() {
                return Err(NavError::EmptyName);
            }
            let mut params = Dict::new();
            params.set("Size", data.len() as i64);
            let mut ef_dict = Dict::new();
            ef_dict.set("Type", Object::name("EmbeddedFile"));
            ef_dict.set("Params", Object::Dict(params));
            let ef = up.add(Object::Stream(crate::dss::compressed_with(ef_dict, data)));
            let mut fs = Dict::new();
            fs.set("Type", Object::name("Filespec"));
            fs.set("F", text_object(name));
            fs.set("UF", text_object(name));
            if !description.is_empty() {
                fs.set("Desc", text_object(description));
            }
            fs.set(
                "EF",
                Dict(vec![
                    (crate::Name::new("F"), Object::Ref(ef)),
                    (crate::Name::new("UF"), Object::Ref(ef)),
                ]),
            );
            let spec = up.add(Object::Dict(fs));
            let key = match text_object(name) {
                Object::String { bytes, .. } => bytes,
                _ => unreachable!(),
            };
            entries.retain(|(k, _)| text_string(k) != name);
            entries.push((key, Object::Ref(spec)));
        }
        self.write_attachments(&mut up, entries)?;
        Ok(up.finish())
    }

    /// The document with attachment `a` called `name`, described by
    /// `description`.
    pub fn rename_attachment(
        &self,
        a: &Attachment,
        name: &str,
        description: &str,
    ) -> Result<Vec<u8>, NavError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NavError::EmptyName);
        }
        let mut up = self.update()?;
        let mut entries = self.raw_attachments();
        let at = entries
            .iter()
            .position(|(k, _)| *k == a.key)
            .ok_or(NavError::EmptyName)?;
        let mut fs = self.resolve(&a.spec).as_dict().cloned().unwrap_or_default();
        fs.set("F", text_object(name));
        fs.set("UF", text_object(name));
        if description.is_empty() {
            fs.remove("Desc");
        } else {
            fs.set("Desc", text_object(description));
        }
        let spec = match a.spec.as_ref() {
            Some(r) => {
                up.replace(r, Object::Dict(fs));
                Object::Ref(r)
            }
            None => Object::Ref(up.add(Object::Dict(fs))),
        };
        let key = match text_object(name) {
            Object::String { bytes, .. } => bytes,
            _ => unreachable!(),
        };
        entries[at] = (key, spec);
        self.write_attachments(&mut up, entries)?;
        Ok(up.finish())
    }

    /// The document without the attachments `names` name.
    pub fn remove_attachments(&self, names: &[String]) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let mut entries = self.raw_attachments();
        entries.retain(|(k, _)| !names.iter().any(|n| text_string(k) == *n));
        self.write_attachments(&mut up, entries)?;
        Ok(up.finish())
    }

    /// Every entry of the tree, key and file specification as stored.
    fn raw_attachments(&self) -> Vec<(Vec<u8>, Object)> {
        let mut out = Vec::new();
        let Some(catalog) = self.catalog() else {
            return out;
        };
        if let Object::Dict(names) = self.resolve_key(&catalog, "Names")
            && let Object::Dict(root) = self.resolve_key(&names, "EmbeddedFiles")
        {
            let mut seen = HashSet::new();
            self.tree_pairs_raw(&root, 0, &mut seen, &mut out);
        }
        out
    }

    /// Writes the whole `/EmbeddedFiles` tree as one node holding
    /// `entries`, sorted by key as a name tree must be.
    fn write_attachments(
        &self,
        up: &mut crate::write::Update,
        mut entries: Vec<(Vec<u8>, Object)>,
    ) -> Result<(), NavError> {
        let root = catalog_ref(self)?;
        let mut catalog = self.catalog().unwrap_or_default();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        let mut names_dict = match self.resolve_key(&catalog, "Names") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        if entries.is_empty() {
            names_dict.remove("EmbeddedFiles");
        } else {
            let mut pairs = Vec::new();
            for (k, v) in entries {
                pairs.push(Object::String {
                    bytes: k,
                    hex: false,
                });
                pairs.push(v);
            }
            let mut tree = Dict::new();
            tree.set("Names", pairs);
            names_dict.set("EmbeddedFiles", up.add(Object::Dict(tree)));
        }
        match catalog.get("Names").and_then(Object::as_ref) {
            Some(r) => up.replace(r, Object::Dict(names_dict)),
            None if names_dict.0.is_empty() => {
                catalog.remove("Names");
            }
            None => catalog.set("Names", names_dict),
        }
        up.replace(root, Object::Dict(catalog));
        Ok(())
    }
}
