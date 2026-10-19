// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding the stream filters the object layer itself needs: Flate with
//! PNG and TIFF predictors (xref and object streams), and the others a
//! page's content may be written in for editing it (LZW, ASCII85,
//! ASCIIHex, RunLength). Images are the renderer's job.

use std::io::Read as _;

use crate::object::{Dict, Object};

/// Decoded streams larger than this are refused, so a tiny "zip bomb"
/// cannot exhaust memory.
pub const MAX_DECODED: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterError {
    /// A filter this layer does not decode (images, mostly).
    Unsupported(String),
    Corrupt,
    TooLarge,
}

/// Decodes `data` by the filters in `dict`.
pub fn decode(dict: &Dict, data: &[u8]) -> Result<Vec<u8>, FilterError> {
    decode_as(dict, data, false)
}

/// As [`decode`], but a Flate stream cut short or damaged is an error
/// rather than what could be read of it: for writing a stream again,
/// where readers that recover more of it would lose what they found.
pub(crate) fn decode_whole(dict: &Dict, data: &[u8]) -> Result<Vec<u8>, FilterError> {
    decode_as(dict, data, true)
}

fn decode_as(dict: &Dict, data: &[u8], whole: bool) -> Result<Vec<u8>, FilterError> {
    let filters: Vec<&[u8]> = match dict.get("Filter") {
        None => return Ok(data.to_vec()),
        Some(Object::Name(n)) => vec![n.as_bytes()],
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_name).collect(),
        Some(_) => return Err(FilterError::Corrupt),
    };
    let params: Vec<Option<&Dict>> = match dict.get("DecodeParms").or_else(|| dict.get("DP")) {
        Some(Object::Dict(d)) => vec![Some(d)],
        Some(Object::Array(a)) => a.iter().map(Object::as_dict).collect(),
        _ => vec![],
    };
    let mut out = data.to_vec();
    for (i, f) in filters.iter().enumerate() {
        out = match *f {
            b"FlateDecode" | b"Fl" => {
                let raw = inflate(&out, whole)?;
                predict(params.get(i).copied().flatten(), raw)?
            }
            b"LZWDecode" | b"LZW" => {
                let early = params
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|p| p.int("EarlyChange"))
                    != Some(0);
                let raw = lzw(&out, early)?;
                predict(params.get(i).copied().flatten(), raw)?
            }
            b"ASCII85Decode" | b"A85" => ascii85(&out)?,
            b"ASCIIHexDecode" | b"AHx" => ascii_hex(&out),
            b"RunLengthDecode" | b"RL" => run_length(&out),
            other => {
                return Err(FilterError::Unsupported(
                    String::from_utf8_lossy(other).into_owned(),
                ));
            }
        };
    }
    Ok(out)
}

fn ascii85(data: &[u8]) -> Result<Vec<u8>, FilterError> {
    let mut out = Vec::with_capacity(data.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0;
    let body = data.strip_prefix(b"<~").unwrap_or(data);
    for &b in body {
        match b {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = b - b'!';
                n += 1;
                if n == 5 {
                    let v = group.iter().fold(0u64, |a, d| a * 85 + u64::from(*d));
                    let v = u32::try_from(v).map_err(|_| FilterError::Corrupt)?;
                    out.extend_from_slice(&v.to_be_bytes());
                    n = 0;
                }
            }
            _ if crate::parse::is_white(b) => {}
            _ => return Err(FilterError::Corrupt),
        }
    }
    if n > 1 {
        for d in group.iter_mut().skip(n) {
            *d = 84;
        }
        let v = group.iter().fold(0u64, |a, d| a * 85 + u64::from(*d));
        let v = u32::try_from(v).map_err(|_| FilterError::Corrupt)?;
        out.extend_from_slice(&v.to_be_bytes()[..n - 1]);
    }
    Ok(out)
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = data
        .iter()
        .take_while(|b| **b != b'>')
        .filter_map(|b| (*b as char).to_digit(16).map(|d| d as u8))
        .collect();
    digits
        .chunks(2)
        .map(|c| c[0] << 4 | c.get(1).copied().unwrap_or(0))
        .collect()
}

fn run_length(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let n = data[i] as usize;
        i += 1;
        match n {
            128 => break,
            0..=127 => {
                let end = (i + n + 1).min(data.len());
                out.extend_from_slice(&data[i..end]);
                i = end;
            }
            _ => {
                if let Some(b) = data.get(i) {
                    out.extend(std::iter::repeat_n(*b, 257 - n));
                }
                i += 1;
            }
        }
    }
    out
}

fn lzw(data: &[u8], early: bool) -> Result<Vec<u8>, FilterError> {
    let mut out: Vec<u8> = Vec::new();
    let mut table: Vec<Vec<u8>> = Vec::new();
    let reset = |table: &mut Vec<Vec<u8>>| {
        table.clear();
        table.extend((0..=255u8).map(|b| vec![b]));
        // 256 clears, 257 ends.
        table.push(Vec::new());
        table.push(Vec::new());
    };
    reset(&mut table);
    let mut width = 9;
    let mut prev: Option<Vec<u8>> = None;
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = acc << 8 | u32::from(b);
        bits += 8;
        while bits >= width {
            let code = (acc >> (bits - width)) as usize & ((1 << width) - 1);
            bits -= width;
            match code {
                256 => {
                    reset(&mut table);
                    width = 9;
                    prev = None;
                    continue;
                }
                257 => return Ok(out),
                _ => {}
            }
            let entry = if code < table.len() {
                table[code].clone()
            } else if let Some(p) = &prev {
                let mut e = p.clone();
                e.push(p[0]);
                e
            } else {
                return Err(FilterError::Corrupt);
            };
            out.extend_from_slice(&entry);
            if out.len() as u64 > MAX_DECODED {
                return Err(FilterError::TooLarge);
            }
            if let Some(p) = prev.take() {
                let mut e = p;
                e.push(entry[0]);
                table.push(e);
            }
            prev = Some(entry);
            let next = table.len() + usize::from(early);
            width = match next {
                0..512 => 9,
                512..1024 => 10,
                1024..2048 => 11,
                _ => 12,
            };
        }
    }
    Ok(out)
}

/// zlib, falling back to raw deflate; a truncated stream keeps what was
/// decoded, as other readers do.
fn inflate(data: &[u8], whole: bool) -> Result<Vec<u8>, FilterError> {
    if whole {
        return inflate_whole(data);
    }
    fn run(reader: impl std::io::Read) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        let ok = reader.take(MAX_DECODED + 1).read_to_end(&mut out).is_ok();
        (out, ok)
    }
    let (out, ok) = run(flate2::read::ZlibDecoder::new(data));
    let (out, ok) = if ok || !out.is_empty() {
        (out, ok)
    } else {
        run(flate2::read::DeflateDecoder::new(data))
    };
    if out.len() as u64 > MAX_DECODED {
        return Err(FilterError::TooLarge);
    }
    if !ok && (out.is_empty() || whole) {
        return Err(FilterError::Corrupt);
    }
    Ok(out)
}

/// A zlib stream inflated to its proper end, its checksum right.
fn inflate_whole(data: &[u8]) -> Result<Vec<u8>, FilterError> {
    let mut z = flate2::Decompress::new(true);
    let mut out = Vec::with_capacity(data.len() * 4);
    loop {
        if out.len() == out.capacity() {
            out.reserve(out.len().max(64 * 1024));
        }
        let read = z.total_in() as usize;
        let status = z
            .decompress_vec(&data[read..], &mut out, flate2::FlushDecompress::None)
            .map_err(|_| FilterError::Corrupt)?;
        if out.len() as u64 > MAX_DECODED {
            return Err(FilterError::TooLarge);
        }
        match status {
            flate2::Status::StreamEnd => return Ok(out),
            // No progress with all the input read: cut short.
            _ if z.total_in() as usize == read && out.len() < out.capacity() => {
                return Err(FilterError::Corrupt);
            }
            _ => {}
        }
    }
}

/// Undoes a PNG (`/Predictor` ≥ 10) or TIFF (2) predictor.
fn predict(params: Option<&Dict>, data: Vec<u8>) -> Result<Vec<u8>, FilterError> {
    let Some(p) = params else { return Ok(data) };
    let predictor = p.int("Predictor").unwrap_or(1);
    if predictor < 2 {
        return Ok(data);
    }
    let colors = p.int("Colors").unwrap_or(1).clamp(1, 32) as usize;
    let bpc = p.int("BitsPerComponent").unwrap_or(8).clamp(1, 16) as usize;
    let columns = p.int("Columns").unwrap_or(1).clamp(1, 1 << 20) as usize;
    let bpp = (colors * bpc).div_ceil(8);
    let row = (colors * bpc * columns).div_ceil(8);
    if predictor == 2 {
        if bpc != 8 {
            return Err(FilterError::Unsupported(
                "TIFF predictor below 8 bits".into(),
            ));
        }
        let mut out = data;
        for line in out.chunks_mut(row) {
            for i in bpp..line.len() {
                line[i] = line[i].wrapping_add(line[i - bpp]);
            }
        }
        return Ok(out);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row];
    for chunk in data.chunks(row + 1) {
        let (kind, src) = (chunk[0], &chunk[1..]);
        let mut line = vec![0u8; row];
        line[..src.len()].copy_from_slice(src);
        for i in 0..row {
            let left = if i >= bpp { line[i - bpp] } else { 0 };
            let up = prev[i];
            let up_left = if i >= bpp { prev[i - bpp] } else { 0 };
            line[i] = match kind {
                0 => line[i],
                1 => line[i].wrapping_add(left),
                2 => line[i].wrapping_add(up),
                3 => line[i].wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8),
                4 => line[i].wrapping_add(paeth(left, up, up_left)),
                _ => return Err(FilterError::Corrupt),
            };
        }
        out.extend_from_slice(&line[..src.len()]);
        prev = line;
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn flate_round_trip() {
        let mut d = Dict::new();
        d.set("Filter", Object::name("FlateDecode"));
        assert_eq!(decode(&d, &zlib(b"hello")).unwrap(), b"hello");
    }

    #[test]
    fn png_up_predictor() {
        // Two rows of 3 bytes, the second encoded "Up".
        let raw = [2, 1, 2, 3, 2, 1, 1, 1];
        let mut parms = Dict::new();
        parms.set("Predictor", 12);
        parms.set("Columns", 3);
        let mut d = Dict::new();
        d.set("Filter", Object::name("FlateDecode"));
        d.set("DecodeParms", parms);
        assert_eq!(decode(&d, &zlib(&raw)).unwrap(), [1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn ascii_filters() {
        assert_eq!(ascii85(b"<~87cURD]i,\"Ebo7~>").unwrap(), b"Hello World");
        assert_eq!(ascii85(b"z~>").unwrap(), [0; 4]);
        assert_eq!(ascii_hex(b"48 65 6c6C6>"), b"Hell`");
        assert_eq!(
            run_length(&[2, b'a', b'b', b'c', 254, b'x', 128]),
            b"abcxxx"
        );
    }

    #[test]
    fn lzw_example() {
        // ISO 32000-1 §7.4.4.2's example.
        let data = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw(&data, true).unwrap(), b"-----A---B");
    }

    #[test]
    fn unsupported_filter_is_reported() {
        let mut d = Dict::new();
        d.set("Filter", Object::name("DCTDecode"));
        assert!(matches!(decode(&d, b""), Err(FilterError::Unsupported(_))));
    }
}
