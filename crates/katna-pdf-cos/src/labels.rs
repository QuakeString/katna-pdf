// SPDX-License-Identifier: GPL-3.0-or-later

//! Page labels (ISO 32000-2 §12.4.2): the numbers a document gives its
//! pages, such as i, ii, iii for the front matter and 1, 2, 3 after it,
//! or A-1, A-2 for an appendix.

use std::collections::HashSet;

use crate::forms::text_object;
use crate::nav::NavError;
use crate::outline::text_value;
use crate::{Dict, Document, Object, Ref, UpdateError};

/// How deep the number tree may nest, and how many of its nodes are
/// read, before the rest is ignored.
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 100_000;

/// How a range's pages are numbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// 1, 2, 3
    Decimal,
    /// I, II, III
    UpperRoman,
    /// i, ii, iii
    LowerRoman,
    /// A to Z, then AA to ZZ, and so on
    UpperLetters,
    /// a to z, then aa to zz
    LowerLetters,
    /// The prefix only.
    None,
}

/// The pages from `start` to the next range's start, labelled the same
/// way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRange {
    /// The first page, from 0.
    pub start: usize,
    pub style: LabelStyle,
    pub prefix: String,
    /// The number of the range's first page (1 or more).
    pub first: u32,
}

impl LabelRange {
    /// The label of the range's `n`th page, from 0.
    pub fn label(&self, n: usize) -> String {
        let value = u64::from(self.first.max(1)) + n as u64;
        let number = match self.style {
            LabelStyle::Decimal => value.to_string(),
            LabelStyle::UpperRoman => roman(value),
            LabelStyle::LowerRoman => roman(value).to_lowercase(),
            LabelStyle::UpperLetters => letters(value),
            LabelStyle::LowerLetters => letters(value).to_lowercase(),
            LabelStyle::None => String::new(),
        };
        format!("{}{number}", self.prefix)
    }
}

/// Roman numerals; beyond 3999, as many M as it takes.
fn roman(mut n: u64) -> String {
    const NUMERALS: [(u64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    // A hostile start number cannot make a huge string.
    n = n.min(100_000);
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// A to Z for 1 to 26, then AA to ZZ for 27 to 52, and so on.
fn letters(n: u64) -> String {
    let n = n.clamp(1, 26 * 100);
    let letter = char::from(b'A' + ((n - 1) % 26) as u8);
    std::iter::repeat_n(letter, (n - 1) as usize / 26 + 1).collect()
}

impl Document {
    /// The ranges the file numbers its pages in, by first page; none when
    /// it numbers them plainly 1, 2, 3.
    pub fn page_label_ranges(&self) -> Vec<LabelRange> {
        if self.is_locked() {
            return Vec::new();
        }
        let Some(catalog) = self.catalog() else {
            return Vec::new();
        };
        let Object::Dict(root) = self.resolve_key(&catalog, "PageLabels") else {
            return Vec::new();
        };
        let mut pairs = Vec::new();
        self.number_tree(&root, 0, &mut HashSet::new(), &mut pairs);
        let count = self.pages().len();
        let mut out: Vec<LabelRange> = pairs
            .into_iter()
            .filter_map(|(start, value)| {
                let start = usize::try_from(start).ok().filter(|&s| s < count)?;
                let Object::Dict(d) = value else {
                    return None;
                };
                let style = match d.name("S") {
                    Some(b"D") => LabelStyle::Decimal,
                    Some(b"R") => LabelStyle::UpperRoman,
                    Some(b"r") => LabelStyle::LowerRoman,
                    Some(b"A") => LabelStyle::UpperLetters,
                    Some(b"a") => LabelStyle::LowerLetters,
                    _ => LabelStyle::None,
                };
                let prefix = match self.resolve_key(&d, "P") {
                    Object::String { bytes, .. } => text_value(&bytes),
                    _ => String::new(),
                };
                let first = self
                    .resolve_key(&d, "St")
                    .as_int()
                    .and_then(|n| u32::try_from(n).ok())
                    .filter(|&n| n >= 1)
                    .unwrap_or(1);
                Some(LabelRange {
                    start,
                    style,
                    prefix,
                    first,
                })
            })
            .collect();
        out.sort_by_key(|r| r.start);
        out.dedup_by_key(|r| r.start);
        out
    }

    /// Each page's label, in page order; nothing when the file has none.
    /// Pages before the first range (which should not be) are numbered
    /// plainly.
    pub fn page_labels(&self) -> Option<Vec<String>> {
        let ranges = self.page_label_ranges();
        if ranges.is_empty() {
            return None;
        }
        Some(labels_of(&ranges, self.pages().len()))
    }

    /// The document with its pages numbered by `ranges`; none takes the
    /// numbering out, back to 1, 2, 3.
    pub fn set_page_labels(&self, ranges: &[LabelRange]) -> Result<Vec<u8>, NavError> {
        let mut up = self.update()?;
        let root = self
            .trailer()
            .get("Root")
            .and_then(Object::as_ref)
            .ok_or(NavError::Update(UpdateError::Repaired))?;
        let count = self.pages().len();
        let mut catalog = self.catalog().unwrap_or_default();
        let mut ranges = ranges.to_vec();
        ranges.sort_by_key(|r| r.start);
        ranges.dedup_by_key(|r| r.start);
        if ranges.is_empty() {
            catalog.remove("PageLabels");
        } else {
            let mut nums = Vec::new();
            // The tree must start at page 1.
            if ranges[0].start != 0 {
                nums.push(Object::Int(0));
                let mut d = Dict::new();
                d.set("S", Object::name("D"));
                nums.push(Object::Dict(d));
            }
            for range in &ranges {
                if range.start >= count {
                    return Err(NavError::NoPage(range.start));
                }
                let mut d = Dict::new();
                let style = match range.style {
                    LabelStyle::Decimal => Some("D"),
                    LabelStyle::UpperRoman => Some("R"),
                    LabelStyle::LowerRoman => Some("r"),
                    LabelStyle::UpperLetters => Some("A"),
                    LabelStyle::LowerLetters => Some("a"),
                    LabelStyle::None => None,
                };
                if let Some(style) = style {
                    d.set("S", Object::name(style));
                }
                if !range.prefix.is_empty() {
                    d.set("P", text_object(&range.prefix));
                }
                if range.first > 1 {
                    d.set("St", i64::from(range.first));
                }
                nums.push(Object::Int(range.start as i64));
                nums.push(Object::Dict(d));
            }
            let mut tree = Dict::new();
            tree.set("Nums", nums);
            let tree = up.add(Object::Dict(tree));
            catalog.set("PageLabels", tree);
        }
        up.replace(root, Object::Dict(catalog));
        Ok(up.finish())
    }

    fn number_tree(
        &self,
        node: &Dict,
        depth: usize,
        seen: &mut HashSet<Ref>,
        out: &mut Vec<(i64, Object)>,
    ) {
        if depth > MAX_DEPTH || seen.len() > MAX_NODES {
            return;
        }
        if let Object::Array(nums) = self.resolve_key(node, "Nums") {
            for pair in nums.as_chunks::<2>().0 {
                if let Some(key) = self.resolve(&pair[0]).as_int() {
                    out.push((key, self.resolve(&pair[1])));
                }
            }
        }
        if let Object::Array(kids) = self.resolve_key(node, "Kids") {
            for kid in kids.iter().filter_map(Object::as_ref) {
                if seen.insert(kid)
                    && let Some(Object::Dict(kid)) = self.get(kid)
                {
                    self.number_tree(&kid, depth + 1, seen, out);
                }
            }
        }
    }
}

/// Each of `count` pages' labels under `ranges` (sorted by start).
pub fn labels_of(ranges: &[LabelRange], count: usize) -> Vec<String> {
    (0..count)
        .map(|page| match ranges.iter().rev().find(|r| r.start <= page) {
            Some(range) => range.label(page - range.start),
            None => (page + 1).to_string(),
        })
        .collect()
}
