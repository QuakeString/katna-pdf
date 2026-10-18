// SPDX-License-Identifier: GPL-3.0-or-later

//! Content streams (ISO 32000-2 §7.8.2): the operators that draw a page,
//! each with its operands and where it sits in the stream's bytes, so an
//! edit can replace one operator and leave every other byte as it was.

use std::ops::Range;

use crate::object::Object;
use crate::parse::{Parser, is_white};
use crate::write::write_object;

/// Deepest nesting of arrays and dictionaries read in an operand.
const MAX_DEPTH: usize = 32;

/// One operator and its operands.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    pub operands: Vec<Object>,
    /// The operator, such as `Tj`. An inline image is one `BI` op whose
    /// span runs to its `EI`.
    pub operator: Vec<u8>,
    /// Where it is in the stream, its operands included.
    pub span: Range<usize>,
}

impl Op {
    pub fn new(operator: &str, operands: Vec<Object>) -> Self {
        Op {
            operands,
            operator: operator.as_bytes().to_vec(),
            span: 0..0,
        }
    }

    pub fn is(&self, operator: &str) -> bool {
        self.operator == operator.as_bytes()
    }

    /// Operand `i` as a number.
    pub fn number(&self, i: usize) -> Option<f64> {
        self.operands.get(i).and_then(Object::as_number)
    }
}

/// The operators of `data`, in order. Bytes that are not an operator or
/// an operand are skipped, as readers do.
pub fn ops(data: &[u8]) -> Vec<Op> {
    let mut p = Parser::new(data, 0);
    let mut out = Vec::new();
    let mut operands = Vec::new();
    let mut start = None;
    loop {
        p.skip_white();
        if p.pos >= data.len() {
            break;
        }
        let at = p.pos;
        if let Some(obj) = p.object(0) {
            start.get_or_insert(at);
            operands.push(obj);
            continue;
        }
        if p.pos != at {
            // A stray delimiter, consumed.
            continue;
        }
        let token = p.token();
        if token.is_empty() {
            p.pos += 1;
            continue;
        }
        let begin = start.take().unwrap_or(at);
        if token == b"BI" {
            let end = inline_image_end(data, p.pos);
            p.pos = end;
            operands.clear();
            out.push(Op {
                operands: Vec::new(),
                operator: token.to_vec(),
                span: begin..end,
            });
            continue;
        }
        out.push(Op {
            operands: std::mem::take(&mut operands),
            operator: token.to_vec(),
            span: begin..p.pos,
        });
    }
    out
}

/// Where the inline image whose `BI` ends at `pos` ends: just after its
/// `EI`, or at the end of the stream.
fn inline_image_end(data: &[u8], pos: usize) -> usize {
    let mut p = Parser::new(data, pos);
    // The image's dictionary, up to `ID`.
    loop {
        p.skip_white();
        if p.pos >= data.len() {
            return data.len();
        }
        let at = p.pos;
        if p.object(MAX_DEPTH - 1).is_some() {
            continue;
        }
        if p.pos != at {
            continue;
        }
        let token = p.token();
        if token == b"ID" {
            break;
        }
        if token.is_empty() {
            p.pos += 1;
        }
    }
    // One white-space byte, then the data, then `EI` standing alone.
    let mut i = p.pos + 1;
    while i + 1 < data.len() {
        if &data[i..i + 2] == b"EI"
            && data.get(i.wrapping_sub(1)).is_some_and(|b| is_white(*b))
            && data.get(i + 2).is_none_or(|b| is_white(*b))
        {
            return i + 2;
        }
        i += 1;
    }
    data.len()
}

/// `op` written out, operands first.
pub fn write_op(out: &mut Vec<u8>, op: &Op) {
    for operand in &op.operands {
        write_object(out, operand);
        out.push(b' ');
    }
    out.extend_from_slice(&op.operator);
}

/// `data` with each span replaced by its bytes. Spans must not overlap.
pub fn splice(data: &[u8], mut edits: Vec<(Range<usize>, Vec<u8>)>) -> Vec<u8> {
    edits.sort_by_key(|(span, _)| span.start);
    let mut out = Vec::with_capacity(data.len());
    let mut at = 0;
    for (span, bytes) in edits {
        if span.start < at {
            continue;
        }
        out.extend_from_slice(&data[at..span.start]);
        out.extend_from_slice(&bytes);
        at = span.end;
    }
    out.extend_from_slice(&data[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_operators_and_spans() {
        let data = b"q 1 0 0 1 72 720 cm BT /F1 12 Tf (Hi \\(x\\)) Tj [(A) -120 (B)] TJ ET Q";
        let ops = ops(data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.operator.as_slice()).collect();
        assert_eq!(
            names,
            [&b"q"[..], b"cm", b"BT", b"Tf", b"Tj", b"TJ", b"ET", b"Q"]
        );
        assert_eq!(ops[1].operands.len(), 6);
        assert_eq!(&data[ops[4].span.clone()], b"(Hi \\(x\\)) Tj");
        assert_eq!(&data[ops[5].span.clone()], b"[(A) -120 (B)] TJ");
    }

    #[test]
    fn skips_inline_images() {
        let data = b"q BI /W 2 /H 1 /BPC 8 /CS /G ID \x00EI\xff EI Q (x) Tj";
        let ops = ops(data);
        let names: Vec<&[u8]> = ops.iter().map(|o| o.operator.as_slice()).collect();
        assert_eq!(names, [&b"q"[..], b"BI", b"Q", b"Tj"]);
    }

    #[test]
    fn splices_spans() {
        let data = b"(a) Tj (b) Tj";
        let ops = ops(data);
        let out = splice(data, vec![(ops[1].span.clone(), b"(c) Tj".to_vec())]);
        assert_eq!(out, b"(a) Tj (c) Tj");
    }
}
