// SPDX-License-Identifier: GPL-3.0-or-later

//! Layers: the document's optional content groups (§8.11), each shown
//! or hidden by the catalog's `/OCProperties` `/D` configuration. They
//! are listed, shown and hidden, added, renamed and taken out, and a
//! page's pictures and shapes put on one by wrapping their operators in
//! `/OC /name BDC … EMC`, the name a `/Properties` resource of the page.

use crate::content::splice;
use crate::document::Document;
use crate::edit::EditError;
use crate::forms::text_object;
use crate::nav::{NavError, catalog_ref};
use crate::object::{Dict, Object, Ref};
use crate::outline::text_string;
use crate::pages::page_dict;

/// A layer of the document.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: Ref,
    pub name: String,
    /// Shown, as the document opens.
    pub on: bool,
}

/// The key of a layer's property on a page, numbered.
const LAYER_KEY: &str = "oc";

impl Document {
    /// The document's layers, in the order its configuration lists them.
    pub fn layers(&self) -> Vec<Layer> {
        if self.is_locked() {
            return Vec::new();
        }
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let Object::Dict(props) = self.resolve_key(&catalog, "OCProperties") else {
            return Vec::new();
        };
        let groups: Vec<Ref> = match self.resolve_key(&props, "OCGs") {
            Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
            _ => Vec::new(),
        };
        let config = match self.resolve_key(&props, "D") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        let refs = |key: &str| -> Vec<Ref> {
            match self.resolve_key(&config, key) {
                Object::Array(a) => a.iter().filter_map(Object::as_ref).collect(),
                _ => Vec::new(),
            }
        };
        let (on, off) = (refs("ON"), refs("OFF"));
        let off_by_default = config.name("BaseState") == Some(b"OFF");
        // The configuration's order, then any group it leaves out.
        let mut order = Vec::new();
        if let Object::Array(items) = self.resolve_key(&config, "Order") {
            self.flatten_order(&items, 0, &mut order);
        }
        for g in &groups {
            if !order.contains(g) {
                order.push(*g);
            }
        }
        order
            .into_iter()
            .filter(|g| groups.contains(g))
            .filter_map(|id| {
                let Some(Object::Dict(d)) = self.get(id) else {
                    return None;
                };
                let name = match self.resolve_key(&d, "Name") {
                    Object::String { bytes, .. } => text_string(&bytes),
                    _ => String::new(),
                };
                let shown = if off_by_default {
                    on.contains(&id)
                } else {
                    !off.contains(&id)
                };
                Some(Layer {
                    id,
                    name,
                    on: shown,
                })
            })
            .collect()
    }

    fn flatten_order(&self, items: &[Object], depth: usize, out: &mut Vec<Ref>) {
        if depth > 8 {
            return;
        }
        for item in items {
            match item {
                Object::Ref(r) => {
                    if let Some(Object::Array(a)) = self.get(*r) {
                        self.flatten_order(&a, depth + 1, out);
                    } else {
                        out.push(*r);
                    }
                }
                Object::Array(a) => self.flatten_order(a, depth + 1, out),
                _ => {}
            }
        }
    }

    /// The document with each layer in `shown` shown or hidden as said.
    pub fn show_layers(&self, shown: &[(Ref, bool)]) -> Result<Vec<u8>, NavError> {
        let layers = self.layers();
        let mut up = self.update()?;
        let (mut props, config_ref, mut config) = self.oc_dicts();
        let mut on = Vec::new();
        let mut off = Vec::new();
        for layer in &layers {
            let state = shown
                .iter()
                .find(|(id, _)| *id == layer.id)
                .map_or(layer.on, |(_, on)| *on);
            if state { &mut on } else { &mut off }.push(Object::Ref(layer.id));
        }
        config.remove("BaseState");
        config.set("ON", on);
        config.set("OFF", off);
        match config_ref {
            Some(r) => up.replace(r, Object::Dict(config)),
            None => props.set("D", config),
        }
        self.write_oc(&mut up, props)?;
        Ok(up.finish())
    }

    /// The document with a new layer called `name`, shown; and the layer.
    pub fn add_layer(&self, name: &str) -> Result<(Vec<u8>, Ref), NavError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NavError::EmptyName);
        }
        let mut up = self.update()?;
        let mut ocg = Dict::new();
        ocg.set("Type", Object::name("OCG"));
        ocg.set("Name", text_object(name));
        let id = up.add(Object::Dict(ocg));
        let (mut props, config_ref, mut config) = self.oc_dicts();
        let mut groups = match self.resolve_key(&props, "OCGs") {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        groups.push(Object::Ref(id));
        props.set("OCGs", groups);
        let mut order = match self.resolve_key(&config, "Order") {
            Object::Array(a) => a,
            _ => Vec::new(),
        };
        order.push(Object::Ref(id));
        config.set("Order", order);
        if config.name("BaseState") == Some(b"OFF") {
            let mut on = match self.resolve_key(&config, "ON") {
                Object::Array(a) => a,
                _ => Vec::new(),
            };
            on.push(Object::Ref(id));
            config.set("ON", on);
        }
        match config_ref {
            Some(r) => up.replace(r, Object::Dict(config)),
            None => props.set("D", config),
        }
        self.write_oc(&mut up, props)?;
        Ok((up.finish(), id))
    }

    /// The document with layer `id` called `name`.
    pub fn rename_layer(&self, id: Ref, name: &str) -> Result<Vec<u8>, NavError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(NavError::EmptyName);
        }
        let mut up = self.update()?;
        let mut ocg = match self.get(id) {
            Some(Object::Dict(d)) => d,
            _ => return Err(NavError::EmptyName),
        };
        ocg.set("Name", text_object(name));
        up.replace(id, Object::Dict(ocg));
        Ok(up.finish())
    }

    /// The document without layer `id`: what was on it is drawn always.
    pub fn remove_layer(&self, id: Ref) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let (mut props, config_ref, mut config) = self.oc_dicts();
        let without = |list: Object| -> Vec<Object> {
            match self.resolve(&list) {
                Object::Array(a) => a.into_iter().filter(|o| o.as_ref() != Some(id)).collect(),
                _ => Vec::new(),
            }
        };
        if let Some(groups) = props.get("OCGs").cloned() {
            props.set("OCGs", without(groups));
        }
        for key in ["ON", "OFF", "Order"] {
            if let Some(list) = config.get(key).cloned() {
                config.set(key, without(list));
            }
        }
        match config_ref {
            Some(r) => up.replace(r, Object::Dict(config)),
            None => props.set("D", config),
        }
        self.write_oc(&mut up, props)?;
        Ok(up.finish())
    }

    /// The `/OCProperties` dictionary (new when there is none) and its
    /// `/D` configuration, with the reference the configuration is
    /// stored at when it is its own object.
    fn oc_dicts(&self) -> (Dict, Option<Ref>, Dict) {
        let catalog = self.catalog().unwrap_or_default();
        let props = match self.resolve_key(&catalog, "OCProperties") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        let config_ref = props.get("D").and_then(Object::as_ref);
        let config = match self.resolve_key(&props, "D") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        (props, config_ref, config)
    }

    /// Writes `props` as the catalog's `/OCProperties`, with every
    /// group listed and a configuration present.
    fn write_oc(&self, up: &mut crate::write::Update, mut props: Dict) -> Result<(), NavError> {
        let root = catalog_ref(self)?;
        let mut catalog = self.catalog().unwrap_or_default();
        if props.get("OCGs").is_none() {
            props.set("OCGs", Vec::<Object>::new());
        }
        if props.get("D").is_none() {
            props.set("D", Dict::new());
        }
        match catalog.get("OCProperties").and_then(Object::as_ref) {
            Some(r) => up.replace(r, Object::Dict(props)),
            None => catalog.set("OCProperties", props),
        }
        up.replace(root, Object::Dict(catalog));
        Ok(())
    }

    /// The names of page `page`'s `/Properties` resources that stand for
    /// a layer, with the layer.
    pub(crate) fn layer_properties(&self, page: usize) -> Vec<(Vec<u8>, Ref)> {
        let Some(page_ref) = self.pages().get(page).copied() else {
            return Vec::new();
        };
        let dict = page_dict(self, page_ref);
        let resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => return Vec::new(),
        };
        let Object::Dict(properties) = self.resolve_key(&resources, "Properties") else {
            return Vec::new();
        };
        properties
            .iter()
            .filter_map(|(name, value)| {
                let r = value.as_ref()?;
                let d = match self.get(r) {
                    Some(Object::Dict(d)) => d,
                    _ => return None,
                };
                // A membership dictionary of one group stands for it.
                let id = match d.name("Type") {
                    Some(b"OCMD") => match self.resolve_key(&d, "OCGs") {
                        Object::Array(a) if a.len() == 1 => a[0].as_ref()?,
                        o => o
                            .as_ref()
                            .or_else(|| d.get("OCGs").and_then(Object::as_ref))?,
                    },
                    _ => r,
                };
                Some((name.as_bytes().to_vec(), id))
            })
            .collect()
    }

    /// The file with objects `indices` of `page_objects(page)` on layer
    /// `layer`, or on none.
    pub fn put_on_layer(
        &self,
        page: usize,
        indices: &[usize],
        layer: Option<Ref>,
    ) -> Result<Vec<u8>, EditError> {
        let objects = self.page_objects(page);
        let read = self.page_content(page).ok_or(EditError::Content)?;
        let ops = &read.ops;
        let page_ref = *self.pages().get(page).ok_or(EditError::NoPage)?;
        let dict = page_dict(self, page_ref);
        let resources_ref = dict.get("Resources").and_then(Object::as_ref);
        let mut resources = match dict.get("Resources").map(|r| self.resolve(r)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let mut properties = match self.resolve_key(&resources, "Properties") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        let key = layer.map(|id| {
            let found = properties
                .iter()
                .find(|(_, v)| v.as_ref() == Some(id))
                .map(|(k, _)| k.as_bytes().to_vec());
            match found {
                Some(k) => k,
                None => {
                    let mut n = 1;
                    let key = loop {
                        let key = format!("{LAYER_KEY}{n}");
                        if properties.get(&key).is_none() {
                            break key;
                        }
                        n += 1;
                    };
                    properties.set(&key, Object::Ref(id));
                    key.into_bytes()
                }
            }
        });
        let mut indices: Vec<usize> = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        let mut edits = Vec::new();
        for index in indices {
            let object = objects.get(index).ok_or(EditError::NoRun)?;
            let (mut a, mut b) = (*object.ops.start(), *object.ops.end());
            // A move wrapped the object in `q … cm … Q`: the layer goes
            // around that.
            while a >= 2
                && ops[a - 2].is("q")
                && ops[a - 1].is("cm")
                && ops.get(b + 1).is_some_and(|op| op.is("Q"))
            {
                a -= 2;
                b += 1;
            }
            let wrapped = a >= 1
                && ops[a - 1].is("BDC")
                && ops[a - 1].operands.first().and_then(Object::as_name) == Some(b"OC")
                && ops.get(b + 1).is_some_and(|op| op.is("EMC"));
            let before = match &key {
                Some(k) => format!("/OC /{} BDC ", String::from_utf8_lossy(k)).into_bytes(),
                None => Vec::new(),
            };
            let after = if key.is_some() {
                b" EMC".to_vec()
            } else {
                Vec::new()
            };
            if wrapped {
                edits.push((ops[a - 1].span.clone(), before));
                edits.push((ops[b + 1].span.clone(), after));
            } else if key.is_some() {
                let first = ops[a].span.start;
                let last = ops[b].span.end;
                edits.push((first..first, before));
                edits.push((last..last, after));
            }
        }
        let content = splice(&read.content, edits);
        let mut update = self.update()?;
        let stream = update.add(Object::Stream(crate::dss::compressed(&content)));
        let mut page_dict = self
            .get(page_ref)
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default();
        page_dict.set("Contents", Object::Ref(stream));
        if !properties.0.is_empty() {
            resources.set("Properties", properties);
        }
        match resources_ref {
            Some(r) => update.replace(r, Object::Dict(resources)),
            None => page_dict.set("Resources", resources),
        }
        update.replace(page_ref, Object::Dict(page_dict));
        Ok(update.finish())
    }
}
