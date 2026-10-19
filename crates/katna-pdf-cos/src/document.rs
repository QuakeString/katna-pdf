// SPDX-License-Identifier: GPL-3.0-or-later

//! A PDF file's object table: cross-reference tables and streams, object
//! streams, and repair when they are broken (ISO 32000-2 §7.5).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::crypt::{Crypt, Permissions, UnlockError};
use crate::filter::decode;
use crate::object::{Dict, Object, Ref};
use crate::parse::{Parser, find, is_white, rfind};

/// Where an object lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Free,
    /// At a byte offset in the file.
    InFile {
        offset: usize,
        generation: u16,
    },
    /// Inside object stream `stream`, at position `index`.
    InStream {
        stream: u32,
        index: u32,
    },
}

/// How the newest cross-reference section is written; an update writes
/// its own section the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefKind {
    Table,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// No `%PDF-` header and no objects found.
    NotPdf,
    /// No document catalog, even after repair.
    NoCatalog,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LoadError::NotPdf => "not a PDF file",
            LoadError::NoCatalog => "the PDF has no document catalog",
        })
    }
}

impl std::error::Error for LoadError {}

/// An object stream's contents: object numbers and objects.
type StreamObjects = Vec<(u32, Object)>;

/// Longest `/Prev` chain followed; real files have a handful.
const MAX_SECTIONS: usize = 4096;
/// Deepest page tree walked.
const MAX_PAGE_DEPTH: usize = 64;

pub struct Document {
    data: Vec<u8>,
    entries: BTreeMap<u32, Entry>,
    trailer: Dict,
    startxref: usize,
    kind: XrefKind,
    repaired: bool,
    /// Every `N G obj` in the file, by object number (last one wins);
    /// built on first need.
    scanned: OnceLock<HashMap<u32, (usize, u16)>>,
    /// Decoded object streams.
    object_streams: Mutex<HashMap<u32, Arc<StreamObjects>>>,
    /// The key to a locked file, once opened.
    crypt: Option<Crypt>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("bytes", &self.data.len())
            .field("objects", &self.entries.len())
            .field("kind", &self.kind)
            .field("repaired", &self.repaired)
            .finish()
    }
}

impl Document {
    pub fn load(data: Vec<u8>) -> Result<Self, LoadError> {
        let mut doc = Document {
            data,
            entries: BTreeMap::new(),
            trailer: Dict::new(),
            startxref: 0,
            kind: XrefKind::Table,
            repaired: false,
            scanned: OnceLock::new(),
            object_streams: Mutex::new(HashMap::new()),
            crypt: None,
        };
        if !doc.read_xref_chain() || !doc.catalog_ok() {
            doc.repair()?;
        }
        // Most locked files open for anyone, their password empty.
        if doc.is_encrypted() {
            let _ = doc.unlock("");
        }
        Ok(doc)
    }

    /// Opens a locked file with `password`, its owner's or its user's.
    pub fn unlock(&mut self, password: &str) -> Result<(), UnlockError> {
        let Some(encrypt) = self.trailer.get("Encrypt").cloned() else {
            return Ok(());
        };
        let dict_ref = encrypt.as_ref();
        let Object::Dict(mut dict) = self.resolve(&encrypt) else {
            return Err(UnlockError::Unsupported);
        };
        // Its crypt filters may be objects of their own.
        if let Object::Dict(filters) = self.resolve_key(&dict, "CF") {
            let filters = filters
                .iter()
                .map(|(k, v)| (k.clone(), self.resolve(v)))
                .collect();
            dict.set("CF", Object::Dict(Dict(filters)));
        }
        let id = match self.trailer.get("ID").map(|i| self.resolve(i)) {
            Some(Object::Array(a)) => match a.first() {
                Some(Object::String { bytes, .. }) => bytes.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let crypt = Crypt::unlock(&dict, dict_ref, &id, password)?;
        self.crypt = Some(crypt);
        // Object streams read before were still locked.
        if let Ok(mut cache) = self.object_streams.lock() {
            cache.clear();
        }
        Ok(())
    }

    /// Whether the file is locked and not yet opened with its password.
    pub fn is_locked(&self) -> bool {
        self.is_encrypted() && self.crypt.is_none()
    }

    /// Whether the file was opened with its owner password, or has no lock
    /// or no owner password: its lock may be changed or taken off.
    pub fn opened_as_owner(&self) -> bool {
        !self.is_encrypted() || self.crypt.as_ref().is_some_and(|c| c.owner)
    }

    /// What the file's lock allows whoever opened it.
    pub fn permissions(&self) -> Permissions {
        match &self.crypt {
            Some(crypt) => crypt.permissions,
            None => Permissions::ALL,
        }
    }

    pub(crate) fn crypt(&self) -> Option<&Crypt> {
        self.crypt.as_ref()
    }

    /// The file as loaded.
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    /// Offset of the newest cross-reference section (an update's `/Prev`).
    pub fn startxref(&self) -> usize {
        self.startxref
    }

    pub fn xref_kind(&self) -> XrefKind {
        self.kind
    }

    /// Whether the cross-reference data was broken and rebuilt by scanning.
    pub fn was_repaired(&self) -> bool {
        self.repaired
    }

    pub fn is_encrypted(&self) -> bool {
        self.trailer.get("Encrypt").is_some()
    }

    /// One more than the highest object number in use (the trailer's
    /// `/Size`, or larger if the table says so).
    pub fn size(&self) -> u32 {
        let from_table = self.entries.keys().next_back().map_or(1, |n| n + 1);
        let declared = self
            .trailer
            .int("Size")
            .and_then(|s| u32::try_from(s).ok())
            .unwrap_or(0);
        from_table.max(declared)
    }

    pub fn entry(&self, num: u32) -> Option<Entry> {
        self.entries.get(&num).copied()
    }

    pub fn object_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries
            .iter()
            .filter(|(_, e)| !matches!(e, Entry::Free))
            .map(|(n, _)| *n)
    }

    /// The object `r` names, or `None` if it is free or unreadable
    /// (which PDF treats as `null`).
    pub fn get(&self, r: Ref) -> Option<Object> {
        match self.entries.get(&r.num).copied() {
            Some(Entry::InFile { offset, generation }) => self
                .object_at(offset, Some(Ref::new(r.num, generation)))
                .or_else(|| self.scanned_object(r.num)),
            Some(Entry::InStream { stream, index }) => self.stream_object(stream, index, r.num),
            Some(Entry::Free) => None,
            None => self.scanned_object(r.num),
        }
    }

    /// `obj` itself, or what it refers to (following chains of
    /// references, up to a limit).
    pub fn resolve(&self, obj: &Object) -> Object {
        let mut cur = obj.clone();
        for _ in 0..32 {
            match cur {
                Object::Ref(r) => cur = self.get(r).unwrap_or(Object::Null),
                other => return other,
            }
        }
        Object::Null
    }

    /// A dictionary entry, resolved.
    pub fn resolve_key(&self, dict: &Dict, key: &str) -> Object {
        dict.get(key).map_or(Object::Null, |o| self.resolve(o))
    }

    pub fn catalog(&self) -> Option<Dict> {
        match self.resolve_key(&self.trailer, "Root") {
            Object::Dict(d) => Some(d),
            _ => None,
        }
    }

    /// The pages in order, by walking the page tree. Loops and impossible
    /// depths are cut off. When the tree is missing or empty, every
    /// `/Type /Page` object is a page, in object-number order, as other
    /// readers recover such files.
    pub fn pages(&self) -> Vec<Ref> {
        let mut out = Vec::new();
        let root = self
            .catalog()
            .and_then(|c| c.get("Pages").and_then(Object::as_ref));
        if let Some(root) = root {
            let mut seen = HashSet::new();
            self.walk_pages(root, 0, &mut seen, &mut out);
        }
        if out.is_empty() && !self.is_locked() {
            out = self
                .object_numbers()
                .filter_map(|num| {
                    let generation = match self.entries.get(&num) {
                        Some(Entry::InFile { generation, .. }) => *generation,
                        _ => 0,
                    };
                    let r = Ref::new(num, generation);
                    let page = self.get(r)?;
                    (page.as_dict()?.name("Type") == Some(b"Page")).then_some(r)
                })
                .collect();
        }
        out
    }

    fn walk_pages(&self, node: Ref, depth: usize, seen: &mut HashSet<Ref>, out: &mut Vec<Ref>) {
        if depth > MAX_PAGE_DEPTH || !seen.insert(node) {
            return;
        }
        let Some(dict) = self.get(node).and_then(|o| o.as_dict().cloned()) else {
            return;
        };
        let is_tree = dict.name("Type") == Some(b"Pages") || dict.get("Kids").is_some();
        if !is_tree || dict.name("Type") == Some(b"Page") {
            out.push(node);
            return;
        }
        if let Object::Array(kids) = self.resolve_key(&dict, "Kids") {
            for kid in kids.iter().filter_map(Object::as_ref) {
                self.walk_pages(kid, depth + 1, seen, out);
            }
        }
    }

    // ----- reading -----

    /// The indirect object whose header is at `offset`. With `expect`, the
    /// header must name that object.
    fn object_at(&self, offset: usize, expect: Option<Ref>) -> Option<Object> {
        let mut p = Parser::new(&self.data, offset);
        let header = p.object_header()?;
        if expect.is_some_and(|e| e.num != header.num) {
            return None;
        }
        let obj = p.indirect_body(&|len| self.length(len))?;
        Some(match &self.crypt {
            Some(crypt) => crypt.decrypt(header, obj),
            None => obj,
        })
    }

    fn length(&self, len: &Object) -> Option<usize> {
        match len {
            Object::Int(i) => usize::try_from(*i).ok(),
            Object::Ref(r) => {
                // An indirect /Length must not itself be a stream.
                let entry = self.entries.get(&r.num).copied();
                let obj = match entry {
                    Some(Entry::InFile { offset, .. }) => {
                        let mut p = Parser::new(&self.data, offset);
                        p.object_header()?;
                        p.object(0)
                    }
                    _ => None,
                }?;
                obj.as_int().and_then(|i| usize::try_from(i).ok())
            }
            _ => None,
        }
    }

    fn scanned_object(&self, num: u32) -> Option<Object> {
        let (offset, generation) = *self.scan().get(&num)?;
        self.object_at(offset, Some(Ref::new(num, generation)))
    }

    fn stream_object(&self, stream: u32, index: u32, num: u32) -> Option<Object> {
        let objects = self.object_stream(stream)?;
        // The index is a hint; trust the object number when they differ.
        match objects.get(index as usize) {
            Some((n, o)) if *n == num => Some(o.clone()),
            _ => objects
                .iter()
                .find(|(n, _)| *n == num)
                .map(|(_, o)| o.clone()),
        }
    }

    fn object_stream(&self, stream: u32) -> Option<Arc<StreamObjects>> {
        if let Some(objects) = self.object_streams.lock().ok()?.get(&stream) {
            return Some(objects.clone());
        }
        let Some(Entry::InFile { offset, generation }) = self.entries.get(&stream).copied() else {
            return None;
        };
        let Object::Stream(s) = self.object_at(offset, Some(Ref::new(stream, generation)))? else {
            return None;
        };
        let data = decode(&s.dict, &s.data).ok()?;
        let n = s.dict.int("N")?.clamp(0, 1 << 20) as usize;
        let first = usize::try_from(s.dict.int("First")?).ok()?;
        let mut header = Parser::new(&data, 0);
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            let (Some(num), Some(off)) = (header.uint(), header.uint()) else {
                break;
            };
            slots.push((u32::try_from(num).ok()?, usize::try_from(off).ok()?));
        }
        let objects: Vec<(u32, Object)> = slots
            .iter()
            .map(|&(num, off)| {
                let mut p = Parser::new(&data, first.saturating_add(off));
                (num, p.object(0).unwrap_or(Object::Null))
            })
            .collect();
        let objects = Arc::new(objects);
        if let Ok(mut cache) = self.object_streams.lock() {
            cache.insert(stream, objects.clone());
        }
        Some(objects)
    }

    // ----- cross-reference sections -----

    /// Reads the newest section and every older one through `/Prev`.
    /// False when the chain is unusable.
    fn read_xref_chain(&mut self) -> bool {
        let Some(start) = self.find_startxref() else {
            return false;
        };
        self.startxref = start;
        let mut next = Some(start);
        let mut seen = HashSet::new();
        let mut first = true;
        while let Some(offset) = next.take() {
            if seen.len() >= MAX_SECTIONS || !seen.insert(offset) {
                break;
            }
            let Some((trailer, kind)) = self.read_section(offset) else {
                if first {
                    return false;
                }
                break;
            };
            if first {
                self.kind = kind;
                self.trailer = trailer.clone();
                first = false;
            } else {
                // Older trailers fill in what newer ones leave out.
                for (k, v) in trailer.iter() {
                    if self
                        .trailer
                        .get(std::str::from_utf8(k.as_bytes()).unwrap_or(""))
                        .is_none()
                    {
                        self.trailer.0.push((k.clone(), v.clone()));
                    }
                }
            }
            // A hybrid file's table points at an xref stream as well.
            if let Some(stm) = trailer.int("XRefStm").and_then(|o| usize::try_from(o).ok()) {
                self.read_section(stm);
            }
            next = trailer.int("Prev").and_then(|o| usize::try_from(o).ok());
        }
        // Keep the trailer to what it should hold; drop stream keys.
        for key in [
            "Length",
            "Filter",
            "DecodeParms",
            "W",
            "Index",
            "Type",
            "XRefStm",
            "Prev",
        ] {
            self.trailer.remove(key);
        }
        !self.entries.is_empty()
    }

    fn find_startxref(&self) -> Option<usize> {
        let tail_start = self.data.len().saturating_sub(4096);
        let at = rfind(&self.data[tail_start..], b"startxref")? + tail_start;
        let mut p = Parser::new(&self.data, at + b"startxref".len());
        usize::try_from(p.uint()?)
            .ok()
            .filter(|o| *o < self.data.len())
    }

    /// One section at `offset`: its entries go into `self.entries` where
    /// no newer section set them. Returns its trailer.
    fn read_section(&mut self, offset: usize) -> Option<(Dict, XrefKind)> {
        let mut p = Parser::new(&self.data, offset);
        if p.eat(b"xref") {
            let pos = p.pos;
            return self.read_table(pos).map(|t| (t, XrefKind::Table));
        }
        // Some writers' offsets are a little off: look nearby for "xref".
        let near = &self.data[offset.saturating_sub(16)..(offset + 16).min(self.data.len())];
        if let Some(i) = find(near, b"xref")
            && !near[i.saturating_sub(1)..i].contains(&b't')
        {
            let mut p = Parser::new(&self.data, offset.saturating_sub(16) + i);
            if p.eat(b"xref") {
                let pos = p.pos;
                return self.read_table(pos).map(|t| (t, XrefKind::Table));
            }
        }
        let Object::Stream(s) = self.object_at(offset, None)? else {
            return None;
        };
        if s.dict.name("Type") != Some(b"XRef") {
            return None;
        }
        self.read_stream_entries(&s.dict, &s.data)?;
        Some((s.dict, XrefKind::Stream))
    }

    fn read_table(&mut self, pos: usize) -> Option<Dict> {
        let (trailer, entries) = parse_table(&self.data, pos)?;
        for (num, entry) in entries {
            self.entries.entry(num).or_insert(entry);
        }
        Some(trailer)
    }

    fn read_stream_entries(&mut self, dict: &Dict, raw: &[u8]) -> Option<()> {
        let data = decode(dict, raw).ok()?;
        let w: Vec<usize> = dict
            .get("W")?
            .as_array()?
            .iter()
            .map(|o| o.as_int().map_or(0, |i| i.clamp(0, 8) as usize))
            .collect();
        if w.len() != 3 {
            return None;
        }
        let size = u32::try_from(dict.int("Size")?).ok()?;
        let index: Vec<u32> = match dict.get("Index") {
            Some(Object::Array(a)) => a
                .iter()
                .filter_map(|o| o.as_int().and_then(|i| u32::try_from(i).ok()))
                .collect(),
            _ => vec![0, size],
        };
        let row = w.iter().sum::<usize>();
        if row == 0 {
            return None;
        }
        let mut rows = data.chunks_exact(row);
        for pair in index.as_chunks::<2>().0 {
            for num in pair[0]..pair[0].saturating_add(pair[1]) {
                let Some(r) = rows.next() else {
                    return Some(());
                };
                let field = |i: usize, default: u64| -> u64 {
                    let start: usize = w[..i].iter().sum();
                    if w[i] == 0 {
                        return default;
                    }
                    r[start..start + w[i]]
                        .iter()
                        .fold(0u64, |acc, b| acc << 8 | u64::from(*b))
                };
                let entry = match field(0, 1) {
                    0 => Entry::Free,
                    1 => Entry::InFile {
                        offset: usize::try_from(field(1, 0)).ok()?,
                        generation: u16::try_from(field(2, 0)).unwrap_or(0),
                    },
                    2 => Entry::InStream {
                        stream: u32::try_from(field(1, 0)).ok()?,
                        index: u32::try_from(field(2, 0)).ok()?,
                    },
                    // Unknown types are to be treated as null objects.
                    _ => Entry::Free,
                };
                self.entries.entry(num).or_insert(entry);
            }
        }
        Some(())
    }

    // ----- repair -----

    fn catalog_ok(&self) -> bool {
        self.catalog().is_some_and(|c| c.get("Pages").is_some())
    }

    /// Every `N G obj` at the start of a line, by object number.
    fn scan(&self) -> &HashMap<u32, (usize, u16)> {
        self.scanned.get_or_init(|| {
            let mut found = HashMap::new();
            let d = &self.data;
            let mut i = 0;
            while let Some(at) = find(&d[i..], b"obj") {
                let pos = i + at;
                i = pos + 3;
                // Walk back over "N G " to the start of the header.
                let mut s = pos;
                let mut fields = 0;
                while fields < 2 && s > 0 {
                    while s > 0 && is_white(d[s - 1]) {
                        s -= 1;
                    }
                    let end = s;
                    while s > 0 && d[s - 1].is_ascii_digit() {
                        s -= 1;
                    }
                    if s == end {
                        break;
                    }
                    fields += 1;
                }
                if fields != 2 {
                    continue;
                }
                let mut p = Parser::new(d, s);
                if let Some(r) = p.object_header()
                    && p.pos == pos + 3
                {
                    found.insert(r.num, (s, r.generation));
                }
            }
            found
        })
    }

    /// Rebuilds the table from a scan of the whole file, keeping what the
    /// broken table had for objects in object streams.
    fn repair(&mut self) -> Result<(), LoadError> {
        self.repaired = true;
        let scanned = self.scan().clone();
        if scanned.is_empty() {
            return Err(LoadError::NotPdf);
        }
        let in_streams: Vec<(u32, Entry)> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e, Entry::InStream { .. }))
            .map(|(n, e)| (*n, *e))
            .collect();
        self.entries.clear();
        for (num, (offset, generation)) in &scanned {
            self.entries.insert(
                *num,
                Entry::InFile {
                    offset: *offset,
                    generation: *generation,
                },
            );
        }
        // Objects inside object streams the scan cannot see.
        for (num, entry) in in_streams {
            self.entries.entry(num).or_insert(entry);
        }
        let mut stream_objects = Vec::new();
        for (&num, &(offset, generation)) in &scanned {
            if let Some(Object::Stream(s)) = self.object_at(offset, Some(Ref::new(num, generation)))
                && s.dict.name("Type") == Some(b"ObjStm")
            {
                stream_objects.push(num);
            }
        }
        for stream in stream_objects {
            if let Some(objects) = self.object_stream(stream) {
                for (i, (num, _)) in objects.iter().enumerate() {
                    self.entries.entry(*num).or_insert(Entry::InStream {
                        stream,
                        index: i as u32,
                    });
                }
            }
        }
        // A trailer: the last `trailer` dictionary, else the newest xref
        // stream's dictionary, else a catalog found by type.
        if !self.catalog_ok() {
            let mut trailer = None;
            let mut from = self.data.len();
            while let Some(at) = rfind(&self.data[..from], b"trailer") {
                let mut p = Parser::new(&self.data, at + 7);
                if let Some(Object::Dict(d)) = p.object(0)
                    && d.get("Root").is_some()
                {
                    trailer = Some(d);
                    break;
                }
                from = at;
            }
            if trailer.is_none() {
                let mut nums: Vec<_> = scanned.keys().copied().collect();
                nums.sort_unstable();
                for &num in nums.iter().rev() {
                    if let Some(obj) = self.get(Ref::new(num, scanned[&num].1))
                        && let Some(d) = obj.as_dict()
                        && d.name("Type") == Some(b"XRef")
                        && d.get("Root").is_some()
                    {
                        trailer = Some(d.clone());
                        break;
                    }
                }
            }
            let mut trailer = trailer.unwrap_or_default();
            if !matches!(self.resolve_key(&trailer, "Root"), Object::Dict(_)) {
                let catalog = self
                    .object_numbers()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .find(|&n| {
                        self.get(Ref::new(n, 0))
                            .and_then(|o| o.as_dict().map(|d| d.name("Type") == Some(b"Catalog")))
                            .unwrap_or(false)
                    });
                let Some(num) = catalog else {
                    return Err(LoadError::NoCatalog);
                };
                let generation = match self.entries.get(&num) {
                    Some(Entry::InFile { generation, .. }) => *generation,
                    _ => 0,
                };
                trailer.set("Root", Ref::new(num, generation));
            }
            for key in [
                "Length",
                "Filter",
                "DecodeParms",
                "W",
                "Index",
                "Type",
                "XRefStm",
                "Prev",
            ] {
                trailer.remove(key);
            }
            self.trailer = trailer;
        }
        if self.startxref == 0 {
            self.startxref = self.find_startxref().unwrap_or(0);
        }
        if self.catalog().is_none() {
            return Err(LoadError::NoCatalog);
        }
        Ok(())
    }
}

/// A classic table at `pos` (after `xref`): its trailer and entries.
fn parse_table(data: &[u8], pos: usize) -> Option<(Dict, Vec<(u32, Entry)>)> {
    let mut p = Parser::new(data, pos);
    let mut entries = Vec::new();
    loop {
        let saved = p.pos;
        if p.eat(b"trailer") {
            return match p.object(0) {
                Some(Object::Dict(d)) => Some((d, entries)),
                _ => None,
            };
        }
        p.pos = saved;
        let (Some(start), Some(count)) = (p.uint(), p.uint()) else {
            return None;
        };
        let start = u32::try_from(start).ok()?;
        for i in 0..count.min(1 << 24) {
            // "nnnnnnnnnn ggggg n" — read tokens, as line endings vary.
            let (Some(off), Some(generation)) = (p.uint(), p.uint()) else {
                return None;
            };
            let kind = p.token();
            let Some(num) = start.checked_add(i as u32) else {
                break;
            };
            // Object 0 is the head of the free list; a few files
            // number their table from 1 by mistake.
            let entry = match kind {
                b"n" if off > 0 => Entry::InFile {
                    offset: usize::try_from(off).ok()?,
                    generation: u16::try_from(generation).unwrap_or(0),
                },
                b"n" | b"f" => Entry::Free,
                _ => return None,
            };
            entries.push((num, entry));
        }
    }
}
