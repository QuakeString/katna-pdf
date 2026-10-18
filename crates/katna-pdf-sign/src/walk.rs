// SPDX-License-Identifier: GPL-3.0-or-later

//! A walk over DER as it is written, for checking signatures made by
//! other software byte for byte. Definite lengths and one-byte tags
//! only, which is all CMS signatures in PDFs use.

use const_oid::ObjectIdentifier;

/// One element: its tag, its contents, and all its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    pub raw: &'a [u8],
}

/// The element at the start of `input`, and what follows it.
pub fn read(input: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let tag = *input.first()?;
    if tag & 0x1f == 0x1f {
        return None;
    }
    let first = *input.get(1)?;
    let (len, header) = if first & 0x80 == 0 {
        (first as usize, 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let bytes = input.get(2..2 + n)?;
        (
            bytes.iter().fold(0usize, |acc, &b| acc << 8 | b as usize),
            2 + n,
        )
    };
    let end = header.checked_add(len)?;
    let raw = input.get(..end)?;
    Some((
        Tlv {
            tag,
            value: &raw[header..],
            raw,
        },
        &input[end..],
    ))
}

/// The elements one after another in `input`.
pub fn children(mut input: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let (tlv, rest) = read(input)?;
        out.push(tlv);
        input = rest;
    }
    Some(out)
}

pub fn oid(tlv: &Tlv<'_>) -> Option<ObjectIdentifier> {
    (tlv.tag == 0x06)
        .then(|| ObjectIdentifier::from_bytes(tlv.value).ok())
        .flatten()
}

/// The algorithm an AlgorithmIdentifier names.
pub fn algorithm(tlv: &Tlv<'_>) -> Option<ObjectIdentifier> {
    if tlv.tag != 0x30 {
        return None;
    }
    oid(children(tlv.value)?.first()?)
}

pub fn uint(tlv: &Tlv<'_>) -> Option<usize> {
    (tlv.tag == 0x02 && !tlv.value.is_empty() && tlv.value.len() <= 4).then(|| {
        tlv.value
            .iter()
            .fold(0usize, |acc, &b| acc << 8 | b as usize)
    })
}

/// A UTCTime or GeneralizedTime as `YYYY-MM-DD HH:MM:SS UTC`.
pub fn time(tlv: &Tlv<'_>) -> Option<String> {
    let text = std::str::from_utf8(tlv.value).ok()?;
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    let full = match (tlv.tag, digits.len()) {
        // UTCTime: two-digit years, 1950 to 2049.
        (0x17, 12) => {
            let yy: u32 = digits[..2].parse().ok()?;
            format!("{}{digits}", if yy >= 50 { 19 } else { 20 })
        }
        (0x18, 14) => digits,
        _ => return None,
    };
    Some(format!(
        "{}-{}-{} {}:{}:{} UTC",
        &full[..4],
        &full[4..6],
        &full[6..8],
        &full[8..10],
        &full[10..12],
        &full[12..14]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elements_are_read_as_written() {
        // SEQUENCE { INTEGER 5, OCTET STRING of 200 bytes }
        let mut der = vec![0x30, 0x81, 0xce, 0x02, 0x01, 0x05, 0x04, 0x81, 0xc8];
        der.extend([7; 200]);
        let (seq, rest) = read(&der).unwrap();
        assert!(rest.is_empty());
        let kids = children(seq.value).unwrap();
        assert_eq!(uint(&kids[0]), Some(5));
        assert_eq!(kids[1].value.len(), 200);
        // Cut short, or a length past the end.
        assert!(read(&der[..20]).is_none());
        assert!(read(&[0x04, 0x05, 1]).is_none());
        let t = [
            0x17, 13, b'2', b'6', b'1', b'0', b'1', b'8', b'1', b'2', b'0', b'0', b'0', b'0', b'Z',
        ];
        assert_eq!(
            time(&read(&t).unwrap().0).unwrap(),
            "2026-10-18 12:00:00 UTC"
        );
    }
}
