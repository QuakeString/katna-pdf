// SPDX-License-Identifier: GPL-3.0-or-later

//! Decoding the stream filters the object layer itself needs: Flate with
//! PNG and TIFF predictors (xref and object streams). Page content and
//! images are the renderer's job.

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
                let raw = inflate(&out)?;
                predict(params.get(i).copied().flatten(), raw)?
            }
            other => {
                return Err(FilterError::Unsupported(
                    String::from_utf8_lossy(other).into_owned(),
                ));
            }
        };
    }
    Ok(out)
}

/// zlib, falling back to raw deflate; a truncated stream keeps what was
/// decoded, as other readers do.
fn inflate(data: &[u8]) -> Result<Vec<u8>, FilterError> {
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
    if !ok && out.is_empty() {
        return Err(FilterError::Corrupt);
    }
    Ok(out)
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
    fn unsupported_filter_is_reported() {
        let mut d = Dict::new();
        d.set("Filter", Object::name("DCTDecode"));
        assert!(matches!(decode(&d, b""), Err(FilterError::Unsupported(_))));
    }
}
