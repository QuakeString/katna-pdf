// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading objects from bytes (ISO 32000-2 §7.2, §7.3).
//!
//! The parser is forgiving where real files are sloppy (stray bytes, a
//! wrong `/Length`, a missing `endobj`) and never panics or loops on bad
//! input: every step consumes bytes or stops, and nesting is limited.

use crate::object::{Dict, Name, Object, Ref, Stream};

/// Deeper nesting than this is treated as damage, not data.
const MAX_DEPTH: usize = 256;

pub fn is_white(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

pub fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_white(b) && !is_delim(b)
}

/// A cursor over a file's bytes.
#[derive(Clone)]
pub struct Parser<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Parser<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Skips white space and comments.
    pub fn skip_white(&mut self) {
        while let Some(b) = self.peek() {
            if is_white(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(b) = self.peek() {
                    if b == b'\n' || b == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// A run of regular characters: a keyword or number.
    pub fn token(&mut self) -> &'a [u8] {
        self.skip_white();
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        &self.data[start..self.pos]
    }

    /// Consumes `keyword` if it comes next, as a whole token.
    pub fn eat(&mut self, keyword: &[u8]) -> bool {
        let saved = self.pos;
        if self.token() == keyword {
            true
        } else {
            self.pos = saved;
            false
        }
    }

    pub fn uint(&mut self) -> Option<u64> {
        let saved = self.pos;
        let t = self.token();
        match std::str::from_utf8(t).ok().and_then(|s| s.parse().ok()) {
            Some(n) if !t.is_empty() && t.iter().all(u8::is_ascii_digit) => Some(n),
            _ => {
                self.pos = saved;
                None
            }
        }
    }

    /// `N G obj` at the cursor.
    pub fn object_header(&mut self) -> Option<Ref> {
        let saved = self.pos;
        let r = (|| {
            let num = u32::try_from(self.uint()?).ok()?;
            let generation = u16::try_from(self.uint()?).ok()?;
            self.eat(b"obj").then_some(Ref::new(num, generation))
        })();
        if r.is_none() {
            self.pos = saved;
        }
        r
    }

    /// An indirect object's body after its header: the object and, for a
    /// stream, its data. `length` resolves an indirect `/Length`.
    pub fn indirect_body(&mut self, length: &dyn Fn(&Object) -> Option<usize>) -> Option<Object> {
        let obj = self.object(0)?;
        let Object::Dict(dict) = obj else {
            self.eat(b"endobj");
            return Some(obj);
        };
        let saved = self.pos;
        if !self.eat(b"stream") {
            self.pos = saved;
            self.eat(b"endobj");
            return Some(Object::Dict(dict));
        }
        // The data starts after CRLF or LF (a lone CR is tolerated).
        match self.peek() {
            Some(b'\r') => {
                self.pos += 1;
                if self.peek() == Some(b'\n') {
                    self.pos += 1;
                }
            }
            Some(b'\n') => self.pos += 1,
            _ => {}
        }
        let start = self.pos;
        let declared = dict.get("Length").and_then(length);
        let end = match declared {
            Some(n) if self.ends_stream_at(start.saturating_add(n)) => start + n,
            _ => self.find_endstream(start)?,
        };
        let data = self.data[start..end].to_vec();
        self.pos = end;
        self.eat(b"endstream");
        self.eat(b"endobj");
        Some(Object::Stream(Stream { dict, data }))
    }

    /// Whether `endstream` follows `end`, after optional end-of-line.
    fn ends_stream_at(&self, end: usize) -> bool {
        if end > self.data.len() {
            return false;
        }
        let mut p = Parser::new(self.data, end);
        p.skip_white();
        p.data[p.pos..].starts_with(b"endstream")
    }

    /// The stream's end when `/Length` is wrong: just before `endstream`,
    /// less the end-of-line in front of it.
    fn find_endstream(&self, start: usize) -> Option<usize> {
        let at = find(&self.data[start..], b"endstream")? + start;
        let mut end = at;
        if end > start && self.data[end - 1] == b'\n' {
            end -= 1;
        }
        if end > start && self.data[end - 1] == b'\r' {
            end -= 1;
        }
        Some(end)
    }

    /// One direct object. References `N G R` are recognised here.
    pub fn object(&mut self, depth: usize) -> Option<Object> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_white();
        let b = self.peek()?;
        match b {
            b'/' => {
                self.pos += 1;
                Some(Object::Name(self.name()))
            }
            b'(' => {
                self.pos += 1;
                Some(Object::String {
                    bytes: self.literal_string(),
                    hex: false,
                })
            }
            b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                self.pos += 2;
                self.dict(depth).map(Object::Dict)
            }
            b'<' => {
                self.pos += 1;
                Some(Object::String {
                    bytes: self.hex_string(),
                    hex: true,
                })
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_white();
                    match self.peek() {
                        None => break,
                        Some(b']') => {
                            self.pos += 1;
                            break;
                        }
                        _ => {}
                    }
                    let before = self.pos;
                    match self.object(depth + 1) {
                        Some(o) => items.push(o),
                        None if self.pos == before => {
                            // Unreadable byte: skip it rather than loop.
                            self.pos += 1;
                        }
                        None => {}
                    }
                }
                Some(Object::Array(items))
            }
            b')' | b'>' | b']' | b'{' | b'}' => {
                self.pos += 1;
                None
            }
            _ => self.keyword_or_number(),
        }
    }

    fn keyword_or_number(&mut self) -> Option<Object> {
        let start = self.pos;
        let t = self.token();
        match t {
            b"true" => return Some(Object::Bool(true)),
            b"false" => return Some(Object::Bool(false)),
            b"null" => return Some(Object::Null),
            _ => {}
        }
        if t.is_empty() {
            self.pos = start + 1;
            return None;
        }
        // An integer may start a reference `N G R`.
        if t.iter().all(u8::is_ascii_digit) {
            let after = self.pos;
            let mut look = self.clone();
            if let (Some(generation), true) = (look.uint(), look.eat(b"R"))
                && let (Ok(num), Ok(generation)) = (
                    u32::try_from(parse_int(t).unwrap_or(-1)),
                    u16::try_from(generation),
                )
            {
                self.pos = look.pos;
                return Some(Object::Ref(Ref::new(num, generation)));
            }
            self.pos = after;
        }
        if let Some(i) = parse_int(t) {
            return Some(Object::Int(i));
        }
        if let Some(r) = parse_real(t) {
            return Some(Object::Real(r));
        }
        // An unknown keyword (such as a stray `endobj`): not an object.
        self.pos = start;
        None
    }

    fn name(&mut self) -> Name {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
            if b == b'#'
                && let (Some(h), Some(l)) = (
                    self.data.get(self.pos).and_then(|c| hex_val(*c)),
                    self.data.get(self.pos + 1).and_then(|c| hex_val(*c)),
                )
            {
                out.push(h << 4 | l);
                self.pos += 2;
                continue;
            }
            out.push(b);
        }
        Name(out)
    }

    fn literal_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut nesting = 1usize;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'(' => {
                    nesting += 1;
                    out.push(b);
                }
                b')' => {
                    nesting -= 1;
                    if nesting == 0 {
                        break;
                    }
                    out.push(b);
                }
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = u32::from(e - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                b'\r' => {
                    // End-of-line in a string reads as LF.
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(b),
            }
        }
        out
    }

    fn hex_string(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.pos += 1;
            if b == b'>' {
                break;
            }
            if let Some(v) = hex_val(b) {
                match high.take() {
                    Some(h) => out.push(h << 4 | v),
                    None => high = Some(v),
                }
            }
        }
        if let Some(h) = high {
            out.push(h << 4);
        }
        out
    }

    fn dict(&mut self, depth: usize) -> Option<Dict> {
        let mut dict = Dict::new();
        loop {
            self.skip_white();
            match self.peek() {
                None => return Some(dict),
                Some(b'>') => {
                    self.pos += 1;
                    if self.peek() == Some(b'>') {
                        self.pos += 1;
                    }
                    return Some(dict);
                }
                Some(b'/') => {
                    self.pos += 1;
                    let key = self.name();
                    self.skip_white();
                    if self.peek() == Some(b'>') {
                        // A key with no value at the end: drop it.
                        continue;
                    }
                    let before = self.pos;
                    match self.object(depth + 1) {
                        Some(v) => dict.0.push((key, v)),
                        None if self.pos == before => self.pos += 1,
                        None => {}
                    }
                }
                Some(_) => {
                    // Not a key: a damaged dictionary. Stop at a keyword
                    // that cannot be inside one; otherwise skip a byte.
                    let saved = self.pos;
                    let t = self.token();
                    if matches!(t, b"stream" | b"endobj" | b"obj" | b"endstream") {
                        self.pos = saved;
                        return Some(dict);
                    }
                    if t.is_empty() {
                        self.pos += 1;
                    }
                }
            }
        }
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn parse_int(t: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(t).ok()?;
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_real(t: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(t).ok()?;
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    // PDF reals: digits with one point; also tolerate "--5" and "5-".
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    if body.bytes().filter(|b| *b == b'.').count() > 1 {
        return None;
    }
    let v: f64 = if body == "." { 0.0 } else { body.parse().ok()? };
    Some(if s.starts_with('-') { -v } else { v })
}

/// The first position of `needle` in `hay`.
pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The last position of `needle` in `hay`.
pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> Object {
        Parser::new(src.as_bytes(), 0).object(0).unwrap()
    }

    #[test]
    fn numbers() {
        assert_eq!(one("42"), Object::Int(42));
        assert_eq!(one("-7"), Object::Int(-7));
        assert_eq!(one("+3"), Object::Int(3));
        assert_eq!(one("3.5"), Object::Real(3.5));
        assert_eq!(one("-.25"), Object::Real(-0.25));
        assert_eq!(one("4."), Object::Real(4.0));
    }

    #[test]
    fn references_and_plain_integers() {
        assert_eq!(one("12 0 R"), Object::Ref(Ref::new(12, 0)));
        assert_eq!(
            one("[1 2 3 0 R 4]"),
            Object::Array(vec![
                Object::Int(1),
                Object::Int(2),
                Object::Ref(Ref::new(3, 0)),
                Object::Int(4)
            ])
        );
    }

    #[test]
    fn names_decode_escapes() {
        assert_eq!(one("/A#20B"), Object::Name(Name(b"A B".to_vec())));
    }

    #[test]
    fn strings() {
        assert_eq!(
            one(r"(a\(b\)c\n\101 (nested))"),
            Object::String {
                bytes: b"a(b)c\nA (nested)".to_vec(),
                hex: false
            }
        );
        assert_eq!(
            one("<48 65 6C6c6F7>"),
            Object::String {
                bytes: b"Hellop".to_vec(),
                hex: true
            }
        );
    }

    #[test]
    fn dictionaries_keep_order() {
        let Object::Dict(d) = one("<< /Type /Page /Count 3 /Kids [1 0 R] >>") else {
            panic!()
        };
        let keys: Vec<_> = d.iter().map(|(k, _)| k.to_string()).collect();
        assert_eq!(keys, ["/Type", "/Count", "/Kids"]);
        assert_eq!(d.int("Count"), Some(3));
    }

    #[test]
    fn stream_with_wrong_length() {
        let src = b"<< /Length 99 >>\nstream\nhello\nendstream\nendobj";
        let mut p = Parser::new(src, 0);
        let obj = p
            .indirect_body(&|o| o.as_int().map(|i| i as usize))
            .unwrap();
        let Object::Stream(s) = obj else { panic!() };
        assert_eq!(s.data, b"hello");
    }

    #[test]
    fn garbage_does_not_hang() {
        for src in ["[[[[[", "<<<<", "(((", "<< /A", "] ) >", "1 2", "<< /K >>"] {
            let mut p = Parser::new(src.as_bytes(), 0);
            for _ in 0..20 {
                p.object(0);
            }
        }
        let deep = "[".repeat(10_000);
        Parser::new(deep.as_bytes(), 0).object(0);
    }
}
