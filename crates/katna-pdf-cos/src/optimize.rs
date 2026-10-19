// SPDX-License-Identifier: GPL-3.0-or-later

//! Making a file smaller (docs/PLAN.md §7, phase 5, "optimize and
//! compress"): the file written again whole with only the objects it
//! uses, streams compressed as tightly as Flate goes, the same stream
//! kept once however often it was embedded, objects packed into object
//! streams with a cross-reference stream, and, when asked, pictures
//! sharper than the pages need brought down to a resolution and saved
//! again. Pages look the same; only pictures lose detail, and only when
//! asked.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;

use crate::document::{Document, Entry};
use crate::filter;
use crate::object::{Dict, Object, Ref, Stream};
use crate::write::write_object;

/// How hard to try.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Optimize {
    /// Pictures brought down to this; none leaves them as they are.
    pub images: Option<ImageLimit>,
    /// Objects packed into object streams (PDF 1.5 and later readers).
    pub object_streams: bool,
}

/// How far pictures are brought down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageLimit {
    /// Dots per inch across the largest page.
    pub dpi: f32,
    /// JPEG quality, 1 to 100, for photographs.
    pub quality: u8,
}

impl Optimize {
    /// Nothing on the pages changes.
    pub const LOSSLESS: Optimize = Optimize {
        images: None,
        object_streams: true,
    };
    /// Pictures to 150 dpi: good on screen and on an office printer.
    pub const MEDIUM: Optimize = Optimize {
        images: Some(ImageLimit {
            dpi: 150.0,
            quality: 75,
        }),
        object_streams: true,
    };
    /// Pictures to 96 dpi: for mailing, read on screen.
    pub const STRONG: Optimize = Optimize {
        images: Some(ImageLimit {
            dpi: 96.0,
            quality: 60,
        }),
        object_streams: true,
    };
}

/// A smaller file, and what made it so.
#[derive(Debug, Clone)]
pub struct Optimized {
    pub bytes: Vec<u8>,
    /// Objects the file no longer used, left out.
    pub unused: usize,
    /// Streams that were copies of another, kept once.
    pub copies: usize,
    /// Pictures brought down.
    pub images: usize,
}

/// Why a file could not be made smaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizeError {
    /// It is locked and was not opened with its password.
    Locked,
    /// It is signed: writing it again would break the signatures.
    Signed,
    /// It has no catalog, or no pages this layer can find.
    Empty,
    /// It is damaged, and written again it would not read back the
    /// same.
    Damaged,
}

impl std::fmt::Display for OptimizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OptimizeError::Locked => "the file is locked; open it with its password first",
            OptimizeError::Signed => {
                "the file is signed, and writing it again would break the signatures"
            }
            OptimizeError::Empty => "the file has no pages to write",
            OptimizeError::Damaged => {
                "the file is damaged, and writing it again would lose parts of it"
            }
        })
    }
}

impl std::error::Error for OptimizeError {}

/// Objects packed into each object stream, at most.
const PER_OBJECT_STREAM: usize = 200;

impl Document {
    /// The file written again as small as `how` allows.
    pub fn optimize(&self, how: &Optimize) -> Result<Optimized, OptimizeError> {
        if self.is_locked() {
            return Err(OptimizeError::Locked);
        }
        if !self.signatures().is_empty() {
            return Err(OptimizeError::Signed);
        }
        let root = self
            .trailer()
            .get("Root")
            .and_then(Object::as_ref)
            .ok_or(OptimizeError::Empty)?;
        if self.pages().is_empty() {
            return Err(OptimizeError::Empty);
        }
        let encrypt = self.trailer().get("Encrypt").and_then(Object::as_ref);
        let reachable = self.reachable(&BTreeMap::new());
        let mut objects: BTreeMap<u32, Object> = BTreeMap::new();
        let mut unused = 0;
        for num in self.object_numbers() {
            if encrypt.is_some_and(|e| e.num == num) {
                continue;
            }
            if !reachable.contains(&num) {
                unused += 1;
                continue;
            }
            let generation = match self.entry(num) {
                Some(Entry::InFile { generation, .. }) => generation,
                _ => 0,
            };
            let Some(obj) = self.get(Ref::new(num, generation)) else {
                continue;
            };
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
            objects.insert(num, obj);
        }

        // Streams compressed again, pictures brought down.
        let side = self.largest_side();
        let mut images = 0;
        for obj in objects.values_mut() {
            let Object::Stream(s) = obj else { continue };
            if let Some(limit) = how.images
                && let Some(smaller) = self.smaller_image(s, side * limit.dpi, limit.quality)
            {
                *s = smaller;
                images += 1;
                continue;
            }
            if let Some(packed) = recompress(s) {
                *s = packed;
            }
        }

        // Each stream once.
        let mut first: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut same: HashMap<u32, u32> = HashMap::new();
        for (&num, obj) in &objects {
            if let Object::Stream(_) = obj {
                let mut key = Vec::new();
                write_object(&mut key, obj);
                match first.get(&key) {
                    Some(&kept) => {
                        same.insert(num, kept);
                    }
                    None => {
                        first.insert(key, num);
                    }
                }
            }
        }
        let copies = same.len();
        for num in same.keys() {
            objects.remove(num);
        }

        // Numbered again from 1, in order.
        let numbers: HashMap<u32, u32> = objects
            .keys()
            .enumerate()
            .map(|(i, &num)| (num, i as u32 + 1))
            .collect();
        let renumber = |r: Ref| -> Option<Ref> {
            let num = same.get(&r.num).copied().unwrap_or(r.num);
            numbers.get(&num).map(|&n| Ref::new(n, 0))
        };
        let mut objects: Vec<Object> = objects
            .into_values()
            .map(|obj| renumbered(obj, &renumber))
            .collect();

        let mut trailer = Dict::new();
        trailer.set("Root", renumber(root).map_or(Object::Null, Object::Ref));
        if let Some(info) = self.trailer().get("Info").and_then(Object::as_ref)
            && let Some(info) = renumber(info)
        {
            trailer.set("Info", Object::Ref(info));
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

        let bytes = match (encrypt, self.crypt()) {
            (Some(_), Some(crypt)) => {
                // Locked files keep their lock, in a plain table.
                let encrypt = self
                    .trailer()
                    .get("Encrypt")
                    .map(|e| renumbered(self.resolve(e), &renumber));
                let r = Ref::new(objects.len() as u32 + 1, 0);
                objects.push(encrypt.unwrap_or(Object::Null));
                trailer.set("Encrypt", Object::Ref(r));
                let crypt = crypt.clone().for_dict(r);
                let objects: Vec<Object> = objects
                    .into_iter()
                    .enumerate()
                    .map(|(i, obj)| crypt.encrypt(Ref::new(i as u32 + 1, 0), obj))
                    .collect();
                crate::write::write_file(&objects, trailer)
            }
            _ if how.object_streams => packed_file(objects, trailer),
            _ => crate::write::write_file(&objects, trailer),
        };
        if !self.reads_back(&bytes) {
            return Err(OptimizeError::Damaged);
        }
        Ok(Optimized {
            bytes,
            unused,
            copies,
            images,
        })
    }

    /// Whether `bytes` read back with the same pages, each drawn from
    /// the same content.
    fn reads_back(&self, bytes: &[u8]) -> bool {
        let Ok(out) = Document::load(bytes.to_vec()) else {
            return false;
        };
        let count = self.pages().len();
        out.pages().len() == count
            // A file locked with a password only it was opened with can
            // only be counted.
            && (out.is_locked() || (0..count).all(|page| {
                self.page_content(page).map(|c| c.content)
                    == out.page_content(page).map(|c| c.content)
            }))
    }

    /// The longest side of the largest page, in inches.
    fn largest_side(&self) -> f32 {
        let longest = (0..self.pages().len())
            .filter_map(|i| self.page_info(i))
            .map(|p| {
                let [x0, y0, x1, y1] = p.media;
                (x1 - x0).abs().max((y1 - y0).abs())
            })
            .fold(0.0f32, f32::max);
        if longest > 0.0 {
            longest / 72.0
        } else {
            // A4's long side.
            11.7
        }
    }

    /// Picture `s` brought down so neither side is over `most` pixels,
    /// each side on its own (a picture may be stretched to fit its
    /// place), if it is a kind this can read and that makes it smaller.
    fn smaller_image(&self, s: &Stream, most: f32, quality: u8) -> Option<Stream> {
        let d = &s.dict;
        if d.name("Subtype") != Some(b"Image")
            || matches!(self.resolve_key(d, "ImageMask"), Object::Bool(true))
            || d.get("Mask").is_some_and(|m| matches!(m, Object::Array(_)))
            || self.resolve_key(d, "BitsPerComponent").as_int() != Some(8)
        {
            return None;
        }
        let width = usize::try_from(self.resolve_key(d, "Width").as_int()?).ok()?;
        let height = usize::try_from(self.resolve_key(d, "Height").as_int()?).ok()?;
        // Only well past the limit: a little over is not worth a loss.
        if width == 0 || height == 0 || (width.max(height) as f32) <= most * 1.25 {
            return None;
        }
        let components = self.components(&self.resolve_key(d, "ColorSpace"))?;
        let filters: Vec<Vec<u8>> = match self.resolve_key(d, "Filter") {
            Object::Null => Vec::new(),
            Object::Name(n) => vec![n.as_bytes().to_vec()],
            Object::Array(a) => a
                .iter()
                .filter_map(|f| f.as_name().map(<[u8]>::to_vec))
                .collect(),
            _ => return None,
        };
        let photo = filters
            .last()
            .is_some_and(|f| f == b"DCTDecode" || f == b"DCT");
        let samples = if photo {
            if filters.len() != 1 {
                return None;
            }
            let (samples, n) = crate::redact::jpeg(&s.data)?;
            if n != components {
                return None;
            }
            samples
        } else {
            filter::decode(d, &s.data).ok()?
        };
        if samples.len() != width * height * components {
            return None;
        }
        let most = most.round() as usize;
        let (w, h) = (width.min(most).max(1), height.min(most).max(1));
        let small = resample(&samples, width, height, components, w, h);
        let data = if photo {
            let color = if components == 3 {
                jpeg_encoder::ColorType::Rgb
            } else {
                jpeg_encoder::ColorType::Luma
            };
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100))
                .encode(
                    &small,
                    u16::try_from(w).ok()?,
                    u16::try_from(h).ok()?,
                    color,
                )
                .ok()?;
            out
        } else {
            deflate(&small)
        };
        if data.len() >= s.data.len() {
            return None;
        }
        let mut dict = d.clone();
        dict.set("Width", w as i64);
        dict.set("Height", h as i64);
        dict.set(
            "Filter",
            Object::name(if photo { "DCTDecode" } else { "FlateDecode" }),
        );
        dict.remove("DecodeParms");
        dict.remove("DP");
        Some(Stream { dict, data })
    }

    /// How many components a pixel of colour space `cs` has, for those
    /// whose samples can be averaged: grey and RGB, plain or ICC based.
    fn components(&self, cs: &Object) -> Option<usize> {
        match cs {
            Object::Name(n) => match n.as_bytes() {
                b"DeviceGray" | b"G" | b"CalGray" => Some(1),
                b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(3),
                _ => None,
            },
            Object::Array(a) => match a.first().and_then(Object::as_name)? {
                b"ICCBased" => {
                    let Object::Stream(icc) = self.resolve(a.get(1)?) else {
                        return None;
                    };
                    match icc.dict.int("N")? {
                        1 => Some(1),
                        3 => Some(3),
                        _ => None,
                    }
                }
                b"CalGray" => Some(1),
                b"CalRGB" => Some(3),
                _ => None,
            },
            _ => None,
        }
    }
}

/// `s` compressed again with Flate at its tightest, when all its filters
/// can be undone without loss and that makes it smaller. XMP metadata
/// is left as it is, for tools that look for it in the plain.
fn recompress(s: &Stream) -> Option<Stream> {
    if s.dict.name("Type") == Some(b"Metadata") {
        return None;
    }
    let lossless = |f: &[u8]| {
        matches!(
            f,
            b"FlateDecode"
                | b"Fl"
                | b"LZWDecode"
                | b"LZW"
                | b"ASCII85Decode"
                | b"A85"
                | b"ASCIIHexDecode"
                | b"AHx"
                | b"RunLengthDecode"
                | b"RL"
        )
    };
    match s.dict.get("Filter") {
        None => {}
        Some(Object::Name(n)) if lossless(n.as_bytes()) => {}
        Some(Object::Array(a)) if a.iter().all(|f| f.as_name().is_some_and(lossless)) => {}
        _ => return None,
    }
    // Predictors are kept: they are there because they pack the data
    // better, and how they undo is where readers most disagree.
    if s.dict.get("DecodeParms").is_some() || s.dict.get("DP").is_some() {
        return None;
    }
    let plain = filter::decode_whole(&s.dict, &s.data).ok()?;
    let data = deflate(&plain);
    if data.len() >= s.data.len() {
        return None;
    }
    let mut dict = s.dict.clone();
    dict.set("Filter", Object::name("FlateDecode"));
    dict.remove("DecodeParms");
    dict.remove("DP");
    Some(Stream { dict, data })
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// `samples` (`width` × `height`, `n` per pixel) brought down to `w` ×
/// `h` by averaging the pixels each new one covers.
fn resample(samples: &[u8], width: usize, height: usize, n: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * n);
    let mut sums = vec![0u64; n];
    for y in 0..h {
        let y0 = y * height / h;
        let y1 = ((y + 1) * height / h).max(y0 + 1).min(height);
        for x in 0..w {
            let x0 = x * width / w;
            let x1 = ((x + 1) * width / w).max(x0 + 1).min(width);
            sums.iter_mut().for_each(|s| *s = 0);
            for row in y0..y1 {
                let start = (row * width + x0) * n;
                for px in samples[start..start + (x1 - x0) * n].chunks_exact(n) {
                    for (s, &v) in sums.iter_mut().zip(px) {
                        *s += u64::from(v);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sums.iter().map(|s| ((s + count / 2) / count) as u8));
        }
    }
    out
}

/// `obj` with each reference numbered as `renumber` says; a reference to
/// an object that is gone becomes null.
fn renumbered(obj: Object, renumber: &impl Fn(Ref) -> Option<Ref>) -> Object {
    match obj {
        Object::Ref(r) => renumber(r).map_or(Object::Null, Object::Ref),
        Object::Array(items) => {
            Object::Array(items.into_iter().map(|o| renumbered(o, renumber)).collect())
        }
        Object::Dict(d) => Object::Dict(renumbered_dict(d, renumber)),
        Object::Stream(s) => Object::Stream(Stream {
            dict: renumbered_dict(s.dict, renumber),
            data: s.data,
        }),
        other => other,
    }
}

fn renumbered_dict(d: Dict, renumber: &impl Fn(Ref) -> Option<Ref>) -> Dict {
    Dict(
        d.0.into_iter()
            .map(|(k, v)| (k, renumbered(v, renumber)))
            .collect(),
    )
}

/// A whole file of `objects` (object `n` at `n - 1`), those that are not
/// streams packed into object streams, found through a cross-reference
/// stream.
fn packed_file(objects: Vec<Object>, mut trailer: Dict) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    // Where each object is: (1, offset) in the file, (2, stream, index)
    // packed.
    let count = objects.len() as u32;
    let mut place: Vec<(u8, u64, u32)> = vec![(0, 0, 0); count as usize + 1];
    let mut next = count + 1;
    let mut packed: Vec<(u32, Object)> = Vec::new();
    for (i, obj) in objects.into_iter().enumerate() {
        let num = i as u32 + 1;
        match obj {
            Object::Stream(_) => {
                place[num as usize] = (1, out.len() as u64, 0);
                let _ = writeln!(out, "{num} 0 obj");
                write_object(&mut out, &obj);
                out.extend_from_slice(b"\nendobj\n");
            }
            obj => packed.push((num, obj)),
        }
    }
    for chunk in packed.chunks(PER_OBJECT_STREAM) {
        let stream_num = next;
        next += 1;
        let mut header = Vec::new();
        let mut body = Vec::new();
        for (index, (num, obj)) in chunk.iter().enumerate() {
            let _ = write!(header, "{num} {} ", body.len());
            write_object(&mut body, obj);
            body.push(b'\n');
            place[*num as usize] = (2, u64::from(stream_num), index as u32);
        }
        let first = header.len();
        header.extend_from_slice(&body);
        let mut dict = Dict::new();
        dict.set("Type", Object::name("ObjStm"));
        dict.set("N", chunk.len() as i64);
        dict.set("First", first as i64);
        dict.set("Filter", Object::name("FlateDecode"));
        place.push((1, out.len() as u64, 0));
        let _ = writeln!(out, "{stream_num} 0 obj");
        write_object(
            &mut out,
            &Object::Stream(Stream {
                dict,
                data: deflate(&header),
            }),
        );
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_num = next;
    let xref_at = out.len() as u64;
    place.push((1, xref_at, 0));
    let mut rows = Vec::with_capacity(place.len() * 7);
    for (i, &(kind, field, index)) in place.iter().enumerate() {
        let (kind, field, index) = if i == 0 {
            (0u8, 0u64, 65_535u32)
        } else {
            (kind, field, index)
        };
        rows.push(kind);
        rows.extend_from_slice(&(field as u32).to_be_bytes());
        rows.extend_from_slice(&(index as u16).to_be_bytes());
    }
    trailer.set("Type", Object::name("XRef"));
    trailer.set("Size", i64::from(xref_num) + 1);
    trailer.set(
        "W",
        Object::Array(vec![Object::Int(1), Object::Int(4), Object::Int(2)]),
    );
    trailer.set("Filter", Object::name("FlateDecode"));
    let _ = writeln!(out, "{xref_num} 0 obj");
    write_object(
        &mut out,
        &Object::Stream(Stream {
            dict: trailer,
            data: deflate(&rows),
        }),
    );
    let _ = write!(out, "\nendobj\nstartxref\n{xref_at}\n%%EOF\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_averaged_down() {
        // 4 × 2 grey, to 2 × 1.
        let samples = [0, 100, 200, 255, 0, 100, 200, 255];
        assert_eq!(resample(&samples, 4, 2, 1, 2, 1), [50, 228]);
        // RGB keeps its channels apart.
        let rgb = [255, 0, 0, 0, 0, 255];
        assert_eq!(resample(&rgb, 2, 1, 3, 1, 1), [128, 0, 128]);
    }
}
