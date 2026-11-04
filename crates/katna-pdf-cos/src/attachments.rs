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
    /// Its key in the tree, as stored.
    key: Vec<u8>,
    /// Its file specification, as stored: a reference or the dictionary.
    spec: Object,
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
                let modified = params
                    .as_ref()
                    .and_then(|p| match self.resolve_key(p, "ModDate") {
                        Object::String { bytes, .. } => Some(text_string(&bytes)),
                        _ => None,
                    });
                Some(Attachment {
                    name,
                    description: text("Desc").unwrap_or_default(),
                    size,
                    modified,
                    key,
                    spec,
                })
            })
            .collect()
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
