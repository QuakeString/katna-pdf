// SPDX-License-Identifier: GPL-3.0-or-later

//! Signing PDFs and checking their signatures, in pure Rust (docs/PLAN.md
//! §4.4): PAdES B-B signatures (ETSI.CAdES.detached CMS) made with a key
//! from a certificate file or, later, a token, and the signatures a file
//! holds checked for integrity and by their signer's key.
//!
//! The PDF side (signature fields, byte ranges) is `katna-pdf-cos`'s;
//! this crate makes and reads the CMS. Trust in the signer (the chain up
//! to a root such as CCA India's, revocation) is not checked yet.

mod cms;
mod keys;
mod oid;
pub mod p12;
pub mod pkcs11;
pub mod trust;
pub mod tsa;
mod walk;

use katna_pdf_cos::{Coverage, Document, SignError, SignRequest, Signature};

pub use crate::cms::{CheckError, Checked, Stamp, common_name};
pub use crate::keys::{KeyKind, Signer, SignerError, SoftKey};
pub use crate::p12::{Identity, P12Error};
pub use crate::trust::{Roots, Trust};
pub use x509_cert::Certificate;

/// The digest a signature is made with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Digest {
    #[default]
    Sha256,
    Sha384,
    Sha512,
}

impl Digest {
    fn oid(self) -> const_oid::ObjectIdentifier {
        match self {
            Digest::Sha256 => oid::SHA256,
            Digest::Sha384 => oid::SHA384,
            Digest::Sha512 => oid::SHA512,
        }
    }

    fn hash(self, parts: &[&[u8]]) -> Vec<u8> {
        Hash::from(self).hash(parts)
    }
}

/// The digests signatures are checked with: SHA-1 too, for old ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hash {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl From<Digest> for Hash {
    fn from(d: Digest) -> Self {
        match d {
            Digest::Sha256 => Hash::Sha256,
            Digest::Sha384 => Hash::Sha384,
            Digest::Sha512 => Hash::Sha512,
        }
    }
}

impl Hash {
    fn from_oid(oid: const_oid::ObjectIdentifier) -> Option<Self> {
        Some(match oid {
            o if o == oid::SHA1 => Hash::Sha1,
            o if o == oid::SHA256 => Hash::Sha256,
            o if o == oid::SHA384 => Hash::Sha384,
            o if o == oid::SHA512 => Hash::Sha512,
            _ => return None,
        })
    }

    fn hash(self, parts: &[&[u8]]) -> Vec<u8> {
        fn all<D: sha2::Digest>(parts: &[&[u8]]) -> Vec<u8> {
            let mut d = D::new();
            for p in parts {
                d.update(p);
            }
            d.finalize().to_vec()
        }
        match self {
            Hash::Sha1 => all::<sha1::Sha1>(parts),
            Hash::Sha256 => all::<sha2::Sha256>(parts),
            Hash::Sha384 => all::<sha2::Sha384>(parts),
            Hash::Sha512 => all::<sha2::Sha512>(parts),
        }
    }

    fn pkcs1v15(self) -> rsa::Pkcs1v15Sign {
        match self {
            Hash::Sha1 => rsa::Pkcs1v15Sign::new::<sha1::Sha1>(),
            Hash::Sha256 => rsa::Pkcs1v15Sign::new::<sha2::Sha256>(),
            Hash::Sha384 => rsa::Pkcs1v15Sign::new::<sha2::Sha384>(),
            Hash::Sha512 => rsa::Pkcs1v15Sign::new::<sha2::Sha512>(),
        }
    }

    fn pss(self, salt: usize) -> rsa::Pss {
        match self {
            Hash::Sha1 => rsa::Pss::new_with_salt::<sha1::Sha1>(salt),
            Hash::Sha256 => rsa::Pss::new_with_salt::<sha2::Sha256>(salt),
            Hash::Sha384 => rsa::Pss::new_with_salt::<sha2::Sha384>(salt),
            Hash::Sha512 => rsa::Pss::new_with_salt::<sha2::Sha512>(salt),
        }
    }
}

/// Why a file could not be signed.
#[derive(Debug)]
pub enum SigningError {
    Pdf(SignError),
    Signer(SignerError),
    Timestamp(tsa::TsaError),
    Encoding(String),
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SigningError::Pdf(e) => e.fmt(f),
            SigningError::Signer(e) => e.fmt(f),
            SigningError::Timestamp(e) => e.fmt(f),
            SigningError::Encoding(e) => write!(f, "the signature could not be written: {e}"),
        }
    }
}

impl std::error::Error for SigningError {}

/// `doc` signed by `signer` as `request` says, with `digest`: the file
/// with the signature appended as an incremental update (PAdES B-B).
pub fn sign(
    doc: &Document,
    request: &SignRequest,
    signer: &dyn Signer,
    digest: Digest,
) -> Result<Vec<u8>, SigningError> {
    sign_with(doc, request, signer, digest, None)
}

/// [`sign`], and the signature timestamped by `stamper` when given
/// (PAdES B-T).
pub fn sign_with(
    doc: &Document,
    request: &SignRequest,
    signer: &dyn Signer,
    digest: Digest,
    stamper: Option<&dyn tsa::Stamper>,
) -> Result<Vec<u8>, SigningError> {
    let encoding = |e: der::Error| SigningError::Encoding(e.to_string());
    let prepared = doc.prepare_signature(request).map_err(SigningError::Pdf)?;
    let document = digest.hash(&prepared.signed_parts());
    let mut signature = cms::build(signer, digest, &document).map_err(|e| match e {
        cms::BuildError::Encoding(e) => encoding(e),
        cms::BuildError::Signer(e) => SigningError::Signer(e),
    })?;
    if let Some(stamper) = stamper {
        let value = cms::signature_value(&signature).map_err(encoding)?;
        let token = stamper
            .stamp(digest, &value)
            .map_err(SigningError::Timestamp)?;
        signature = cms::add_timestamp(&signature, &token).map_err(encoding)?;
    }
    prepared.finish(&signature).map_err(SigningError::Pdf)
}

/// Lines for a visible signature: who signed, when, and why.
pub fn appearance_lines(signer: &dyn Signer, date: &str, reason: Option<&str>) -> Vec<String> {
    let name = common_name(&signer.certificate().tbs_certificate.subject);
    let mut lines = vec![
        format!("Digitally signed by {name}"),
        format!("Date: {date}"),
    ];
    if let Some(reason) = reason.filter(|r| !r.is_empty()) {
        lines.push(format!("Reason: {reason}"));
    }
    lines
}

/// `time` as people read it, in UTC: `2026-10-18 12:50 UTC`.
pub fn shown_date(time: std::time::SystemTime) -> String {
    let d = katna_pdf_cos::pdf_date(time);
    format!(
        "{}-{}-{} {}:{} UTC",
        &d[2..6],
        &d[6..8],
        &d[8..10],
        &d[10..12],
        &d[12..14]
    )
}

/// PDF date `date` (`D:YYYYMMDDHHmmSS` and a zone) as people read it:
/// `2026-10-18 12:50:45 UTC`, or `+05:30` for a zone ahead of UTC.
pub fn readable_pdf_date(date: &str) -> Option<String> {
    let d = date.strip_prefix("D:").unwrap_or(date);
    let digits = d.chars().take_while(char::is_ascii_digit).count();
    if digits < 8 {
        return None;
    }
    let part = |from: usize, to: usize, or: &'static str| {
        d.get(from..to).filter(|_| digits >= to).unwrap_or(or)
    };
    let zone = match d[digits..].chars().next() {
        Some('Z') => "UTC".to_owned(),
        Some(sign @ ('+' | '-')) => {
            let z: String = d[digits + 1..]
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            match z.len() {
                4 => format!("{sign}{}:{}", &z[..2], &z[2..]),
                2 => format!("{sign}{z}:00"),
                _ => String::new(),
            }
        }
        _ => String::new(),
    };
    let shown = format!(
        "{}-{}-{} {}:{}:{} {zone}",
        &d[..4],
        &d[4..6],
        &d[6..8],
        part(8, 10, "00"),
        part(10, 12, "00"),
        part(12, 14, "00"),
    );
    Some(shown.trim_end().to_owned())
}

/// One signature of a file, checked.
#[derive(Debug, Clone)]
pub struct SignatureCheck {
    pub signature: Signature,
    pub coverage: Coverage,
    pub result: Result<Checked, CheckError>,
    /// Whether the signer's certificate chains to a trusted root, when
    /// the signature could be read.
    pub trust: Option<Trust>,
}

impl SignatureCheck {
    /// The signature holds: the bytes it covers are as signed, by the
    /// key of the certificate it names.
    pub fn intact(&self) -> bool {
        self.coverage != Coverage::Broken
            && self.result.as_ref().is_ok_and(|c| {
                c.digest_matches && c.signature_valid && c.certificate_named != Some(false)
            })
    }

    /// When it was signed: as the signature states it, or as the file
    /// says (which the signer's computer set).
    pub fn time(&self) -> Option<String> {
        self.result
            .as_ref()
            .ok()
            .and_then(|r| r.signing_time.clone())
            .or_else(|| self.signature.time.as_deref().and_then(readable_pdf_date))
    }

    /// Who signed: the common name of the certificate, or the name the
    /// file gives.
    pub fn signer(&self) -> Option<String> {
        match &self.result {
            Ok(r) => Some(common_name(&r.signer.tbs_certificate.subject)),
            Err(_) => self.signature.info.name.clone(),
        }
    }

    /// Why it does not hold, in a few words; `None` when it does.
    pub fn problem(&self) -> Option<String> {
        if self.intact() {
            return None;
        }
        Some(match &self.result {
            Err(e) => e.to_string(),
            Ok(r) if !r.digest_matches => "the signed part of the file was changed".into(),
            Ok(r) if !r.signature_valid => "the signature does not match the signer's key".into(),
            Ok(_) if self.coverage == Coverage::Broken => "its byte range is wrong".into(),
            Ok(_) => "it names another certificate than the one that signed".into(),
        })
    }
}

/// Every signature of `doc`, whose bytes are `file`, checked, and its
/// signer's certificate chained to `roots`.
pub fn check(doc: &Document, file: &[u8], roots: &Roots) -> Vec<SignatureCheck> {
    doc.signatures()
        .into_iter()
        .map(|signature| {
            let coverage = signature.coverage(file);
            let result = match signature.signed_parts(file) {
                None => Err(CheckError::Malformed),
                Some(_) if signature.sub_filter == "adbe.pkcs7.sha1" => Err(
                    CheckError::Unsupported("the old adbe.pkcs7.sha1 format".into()),
                ),
                Some(parts) => cms::check(&signature.contents, &parts),
            };
            let trust = result
                .as_ref()
                .ok()
                .map(|r| trust::chain(&r.signer, &r.certificates, roots, checked_at(r)));
            SignatureCheck {
                signature,
                coverage,
                result,
                trust,
            }
        })
        .collect()
}

/// When a signature's certificates should have been valid: when its
/// timestamp says it was signed, or, without one, when it says it was
/// signed itself, or else now.
fn checked_at(r: &Checked) -> std::time::SystemTime {
    r.timestamp
        .as_ref()
        .filter(|s| s.valid)
        .and_then(|s| s.at)
        .or(r.signed_at)
        .unwrap_or_else(std::time::SystemTime::now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_people_write_them() {
        assert_eq!(
            readable_pdf_date("D:20261018125045Z").as_deref(),
            Some("2026-10-18 12:50:45 UTC")
        );
        assert_eq!(
            readable_pdf_date("D:20261018125045+05'30'").as_deref(),
            Some("2026-10-18 12:50:45 +05:30")
        );
        assert_eq!(
            readable_pdf_date("D:20261018").as_deref(),
            Some("2026-10-18 00:00:00")
        );
        assert_eq!(readable_pdf_date("D:2026"), None);
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_327_845);
        assert_eq!(shown_date(t), "2026-10-18 12:50 UTC");
    }
}
