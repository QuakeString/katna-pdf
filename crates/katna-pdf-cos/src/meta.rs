// SPDX-License-Identifier: GPL-3.0-or-later

//! A document's properties (docs/PLAN.md §4.3, "metadata editor"): its
//! title, author, subject and keywords, read from the trailer's `/Info`
//! and written back there as an incremental update. A file that also
//! keeps them as XMP (the catalog's `/Metadata`, which readers prefer)
//! gets a new XMP packet saying the same, keeping its PDF/A claim.

use crate::annots::pdf_date;
use crate::document::Document;
use crate::forms::text_object;
use crate::object::{Dict, Name, Object, Stream};
use crate::outline::text_string;
use crate::write::UpdateError;

/// What a document says about itself. `None` is not set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    /// The program it was made in.
    pub creator: Option<String>,
    /// The program that made the PDF.
    pub producer: Option<String>,
    /// When it was made and last changed, as PDF dates (`D:2026…`).
    pub created: Option<String>,
    pub modified: Option<String>,
}

/// The `/Info` keys of the fields people change.
const KEYS: [&str; 6] = [
    "Title", "Author", "Subject", "Keywords", "Creator", "Producer",
];

impl Metadata {
    fn field(&self, key: &str) -> Option<&str> {
        match key {
            "Title" => self.title.as_deref(),
            "Author" => self.author.as_deref(),
            "Subject" => self.subject.as_deref(),
            "Keywords" => self.keywords.as_deref(),
            "Creator" => self.creator.as_deref(),
            "Producer" => self.producer.as_deref(),
            _ => None,
        }
    }
}

impl Document {
    /// What the document says about itself.
    pub fn metadata(&self) -> Metadata {
        let info = match self.trailer().get("Info").map(|i| self.resolve(i)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        let get = |key| match self.resolve_key(&info, key) {
            Object::String { bytes, .. } => {
                Some(text_string(&bytes)).filter(|s| !s.trim().is_empty())
            }
            _ => None,
        };
        Metadata {
            title: get("Title"),
            author: get("Author"),
            subject: get("Subject"),
            keywords: get("Keywords"),
            creator: get("Creator"),
            producer: get("Producer"),
            created: get("CreationDate"),
            modified: get("ModDate"),
        }
    }

    /// The file saying `meta` about itself; the time it was changed is
    /// now. Blank fields are taken out.
    pub fn set_metadata(&self, meta: &Metadata) -> Result<Vec<u8>, UpdateError> {
        let mut update = self.update()?;
        let info_ref = match self.trailer().get("Info") {
            Some(Object::Ref(r)) => Some(*r),
            _ => None,
        };
        let mut info = match self.trailer().get("Info").map(|i| self.resolve(i)) {
            Some(Object::Dict(d)) => d,
            _ => Dict::new(),
        };
        for key in KEYS {
            match meta.field(key).map(str::trim).filter(|v| !v.is_empty()) {
                Some(value) => info.set(key, text_object(value)),
                None => {
                    info.remove(key);
                }
            }
        }
        let now = pdf_date(std::time::SystemTime::now());
        info.set(
            "ModDate",
            Object::String {
                bytes: now.clone().into_bytes(),
                hex: false,
            },
        );
        match info_ref {
            Some(r) => update.replace(r, Object::Dict(info.clone())),
            None => {
                let r = update.add(Object::Dict(info.clone()));
                update.trailer_mut().set("Info", Object::Ref(r));
            }
        }
        // The XMP copy, when there is one.
        if let Some(Object::Ref(root)) = self.trailer().get("Root")
            && let Some(Object::Dict(catalog)) = self.get(*root)
            && let Some(Object::Ref(xmp_ref)) = catalog.get("Metadata")
        {
            let old = match self.get(*xmp_ref) {
                Some(Object::Stream(s)) => {
                    crate::filter::decode(&s.dict, &s.data).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            let created = match self.resolve_key(&info, "CreationDate") {
                Object::String { bytes, .. } => Some(text_string(&bytes)),
                _ => None,
            };
            let packet = xmp(
                meta,
                created.as_deref(),
                &now,
                &String::from_utf8_lossy(&old),
            );
            let dict = Dict(vec![
                (Name::new("Type"), Object::name("Metadata")),
                (Name::new("Subtype"), Object::name("XML")),
                (Name::new("Length"), Object::Int(packet.len() as i64)),
            ]);
            update.replace(
                *xmp_ref,
                Object::Stream(Stream {
                    dict,
                    data: packet.into_bytes(),
                }),
            );
        }
        Ok(update.finish())
    }
}

/// A PDF date (`D:20261019103000Z`, or with `+05'30'`) as XMP writes
/// dates (`2026-10-19T10:30:00Z`).
fn xmp_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let digits = |from: usize, len: usize| -> Option<&str> {
        d.get(from..from + len)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
    };
    let year = digits(0, 4)?;
    let part = |from| digits(from, 2).unwrap_or(if from < 8 { "01" } else { "00" });
    let zone = match d.get(14..15) {
        Some("+") | Some("-") => {
            let sign = &d[14..15];
            let hours = d.get(15..17).unwrap_or("00");
            let minutes = d.get(18..20).unwrap_or("00");
            format!("{sign}{hours}:{minutes}")
        }
        _ => "Z".into(),
    };
    Some(format!(
        "{year}-{}-{}T{}:{}:{}{zone}",
        part(4),
        part(6),
        part(8),
        part(10),
        part(12)
    ))
}

/// Text made safe inside XML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The value of `name="…"` or `<name>…</name>` in old XMP.
fn old_value(old: &str, name: &str) -> Option<String> {
    let attribute = format!("{name}=\"");
    if let Some(at) = old.find(&attribute) {
        let rest = &old[at + attribute.len()..];
        return rest.find('"').map(|end| rest[..end].to_owned());
    }
    let open = format!("<{name}>");
    let at = old.find(&open)?;
    let rest = &old[at + open.len()..];
    rest.find('<').map(|end| rest[..end].trim().to_owned())
}

/// A whole XMP packet saying `meta`.
fn xmp(meta: &Metadata, created: Option<&str>, modified: &str, old: &str) -> String {
    let mut fields = String::new();
    let mut alt = |name: &str, value: Option<&str>| {
        if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
            fields.push_str(&format!(
                "   <dc:{name}><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:{name}>\n",
                escape(v)
            ));
        }
    };
    alt("title", meta.title.as_deref());
    alt("description", meta.subject.as_deref());
    if let Some(author) = meta
        .author
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        fields.push_str(&format!(
            "   <dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n",
            escape(author)
        ));
    }
    let mut simple = |name: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            fields.push_str(&format!("   <{name}>{}</{name}>\n", escape(v.trim())));
        }
    };
    simple("pdf:Keywords", meta.keywords.clone());
    simple("pdf:Producer", meta.producer.clone());
    simple("xmp:CreatorTool", meta.creator.clone());
    simple("xmp:CreateDate", created.and_then(xmp_date));
    simple("xmp:ModifyDate", xmp_date(modified));
    simple("xmp:MetadataDate", xmp_date(modified));
    // A PDF/A file keeps saying so.
    simple("pdfaid:part", old_value(old, "pdfaid:part"));
    simple("pdfaid:conformance", old_value(old, "pdfaid:conformance"));
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
         \x20<rdf:Description rdf:about=\"\"\n\
         \x20  xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
         \x20  xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
         \x20  xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
         \x20  xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\">\n\
         {fields}\
         \x20</rdf:Description>\n\
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_turn_into_xmp_dates() {
        assert_eq!(
            xmp_date("D:20261019103005Z").as_deref(),
            Some("2026-10-19T10:30:05Z")
        );
        assert_eq!(
            xmp_date("D:20261019103005+05'30'").as_deref(),
            Some("2026-10-19T10:30:05+05:30")
        );
        assert_eq!(xmp_date("D:2026").as_deref(), Some("2026-01-01T00:00:00Z"));
        assert_eq!(xmp_date("yesterday"), None);
    }

    #[test]
    fn old_values_are_found() {
        let old = r#"<rdf:Description pdfaid:part="2" xmlns:pdfaid="x"><pdfaid:conformance>B</pdfaid:conformance>"#;
        assert_eq!(old_value(old, "pdfaid:part").as_deref(), Some("2"));
        assert_eq!(old_value(old, "pdfaid:conformance").as_deref(), Some("B"));
        assert_eq!(old_value(old, "pdf:Keywords"), None);
    }
}
