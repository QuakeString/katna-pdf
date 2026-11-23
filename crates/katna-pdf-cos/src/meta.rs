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
    /// What the document says about itself: `/Info`, with what it lacks
    /// taken from the XMP packet. A locked file's `/Info` can't be read
    /// without its password, so only a plain XMP packet speaks for it.
    pub fn metadata(&self) -> Metadata {
        let xmp = self.xmp_metadata().unwrap_or_default();
        if self.is_locked() {
            return xmp;
        }
        let mut meta = self.info_metadata();
        let fill = |mine: &mut Option<String>, theirs: Option<String>| {
            if mine.is_none() {
                *mine = theirs;
            }
        };
        fill(&mut meta.title, xmp.title);
        fill(&mut meta.author, xmp.author);
        fill(&mut meta.subject, xmp.subject);
        fill(&mut meta.keywords, xmp.keywords);
        fill(&mut meta.creator, xmp.creator);
        fill(&mut meta.producer, xmp.producer);
        fill(&mut meta.created, xmp.created);
        fill(&mut meta.modified, xmp.modified);
        meta
    }

    /// Whether the file's lock leaves its XMP packet plain
    /// (`/EncryptMetadata false`), readable without the password.
    pub fn metadata_plain(&self) -> bool {
        match self.trailer().get("Encrypt").map(|e| self.resolve(e)) {
            Some(Object::Dict(d)) => matches!(d.get("EncryptMetadata"), Some(Object::Bool(false))),
            _ => false,
        }
    }

    /// What the XMP packet (the catalog's `/Metadata`) says, when it can
    /// be read: once the file is open, or while it is locked when its
    /// lock left the packet plain.
    pub fn xmp_metadata(&self) -> Option<Metadata> {
        if self.is_locked() && !self.metadata_plain() {
            return None;
        }
        let Some(Object::Dict(catalog)) = self.trailer().get("Root").map(|r| self.resolve(r))
        else {
            return None;
        };
        let Object::Stream(s) = self.resolve_key(&catalog, "Metadata") else {
            return None;
        };
        let data = crate::filter::decode(&s.dict, &s.data).ok()?;
        Some(read_xmp(&String::from_utf8_lossy(&data)))
    }

    fn info_metadata(&self) -> Metadata {
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

/// The fields of an XMP packet.
fn read_xmp(xml: &str) -> Metadata {
    let first = |name: &str| xmp_values(xml, name).into_iter().next();
    let creators = xmp_values(xml, "dc:creator");
    let date = |name: &str| first(name).as_deref().and_then(pdf_date_of);
    Metadata {
        title: first("dc:title"),
        author: (!creators.is_empty()).then(|| creators.join("; ")),
        subject: first("dc:description"),
        keywords: first("pdf:Keywords"),
        creator: first("xmp:CreatorTool"),
        producer: first("pdf:Producer"),
        created: date("xmp:CreateDate"),
        modified: date("xmp:ModifyDate"),
    }
}

/// The values of property `name` in `xml`: an attribute, the text of an
/// element, or each `rdf:li` of its list. Blank ones are left out.
fn xmp_values(xml: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let attribute = format!("{name}=");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&attribute).map(|a| a + from) {
        from = at + attribute.len();
        // Not the end of a longer name.
        if xml[..at].ends_with(|c: char| c.is_alphanumeric() || c == ':') {
            continue;
        }
        let rest = &xml[from..];
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        if let Some(end) = rest[1..].find(quote) {
            out.push(unescape(&rest[1..1 + end]));
        }
    }
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&open).map(|a| a + from) {
        let after = &xml[at + open.len()..];
        from = at + open.len();
        if !after.starts_with(['>', ' ', '\t', '\n', '\r']) {
            continue;
        }
        let Some(start) = after.find('>') else { break };
        if after[..start].ends_with('/') {
            continue;
        }
        let body = &after[start + 1..];
        let Some(end) = body.find(&close) else { break };
        let body = &body[..end];
        if body.contains("<rdf:li") {
            let mut rest = body;
            while let Some(li) = rest.find("<rdf:li") {
                let item = &rest[li..];
                let Some(gt) = item.find('>') else { break };
                let text = &item[gt + 1..];
                let Some(stop) = text.find("</rdf:li>") else {
                    break;
                };
                out.push(unescape(&text[..stop]));
                rest = &text[stop..];
            }
        } else if !body.contains('<') {
            out.push(unescape(body));
        }
    }
    out.retain(|v| !v.trim().is_empty());
    out.iter_mut().for_each(|v| *v = v.trim().to_owned());
    out
}

/// XML text with its entities read.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let entity = &tail[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => out.push(c),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// An XMP date (`2026-10-19T10:30:00+05:30`) as a PDF date
/// (`D:20261019103000+05'30'`).
fn pdf_date_of(xmp: &str) -> Option<String> {
    let xmp = xmp.trim();
    let (when, zone) = match xmp
        .find(['Z', '+'])
        .or_else(|| xmp.get(10..).and_then(|t| t.find('-').map(|i| i + 10)))
    {
        Some(at) => (&xmp[..at], &xmp[at..]),
        None => (xmp, ""),
    };
    let digits: String = when.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 4 {
        return None;
    }
    let zone = match zone {
        "" => String::new(),
        "Z" => "Z".to_owned(),
        z => {
            let (sign, hm) = z.split_at(1);
            let (h, m) = hm.split_once(':').unwrap_or((hm, "00"));
            format!("{sign}{h}'{m}'")
        }
    };
    Some(format!("D:{digits}{zone}"))
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
pub(crate) fn xmp(meta: &Metadata, created: Option<&str>, modified: &str, old: &str) -> String {
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

    #[test]
    fn reads_an_xmp_packet() {
        let xml = r#"<x:xmpmeta><rdf:RDF><rdf:Description rdf:about=""
            pdf:Producer="Acrobat &amp; co" xmp:CreateDate="2026-10-19T10:30:00+05:30">
           <dc:title><rdf:Alt><rdf:li xml:lang="x-default">Rent &lt;2026&gt;</rdf:li></rdf:Alt></dc:title>
           <dc:creator><rdf:Seq><rdf:li>Asha</rdf:li><rdf:li>Ravi</rdf:li></rdf:Seq></dc:creator>
           <pdf:Keywords>lease, flat</pdf:Keywords>
           <xmp:CreatorTool/>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let m = read_xmp(xml);
        assert_eq!(m.title.as_deref(), Some("Rent <2026>"));
        assert_eq!(m.author.as_deref(), Some("Asha; Ravi"));
        assert_eq!(m.keywords.as_deref(), Some("lease, flat"));
        assert_eq!(m.producer.as_deref(), Some("Acrobat & co"));
        assert_eq!(m.created.as_deref(), Some("D:20261019103000+05'30'"));
        assert_eq!(m.creator, None);
        // What this crate writes reads back.
        let back = read_xmp(&xmp(&m, m.created.as_deref(), "D:20261020000000Z", ""));
        assert_eq!(back.title, m.title);
        assert_eq!(back.author, m.author);
    }
}
