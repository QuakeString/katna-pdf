// SPDX-License-Identifier: GPL-3.0-or-later

//! Putting a password on a file and taking it off (docs/PLAN.md §4.3,
//! "security"): the whole file written again, every object locked with a
//! new AES-256 key, or plain. An update cannot do this, since the older
//! revisions it keeps would still be readable without the password. Only
//! someone who opened the file as its owner may change its lock.

use std::collections::BTreeMap;
use std::io::Write as _;

use crate::crypt::{Crypt, Permissions, new_lock};
use crate::document::Document;
use crate::object::{Object, Ref};
use crate::write::write_object;

/// Why a file's lock could not be changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockError {
    /// The file is locked and was not opened with its password.
    Locked,
    /// It was opened with the user password; changing the lock needs the
    /// owner's.
    NotOwner,
    /// The file has nothing to write (no catalog).
    Empty,
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LockError::Locked => "the file is locked; open it with its password first",
            LockError::NotOwner => "changing the password needs the owner's password",
            LockError::Empty => "the file has no pages to write",
        })
    }
}

impl std::error::Error for LockError {}

/// The lock a file written whole is given.
pub(crate) enum NewLock {
    /// A new one.
    Set(crate::object::Dict, Crypt),
    /// The one it has, with its key, if any.
    Keep,
    /// None.
    Off,
}

impl Document {
    /// The file locked with AES-256: `user` opens it with `permissions`
    /// (an empty one lets anyone open it, with those permissions),
    /// `owner` opens it fully (an empty one is made random).
    pub fn lock(
        &self,
        user: &str,
        owner: &str,
        permissions: Permissions,
    ) -> Result<Vec<u8>, LockError> {
        let (dict, crypt) = new_lock(user, owner, permissions);
        self.write_whole(NewLock::Set(dict, crypt), false, &BTreeMap::new())
    }

    /// The file with no password.
    pub fn without_lock(&self) -> Result<Vec<u8>, LockError> {
        self.write_whole(NewLock::Off, false, &BTreeMap::new())
    }

    /// Every object written again, by its number, locked as `lock` says;
    /// only those the file still uses when `reachable`, so nothing an
    /// older revision or a dropped object held is left in it. Objects in
    /// `changed` (by number, with their generation) are written in place
    /// of the file's, or as well as them.
    pub(crate) fn write_whole(
        &self,
        lock: NewLock,
        reachable: bool,
        changed: &BTreeMap<u32, (u16, Object)>,
    ) -> Result<Vec<u8>, LockError> {
        if self.is_locked() {
            return Err(LockError::Locked);
        }
        if !matches!(lock, NewLock::Keep) && !self.opened_as_owner() {
            return Err(LockError::NotOwner);
        }
        let kept = reachable.then(|| self.reachable(changed));
        let root = self
            .trailer()
            .get("Root")
            .cloned()
            .ok_or(LockError::Empty)?;
        let old_lock = match lock {
            NewLock::Keep => None,
            _ => self.trailer().get("Encrypt").and_then(Object::as_ref),
        };
        let mut objects: BTreeMap<u32, (u16, Object)> = BTreeMap::new();
        let mut numbers: Vec<u32> = self.object_numbers().collect();
        numbers.extend(changed.keys());
        numbers.sort_unstable();
        numbers.dedup();
        for num in numbers {
            let generation = match (changed.get(&num), self.entry(num)) {
                (Some((generation, _)), _) => *generation,
                (_, Some(crate::document::Entry::InFile { generation, .. })) => generation,
                _ => 0,
            };
            let r = Ref::new(num, generation);
            if old_lock.is_some_and(|l| l.num == num)
                || kept.as_ref().is_some_and(|k| !k.contains(&num))
            {
                continue;
            }
            let Some(obj) = changed
                .get(&num)
                .map(|(_, o)| o.clone())
                .or_else(|| self.get(r))
            else {
                continue;
            };
            // Cross-reference and object streams are made again (as a
            // table, and their objects on their own); a linearized
            // file's hints no longer fit.
            if let Object::Stream(s) = &obj
                && matches!(s.dict.name("Type"), Some(b"XRef") | Some(b"ObjStm"))
            {
                continue;
            }
            if let Object::Dict(d) = &obj
                && d.get("Linearized").is_some()
            {
                continue;
            }
            objects.insert(num, (generation, obj));
        }
        let mut next = objects.keys().next_back().map_or(1, |n| n + 1);
        let mut trailer = crate::object::Dict::new();
        trailer.set("Root", root);
        if let Some(info) = self.trailer().get("Info").cloned() {
            trailer.set("Info", info);
        }
        let id = match self.trailer().get("ID").map(|i| self.resolve(i)) {
            Some(Object::Array(a)) if !a.is_empty() => a,
            _ => {
                let mut first = [0u8; 16];
                crate::crypt::random(&mut first);
                let s = Object::String {
                    bytes: first.to_vec(),
                    hex: true,
                };
                vec![s.clone(), s]
            }
        };
        trailer.set("ID", Object::Array(id));
        let crypt = match lock {
            NewLock::Set(dict, crypt) => {
                let r = Ref::new(next, 0);
                next += 1;
                objects.insert(r.num, (0, Object::Dict(dict)));
                trailer.set("Encrypt", Object::Ref(r));
                Some(crypt.for_dict(r))
            }
            NewLock::Keep => match (self.trailer().get("Encrypt"), self.crypt()) {
                (Some(encrypt), Some(crypt)) => {
                    trailer.set("Encrypt", encrypt.clone());
                    Some(crypt.clone())
                }
                _ => None,
            },
            NewLock::Off => None,
        };
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets: BTreeMap<u32, (usize, u16)> = BTreeMap::new();
        for (num, (generation, obj)) in objects {
            offsets.insert(num, (out.len(), generation));
            let _ = writeln!(out, "{num} {generation} obj");
            let obj = match &crypt {
                Some(c) => c.encrypt(Ref::new(num, generation), obj),
                None => obj,
            };
            write_object(&mut out, &obj);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let _ = write!(out, "xref\n0 {next}\n");
        for num in 0..next {
            match offsets.get(&num) {
                Some((offset, generation)) => {
                    let _ = write!(out, "{offset:010} {generation:05} n\r\n");
                }
                None => out.extend_from_slice(b"0000000000 65535 f\r\n"),
            }
        }
        trailer.set("Size", i64::from(next));
        out.extend_from_slice(b"trailer\n");
        write_object(&mut out, &Object::Dict(trailer));
        let _ = write!(out, "\nstartxref\n{xref}\n%%EOF\n");
        Ok(out)
    }

    /// The numbers of the objects the file uses: those its trailer leads
    /// to, through any number of references.
    pub(crate) fn reachable(
        &self,
        changed: &BTreeMap<u32, (u16, Object)>,
    ) -> std::collections::HashSet<u32> {
        let mut seen = std::collections::HashSet::new();
        let mut todo: Vec<Ref> = Vec::new();
        let trailer = Object::Dict(self.trailer().clone());
        refs_in(&trailer, &mut todo);
        while let Some(r) = todo.pop() {
            if !seen.insert(r.num) {
                continue;
            }
            match changed.get(&r.num) {
                Some((_, obj)) => refs_in(obj, &mut todo),
                None => {
                    if let Some(obj) = self.get(r) {
                        refs_in(&obj, &mut todo);
                    }
                }
            }
        }
        seen
    }
}

/// The references `obj` holds, at any depth.
fn refs_in(obj: &Object, out: &mut Vec<Ref>) {
    match obj {
        Object::Ref(r) => out.push(*r),
        Object::Array(items) => items.iter().for_each(|o| refs_in(o, out)),
        Object::Dict(d) => d.iter().for_each(|(_, v)| refs_in(v, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| refs_in(v, out)),
        _ => {}
    }
}
