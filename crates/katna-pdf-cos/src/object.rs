// SPDX-License-Identifier: GPL-3.0-or-later

//! PDF objects (ISO 32000-2 §7.3).

use std::fmt;

/// An indirect reference: object number and generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ref {
    pub num: u32,
    pub generation: u16,
}

impl Ref {
    pub const fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }
}

impl fmt::Display for Ref {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// A name, without its leading `/` and with `#xx` escapes decoded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(pub Vec<u8>);

impl Name {
    pub fn new(name: &str) -> Self {
        Self(name.as_bytes().to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", String::from_utf8_lossy(&self.0))
    }
}

/// A dictionary. Keys keep the order they were read or added in, so a
/// rewritten object reads like the original.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dict(pub Vec<(Name, Object)>);

impl Dict {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &str) -> Option<&Object> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Object> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Sets `key`, replacing any value it had.
    pub fn set(&mut self, key: &str, value: impl Into<Object>) {
        let value = value.into();
        match self.get_mut(key) {
            Some(slot) => *slot = value,
            None => self.0.push((Name::new(key), value)),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Object> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    /// The value of `key` when it is a name, as text.
    pub fn name(&self, key: &str) -> Option<&[u8]> {
        match self.get(key)? {
            Object::Name(n) => Some(n.as_bytes()),
            _ => None,
        }
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.get(key)?.as_int()
    }
}

/// A stream: its dictionary and its bytes as stored in the file (still
/// encoded by its filters).
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    /// A string's bytes; `hex` remembers how it was written.
    String {
        bytes: Vec<u8>,
        hex: bool,
    },
    Name(Name),
    Array(Vec<Object>),
    Dict(Dict),
    Stream(Stream),
    Ref(Ref),
}

impl Object {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Object::Int(i) => Some(*i),
            Object::Real(r) if r.fract() == 0.0 => Some(*r as i64),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Object::Int(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_ref(&self) -> Option<Ref> {
        match self {
            Object::Ref(r) => Some(*r),
            _ => None,
        }
    }

    pub fn as_name(&self) -> Option<&[u8]> {
        match self {
            Object::Name(n) => Some(n.as_bytes()),
            _ => None,
        }
    }

    pub fn string(text: &str) -> Self {
        Object::String {
            bytes: text.as_bytes().to_vec(),
            hex: false,
        }
    }

    pub fn name(name: &str) -> Self {
        Object::Name(Name::new(name))
    }
}

impl From<i64> for Object {
    fn from(v: i64) -> Self {
        Object::Int(v)
    }
}

impl From<bool> for Object {
    fn from(v: bool) -> Self {
        Object::Bool(v)
    }
}

impl From<Ref> for Object {
    fn from(v: Ref) -> Self {
        Object::Ref(v)
    }
}

impl From<Dict> for Object {
    fn from(v: Dict) -> Self {
        Object::Dict(v)
    }
}

impl From<Vec<Object>> for Object {
    fn from(v: Vec<Object>) -> Self {
        Object::Array(v)
    }
}

impl From<Name> for Object {
    fn from(v: Name) -> Self {
        Object::Name(v)
    }
}
