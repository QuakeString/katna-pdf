// SPDX-License-Identifier: GPL-3.0-or-later

//! Certified files (ISO 32000-2 §12.8.2.2, DocMDP): the first signature
//! of a file may certify it, saying which changes it still allows, and
//! what was changed after a revision, sorted into the kinds of change
//! certification speaks of.

use std::collections::{BTreeSet, HashMap};

use crate::object::{Dict, Object, Ref};
use crate::{Document, Name};

/// Which changes a certified file allows (`/P` of its DocMDP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allowed {
    /// None.
    Nothing,
    /// Filling in the form, and signing.
    FormFilling,
    /// Those, and adding, changing and removing comments.
    Comments,
}

impl Allowed {
    /// `/P`.
    pub fn level(self) -> i64 {
        match self {
            Allowed::Nothing => 1,
            Allowed::FormFilling => 2,
            Allowed::Comments => 3,
        }
    }

    /// From `/P`, which is 2 when missing or unknown.
    pub fn from_level(p: i64) -> Self {
        match p {
            1 => Allowed::Nothing,
            3 => Allowed::Comments,
            _ => Allowed::FormFilling,
        }
    }

    /// Whether a change of kind `change` is allowed. Long-term
    /// validation data is, whatever the level (ISO 32000-2 §12.8.4.3).
    pub fn permits(self, change: Change) -> bool {
        match change {
            Change::Dss => true,
            Change::FormFilled | Change::Signed => self != Allowed::Nothing,
            Change::Commented => self == Allowed::Comments,
            Change::NewField | Change::Pages | Change::Content | Change::Other => false,
        }
    }
}

/// A kind of change made to a file in a later revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Change {
    /// A form field filled in.
    FormFilled,
    /// A signature added.
    Signed,
    /// A comment added, changed or removed.
    Commented,
    /// A signature field added (Acrobat and pyHanko allow signing a
    /// certified file's own fields only).
    NewField,
    /// Long-term validation data (certificates, OCSP answers, CRLs).
    Dss,
    /// Pages added, removed, moved, rotated or resized.
    Pages,
    /// What a page shows (its content).
    Content,
    /// Anything else.
    Other,
}

impl Document {
    /// How the file is certified: what its certification signature
    /// allows, if it has one.
    pub fn certification(&self) -> Option<Allowed> {
        let catalog = self.catalog()?;
        let Object::Dict(perms) = self.resolve_key(&catalog, "Perms") else {
            return None;
        };
        match self.resolve_key(&perms, "DocMDP") {
            Object::Dict(sig) => self.docmdp(&sig),
            _ => None,
        }
    }

    /// What signature dictionary `sig` allows, if it certifies.
    pub(crate) fn docmdp(&self, sig: &Dict) -> Option<Allowed> {
        let Object::Array(refs) = self.resolve_key(sig, "Reference") else {
            return None;
        };
        refs.iter().find_map(|r| {
            let Object::Dict(r) = self.resolve(r) else {
                return None;
            };
            if r.name("TransformMethod") != Some(b"DocMDP") {
                return None;
            }
            let p = match self.resolve_key(&r, "TransformParams") {
                Object::Dict(params) => self.resolve_key(&params, "P").as_int(),
                _ => None,
            };
            Some(Allowed::from_level(p.unwrap_or(2)))
        })
    }

    /// The kinds of change made since the revision of the file that
    /// ended at byte `end`.
    pub fn changes_since(&self, end: usize) -> BTreeSet<Change> {
        let mut out = BTreeSet::new();
        let Some(Ok(old)) = self.bytes().get(..end).map(|b| Document::load(b.to_vec())) else {
            out.insert(Change::Other);
            return out;
        };
        let appearances = self.appearances();
        let pages: BTreeSet<u32> = self.pages().iter().map(|p| p.num).collect();
        for num in self.object_numbers() {
            let r = Ref::new(num, 0);
            let (Some(new), Some(before)) = (self.get(r), old.get(r)) else {
                // Objects added count through those that point to them;
                // objects removed, through those that pointed to them.
                continue;
            };
            if new == before {
                continue;
            }
            let dict = match &new {
                Object::Dict(d) => Some(d),
                Object::Stream(s) => Some(&s.dict),
                _ => None,
            };
            let kind = dict.and_then(|d| d.name("Type"));
            if matches!(kind, Some(b"XRef" | b"ObjStm")) {
                continue;
            }
            if let Some(&change) = appearances.get(&num) {
                out.insert(change);
                continue;
            }
            match (&new, &before) {
                (Object::Dict(new), Object::Dict(before)) => {
                    if kind == Some(b"Catalog") || num == self.root_num() {
                        for key in changed_keys(new, before) {
                            match key.as_bytes() {
                                b"DSS" | b"Extensions" => {
                                    out.insert(Change::Dss);
                                }
                                b"AcroForm" => {
                                    let form = |doc: &Document, d: &Dict| match doc
                                        .resolve_key(d, "AcroForm")
                                    {
                                        Object::Dict(f) => f,
                                        _ => Dict::new(),
                                    };
                                    out.extend(self.form_changes(
                                        &old,
                                        &form(self, new),
                                        &form(&old, before),
                                    ));
                                }
                                _ => {
                                    out.insert(Change::Other);
                                }
                            }
                        }
                    } else if kind == Some(b"Page") || pages.contains(&num) {
                        for key in changed_keys(new, before) {
                            match key.as_bytes() {
                                b"Annots" => {
                                    let list = |doc: &Document, d: &Dict| match doc
                                        .resolve_key(d, "Annots")
                                    {
                                        Object::Array(a) => a,
                                        _ => Vec::new(),
                                    };
                                    out.extend(self.list_changes(
                                        &old,
                                        &list(self, new),
                                        &list(&old, before),
                                    ));
                                }
                                b"Contents" | b"Resources" => {
                                    out.insert(Change::Content);
                                }
                                _ => {
                                    out.insert(Change::Pages);
                                }
                            }
                        }
                    } else if kind == Some(b"Pages") {
                        out.insert(Change::Pages);
                    } else if new.get("Fields").is_some() {
                        out.extend(self.form_changes(&old, new, before));
                    } else {
                        out.insert(self.kind_of(new));
                    }
                }
                (Object::Array(new), Object::Array(before)) => {
                    out.extend(self.list_changes(&old, new, before));
                }
                (Object::Stream(_), _) => {
                    out.insert(Change::Content);
                }
                _ => {
                    out.insert(Change::Other);
                }
            }
        }
        out
    }

    /// The changes the form dictionary went through, from `before` (in
    /// `old`) to `new`: by the fields added and removed. Its flags only
    /// say that signatures are present.
    fn form_changes(&self, old: &Document, new: &Dict, before: &Dict) -> Vec<Change> {
        let mut out = Vec::new();
        for key in changed_keys(new, before) {
            match key.as_bytes() {
                b"Fields" => {
                    let list = |doc: &Document, d: &Dict| match doc.resolve_key(d, "Fields") {
                        Object::Array(a) => a,
                        _ => Vec::new(),
                    };
                    out.extend(self.list_changes(old, &list(self, new), &list(old, before)));
                }
                b"SigFlags" => {}
                _ => out.push(Change::FormFilled),
            }
        }
        out
    }

    fn root_num(&self) -> u32 {
        self.trailer()
            .get("Root")
            .and_then(Object::as_ref)
            .map_or(0, |r| r.num)
    }

    /// The changes a list of annotations or fields went through, from
    /// `before` (in `old`) to `new`: by what was added and removed. Of
    /// fields, only comments come and go; a signature field added is a
    /// new field, not a field signed.
    fn list_changes(&self, old: &Document, new: &[Object], before: &[Object]) -> Vec<Change> {
        let kind = |doc: &Document, o: &Object| match doc.resolve(o) {
            Object::Dict(d) => doc.kind_of(&d),
            // A page's content streams, or anything else.
            _ => Change::Other,
        };
        let added = new
            .iter()
            .filter(|o| !before.contains(o))
            .map(|o| match kind(self, o) {
                Change::Signed => Change::NewField,
                Change::FormFilled => Change::Other,
                k => k,
            });
        let removed = before
            .iter()
            .filter(|o| !new.contains(o))
            .map(|o| match kind(old, o) {
                Change::Commented => Change::Commented,
                _ => Change::Other,
            });
        added.chain(removed).collect()
    }

    /// The change that an annotation, field, signature or form
    /// dictionary `dict` being added or changed is.
    fn kind_of(&self, dict: &Dict) -> Change {
        if dict.name("Type") == Some(b"Sig") {
            return Change::Signed;
        }
        if dict.get("Fields").is_some() {
            return Change::FormFilled;
        }
        if ["Certs", "OCSPs", "CRLs", "VRI"]
            .iter()
            .any(|k| dict.get(k).is_some())
        {
            return Change::Dss;
        }
        // A document timestamp, or a field holding one: validation data.
        if dict.name("Type") == Some(b"DocTimeStamp") {
            return Change::Dss;
        }
        if let Object::Dict(v) = self.resolve_key(dict, "V")
            && v.name("Type") == Some(b"DocTimeStamp")
        {
            return Change::Dss;
        }
        let is_widget = dict.name("Subtype") == Some(b"Widget");
        let is_field = dict.get("FT").is_some() || dict.get("T").is_some();
        if is_widget || is_field {
            return match self.field_type(dict).as_deref() {
                Some(b"Sig") => Change::Signed,
                Some(_) => Change::FormFilled,
                None if is_widget => Change::Other,
                None => Change::FormFilled,
            };
        }
        if dict.get("Subtype").is_some()
            && (dict.name("Type") == Some(b"Annot") || dict.get("Rect").is_some())
        {
            return Change::Commented;
        }
        Change::Other
    }

    /// A field's type, inherited from its parents.
    fn field_type(&self, dict: &Dict) -> Option<Vec<u8>> {
        let mut cur = dict.clone();
        for _ in 0..16 {
            if let Some(ft) = cur.name("FT") {
                return Some(ft.to_vec());
            }
            match self.resolve_key(&cur, "Parent") {
                Object::Dict(parent) => cur = parent,
                _ => return None,
            }
        }
        None
    }

    /// The appearance streams of every annotation, by object number,
    /// with the change that redrawing them is.
    fn appearances(&self) -> HashMap<u32, Change> {
        let mut out = HashMap::new();
        for page in self.pages() {
            let Some(Object::Dict(page)) = self.get(page) else {
                continue;
            };
            let Object::Array(annots) = self.resolve_key(&page, "Annots") else {
                continue;
            };
            for annot in annots {
                let Object::Dict(dict) = self.resolve(&annot) else {
                    continue;
                };
                let change = self.kind_of(&dict);
                let Object::Dict(ap) = self.resolve_key(&dict, "AP") else {
                    continue;
                };
                for (_, state) in ap.iter() {
                    let mut refs = Vec::new();
                    match state {
                        Object::Ref(r) => {
                            refs.push(*r);
                            if let Some(Object::Dict(states)) = self.get(*r) {
                                refs.extend(states.iter().filter_map(|(_, o)| o.as_ref()));
                            }
                        }
                        Object::Dict(states) => {
                            refs.extend(states.iter().filter_map(|(_, o)| o.as_ref()));
                        }
                        _ => {}
                    }
                    for r in refs {
                        out.insert(r.num, change);
                    }
                }
            }
        }
        out
    }
}

/// The keys whose values differ between `a` and `b`, or are in only one.
fn changed_keys(a: &Dict, b: &Dict) -> BTreeSet<Name> {
    a.iter()
        .chain(b.iter())
        .map(|(k, _)| k)
        .filter(|k| {
            let k = String::from_utf8_lossy(k.as_bytes());
            a.get(&k) != b.get(&k)
        })
        .cloned()
        .collect()
}
