// SPDX-License-Identifier: GPL-3.0-or-later

//! Whole-file tests: classic tables, xref and object streams, repair, and
//! incremental updates.

use std::io::Write as _;

use katna_pdf_cos::{Dict, Document, Object, Ref, XrefKind};

/// A file with a classic table: catalog, page tree, `n` pages.
fn classic(n: usize) -> Vec<u8> {
    let mut objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!(
            "<< /Type /Pages /Count {n} /Kids [{}] >>",
            (0..n)
                .map(|i| format!("{} 0 R", 3 + i))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    ];
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] >>".into());
    }
    let mut out = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{o}\nendobj\n", i + 1).unwrap();
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).unwrap();
    for o in offsets {
        writeln!(out, "{o:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R /ID [<0102> <0102>] >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// A PDF 1.5 file: the page tree inside an object stream, found through a
/// compressed xref stream with a PNG predictor.
fn compressed() -> Vec<u8> {
    let mut out = b"%PDF-1.5\n".to_vec();
    let catalog_at = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    // Object stream 5 holds objects 2, 3, 4.
    let bodies = [
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
        "<< /Type /Page /Parent 2 0 R >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
    let mut header = String::new();
    let mut body = String::new();
    for (i, b) in bodies.iter().enumerate() {
        header.push_str(&format!("{} {} ", 2 + i, body.len()));
        body.push_str(b);
        body.push(' ');
    }
    let content = format!("{header}{body}");
    let packed = zlib(content.as_bytes());
    let objstm_at = out.len();
    write!(
        out,
        "5 0 obj\n<< /Type /ObjStm /N 3 /First {} /Filter /FlateDecode /Length {} >>\nstream\n",
        header.len(),
        packed.len()
    )
    .unwrap();
    out.extend_from_slice(&packed);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    // Xref stream 6, rows of W [1 4 2], PNG "Up" predictor.
    let xref_at = out.len();
    let rows: Vec<[u8; 7]> = vec![
        [0, 0, 0, 0, 0, 0xff, 0xff],
        row(1, catalog_at as u32, 0),
        row(2, 5, 0),
        row(2, 5, 1),
        row(2, 5, 2),
        row(1, objstm_at as u32, 0),
        row(1, xref_at as u32, 0),
    ];
    let mut raw = Vec::new();
    let mut prev = [0u8; 7];
    for r in &rows {
        raw.push(2);
        for i in 0..7 {
            raw.push(r[i].wrapping_sub(prev[i]));
        }
        prev = *r;
    }
    let packed = zlib(&raw);
    write!(
        out,
        "6 0 obj\n<< /Type /XRef /Size 7 /W [1 4 2] /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 7 >> /Length {} >>\nstream\n",
        packed.len()
    )
    .unwrap();
    out.extend_from_slice(&packed);
    write!(out, "\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").unwrap();
    out
}

fn row(kind: u8, field: u32, generation: u16) -> [u8; 7] {
    let f = field.to_be_bytes();
    let g = generation.to_be_bytes();
    [kind, f[0], f[1], f[2], f[3], g[0], g[1]]
}

#[test]
fn reads_a_classic_table() {
    let doc = Document::load(classic(3)).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Table);
    assert!(!doc.was_repaired());
    assert_eq!(doc.pages().len(), 3);
    assert_eq!(doc.size(), 6);
}

#[test]
fn reads_xref_and_object_streams() {
    let doc = Document::load(compressed()).unwrap();
    assert_eq!(doc.xref_kind(), XrefKind::Stream);
    assert!(!doc.was_repaired(), "{doc:?}");
    assert_eq!(doc.pages(), vec![Ref::new(3, 0), Ref::new(4, 0)]);
}

#[test]
fn repairs_wrong_offsets_and_a_missing_startxref() {
    // Shift every object by inserting bytes after the header.
    let mut shifted = classic(2);
    shifted.splice(9..9, b"junk junk\n".iter().copied());
    let doc = Document::load(shifted).unwrap();
    assert_eq!(doc.pages().len(), 2);

    let mut cut = classic(2);
    let at = cut.windows(9).rposition(|w| w == b"startxref").unwrap();
    cut.truncate(at);
    let doc = Document::load(cut).unwrap();
    assert!(doc.was_repaired());
    assert_eq!(doc.pages().len(), 2);
}

#[test]
fn rejects_what_is_not_a_pdf() {
    assert!(Document::load(b"hello".to_vec()).is_err());
    assert!(Document::load(Vec::new()).is_err());
}

#[test]
fn survives_loops_in_the_page_tree() {
    let pdf = classic(1);
    let text = String::from_utf8_lossy(&pdf).replace(
        "/Kids [3 0 R]",
        "/Kids [2 0 R]", // the tree contains itself
    );
    let doc = Document::load(text.into_bytes()).unwrap();
    assert!(doc.pages().len() <= 1);
}

fn update_round_trip(original: Vec<u8>) {
    let doc = Document::load(original.clone()).unwrap();
    let mut up = doc.update().unwrap();
    let mut info = Dict::new();
    info.set("Producer", Object::string("Katna PDF"));
    let info_ref = up.add(Object::Dict(info));
    up.trailer_mut().set("Info", info_ref);
    // Change the catalog too, as signing does (it adds /AcroForm).
    let root = doc.trailer().get("Root").and_then(Object::as_ref).unwrap();
    let mut catalog = doc.catalog().unwrap();
    catalog.set("PageMode", Object::name("UseOutlines"));
    up.replace(root, Object::Dict(catalog));
    let updated = up.finish();

    assert!(updated.starts_with(&original), "original bytes must stay");
    let again = Document::load(updated).unwrap();
    assert!(!again.was_repaired());
    assert_eq!(again.xref_kind(), doc.xref_kind());
    assert_eq!(again.pages(), doc.pages());
    assert_eq!(
        again.catalog().unwrap().name("PageMode"),
        Some(&b"UseOutlines"[..])
    );
    let info = again.resolve_key(again.trailer(), "Info");
    assert_eq!(
        info.as_dict().unwrap().get("Producer"),
        Some(&Object::string("Katna PDF"))
    );
    // And a second update on top of the first.
    let mut up = again.update().unwrap();
    up.add(Object::Int(7));
    let third = Document::load(up.finish()).unwrap();
    assert_eq!(third.pages(), doc.pages());
}

#[test]
fn incremental_update_on_a_classic_table() {
    update_round_trip(classic(2));
}

#[test]
fn incremental_update_on_an_xref_stream() {
    update_round_trip(compressed());
}

#[test]
fn update_keeps_the_first_id() {
    let doc = Document::load(classic(1)).unwrap();
    let up = doc.update().unwrap();
    let again = Document::load(up.finish()).unwrap();
    let Some(Object::Array(id)) = again.trailer().get("ID") else {
        panic!("no ID")
    };
    assert_eq!(
        id[0],
        Object::String {
            bytes: vec![1, 2],
            hex: true
        }
    );
    assert_ne!(id[1], id[0]);
}
