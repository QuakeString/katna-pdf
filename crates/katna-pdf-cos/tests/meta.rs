// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading and changing what a document says about itself.

use katna_pdf_cos::{Document, Object, Stream, decode, text_pdf};

fn report() -> Document {
    Document::load(text_pdf("Quarterly report", &["  Sales rose.".to_string()])).unwrap()
}

#[test]
fn properties_are_read_and_changed() {
    let doc = report();
    let meta = doc.metadata();
    assert_eq!(meta.title.as_deref(), Some("Quarterly report"));
    let mut new = meta.clone();
    new.title = Some("Rapport trimestriel – 2026".into());
    new.author = Some("Asha Rao".into());
    new.keywords = Some("sales, 2026".into());
    let out = Document::load(doc.set_metadata(&new).unwrap()).unwrap();
    assert!(out.bytes().starts_with(doc.bytes()));
    let read = out.metadata();
    assert_eq!(read.title, new.title);
    assert_eq!(read.author.as_deref(), Some("Asha Rao"));
    assert_eq!(read.keywords.as_deref(), Some("sales, 2026"));
    assert!(read.modified.as_deref().unwrap().starts_with("D:20"));
    // A blank field is taken out.
    let mut blank = read.clone();
    blank.author = Some("  ".into());
    let out = Document::load(out.set_metadata(&blank).unwrap()).unwrap();
    assert_eq!(out.metadata().author, None);
    assert_eq!(out.metadata().title, new.title);
}

#[test]
fn the_xmp_copy_says_the_same_and_keeps_pdfa() {
    let doc = report();
    // Give the file an XMP packet claiming PDF/A-2b.
    let mut update = doc.update().unwrap();
    let old = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" pdfaid:part="2" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
    let mut dict = katna_pdf_cos::Dict::new();
    dict.set("Type", Object::name("Metadata"));
    dict.set("Subtype", Object::name("XML"));
    let xmp = update.add(Object::Stream(Stream {
        dict,
        data: old.to_vec(),
    }));
    let Some(Object::Ref(root)) = doc.trailer().get("Root").cloned() else {
        panic!("no catalog");
    };
    let mut catalog = doc.get(root).unwrap().as_dict().unwrap().clone();
    catalog.set("Metadata", Object::Ref(xmp));
    update.replace(root, Object::Dict(catalog));
    let doc = Document::load(update.finish()).unwrap();

    let mut meta = doc.metadata();
    meta.title = Some("Tom & Jerry <draft>".into());
    meta.author = Some("Asha".into());
    let out = Document::load(doc.set_metadata(&meta).unwrap()).unwrap();
    let Some(Object::Stream(s)) = out.get(xmp) else {
        panic!("no XMP");
    };
    let text = String::from_utf8(decode(&s.dict, &s.data).unwrap()).unwrap();
    assert!(
        text.contains(r#"<rdf:li xml:lang="x-default">Tom &amp; Jerry &lt;draft&gt;</rdf:li>"#),
        "{text}"
    );
    assert!(text.contains("<rdf:li>Asha</rdf:li>"), "{text}");
    assert!(text.contains("<pdfaid:part>2</pdfaid:part>"), "{text}");
    assert!(
        text.contains("<pdfaid:conformance>B</pdfaid:conformance>"),
        "{text}"
    );
    assert!(text.contains("<xmp:ModifyDate>20"), "{text}");
}
