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
pub mod revocation;
pub mod trust;
pub mod tsa;
mod walk;

use std::collections::BTreeSet;
use std::sync::Arc;

use der::{Decode as _, Encode as _};
use katna_pdf_cos::{
    Allowed, Change, Coverage, Document, Dss, Lock, SignError, SignRequest, Signature,
};

pub use crate::cms::{CheckError, Checked, Stamp, common_name};
pub use crate::keys::{KeyKind, Signer, SignerError, SoftKey};
pub use crate::p12::{Identity, P12Error};
pub use crate::revocation::Revocation;
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

/// `doc` with a document timestamp from `stamper` over all of it (PAdES
/// B-LTA, after its validation data is kept): it vouches the file, and
/// the validation data in it, existed then, so they can be trusted after
/// the certificates in them expire.
pub fn timestamp_document(
    doc: &Document,
    stamper: &dyn tsa::Stamper,
    digest: Digest,
) -> Result<Vec<u8>, SigningError> {
    let prepared = doc
        .prepare_signature(&SignRequest::document_timestamp())
        .map_err(SigningError::Pdf)?;
    let token = stamper
        .stamp(digest, &prepared.signed_parts().concat())
        .map_err(SigningError::Timestamp)?;
    prepared.finish(&token).map_err(SigningError::Pdf)
}

impl SignatureCheck {
    /// Whether it is a document timestamp rather than a signature.
    pub fn is_document_timestamp(&self) -> bool {
        self.signature.sub_filter == "ETSI.RFC3161"
    }
}

/// Widest a picture in a signature box is kept, in pixels: enough for
/// print, small in the file.
const PICTURE_WIDTH: u32 = 900;

/// The picture in a PNG or JPEG file `bytes`, such as a scanned
/// handwritten signature, to show in a signature's box; made smaller
/// when it is large.
pub fn picture(bytes: &[u8]) -> Result<katna_pdf_cos::Picture, String> {
    let image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let image = if image.width() > PICTURE_WIDTH {
        image.resize(
            PICTURE_WIDTH,
            u32::MAX,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    let mut alpha = Vec::with_capacity((width * height) as usize);
    for p in rgba.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
        alpha.push(p.0[3]);
    }
    let see_through = alpha.iter().any(|&a| a < 255);
    Ok(katna_pdf_cos::Picture {
        width,
        height,
        rgb,
        alpha: see_through.then_some(alpha),
    })
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
    /// Whether the signer's certificates were revoked, once asked
    /// ([`check_online`]).
    pub revocation: Option<Revocation>,
    /// When it certifies the file: the changes made after it that it
    /// does not allow.
    pub forbidden: Vec<Change>,
    /// What was changed after it, when the file was changed after it.
    pub later: BTreeSet<Change>,
    /// The fields it locks that were changed after it, by full name.
    pub unlocked: Vec<String>,
    /// The validation data the file keeps.
    known: Arc<Dss>,
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
            if !self.unlocked.is_empty() {
                return Some(format!(
                    "fields it locked were changed after it ({})",
                    self.unlocked.join(", ")
                ));
            }
            if self.forbidden.is_empty() {
                return None;
            }
            let changes: Vec<&str> = self.forbidden.iter().map(|c| change_text(*c)).collect();
            return Some(format!(
                "after it certified the file, changes it does not allow were made ({})",
                changes.join(", ")
            ));
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

/// A kind of change, in a few words.
pub fn change_text(change: Change) -> &'static str {
    match change {
        Change::FormFilled => "form filled in",
        Change::Signed => "signed",
        Change::Commented => "comments",
        Change::NewField => "signature fields added",
        Change::Dss => "validation data",
        Change::Pages => "pages",
        Change::Content => "page content",
        Change::Other => "other changes",
    }
}

/// Whether `cert` is made for signing: its key usage, when it says,
/// includes signing (digital signature or non-repudiation), and it is
/// not a certificate authority's.
pub fn can_sign(cert: &Certificate) -> bool {
    use x509_cert::ext::pkix::{BasicConstraints, KeyUsage};
    let tbs = &cert.tbs_certificate;
    if let Ok(Some((_, bc))) = tbs.get::<BasicConstraints>()
        && bc.ca
    {
        return false;
    }
    match tbs.get::<KeyUsage>() {
        Ok(Some((_, usage))) => usage.digital_signature() || usage.non_repudiation(),
        _ => true,
    }
}

/// Which fields a signature locks, in a few words.
pub fn lock_text(lock: &Lock) -> String {
    match lock {
        Lock::All => "all form fields".into(),
        Lock::Include(names) => names.join(", "),
        Lock::Exclude(names) => format!("all form fields but {}", names.join(", ")),
    }
}

/// Which changes a certification allows, in a few words.
pub fn allowed_text(allowed: Allowed) -> &'static str {
    match allowed {
        Allowed::Nothing => "no changes",
        Allowed::FormFilling => "form filling and signing",
        Allowed::Comments => "form filling, signing and comments",
    }
}

/// Every signature of `doc`, whose bytes are `file`, checked, and its
/// signer's certificate chained to `roots`.
pub fn check(doc: &Document, file: &[u8], roots: &Roots) -> Vec<SignatureCheck> {
    let known = Arc::new(doc.dss());
    let kept = certificates(&known.certs);
    doc.signatures()
        .into_iter()
        .map(|signature| {
            let coverage = signature.coverage(file);
            let result = match signature.signed_parts(file) {
                None => Err(CheckError::Malformed),
                Some(_) if signature.sub_filter == "adbe.pkcs7.sha1" => Err(
                    CheckError::Unsupported("the old adbe.pkcs7.sha1 format".into()),
                ),
                Some(parts) if signature.sub_filter == "ETSI.RFC3161" => {
                    cms::check_document_stamp(&signature.contents, &parts)
                }
                Some(parts) => cms::check(&signature.contents, &parts),
            };
            // The chain, through the certificates sent and those the
            // file keeps; and whether they were revoked, as far as the
            // file's own validation data tells.
            let mut revocation = None;
            let trust = result.as_ref().ok().map(|r| {
                let mut sent = r.certificates.clone();
                sent.extend(kept.iter().cloned());
                let (trust, path) = trust::path(&r.signer, &sent, roots, checked_at(r));
                if path.len() > 1 && !known.is_empty() {
                    let told = revocation::check_with(
                        &path,
                        revoked_by(r),
                        &known,
                        None,
                        &mut Dss::default(),
                    );
                    if !matches!(told, Revocation::Unknown { .. }) {
                        revocation = Some(told);
                    }
                }
                trust
            });
            let later = match coverage {
                Coverage::Revision(end) => doc.changes_since(end),
                _ => BTreeSet::new(),
            };
            let unlocked = match (&signature.lock, coverage) {
                (Some(lock), Coverage::Revision(end)) => doc
                    .fields_changed_since(end)
                    .into_iter()
                    .filter(|(name, sig)| lock.covers(name, *sig))
                    .map(|(name, _)| name)
                    .collect(),
                _ => Vec::new(),
            };
            let forbidden = match signature.certify {
                Some(allowed) => later
                    .iter()
                    .copied()
                    .filter(|c| !allowed.permits(*c))
                    .collect(),
                None => Vec::new(),
            };
            SignatureCheck {
                signature,
                coverage,
                result,
                trust,
                revocation,
                forbidden,
                later,
                unlocked,
                known: known.clone(),
            }
        })
        .collect()
}

/// Checks `check` further, over the network through `fetch`: the
/// issuers its signature did not send are fetched from where the
/// certificates say they are, and its signer's certificates are asked
/// about at their issuers' OCSP responders, or looked up in their CRLs.
pub fn check_online(check: &mut SignatureCheck, roots: &Roots, fetch: &dyn revocation::Fetch) {
    // What the file keeps told already.
    if check.revocation.is_some() {
        return;
    }
    let Ok(r) = &check.result else {
        return;
    };
    let mut sent = r.certificates.clone();
    sent.extend(certificates(&check.known.certs));
    sent.extend(revocation::issuers(&r.signer, &sent, fetch));
    let (trust, path) = trust::path(&r.signer, &sent, roots, checked_at(r));
    if path.len() > 1 {
        check.revocation = Some(revocation::check_with(
            &path,
            revoked_by(r),
            &check.known,
            Some(fetch),
            &mut Dss::default(),
        ));
    }
    check.trust = Some(trust);
}

/// The validation data for `doc`'s signatures: the certificates of each
/// signer's chain (missing issuers fetched) and OCSP answers or CRLs
/// saying they were not revoked, fetched through `fetch`; and what could
/// not be told, by signer.
pub fn validation_data(
    doc: &Document,
    roots: &Roots,
    fetch: &dyn revocation::Fetch,
) -> (Dss, Vec<String>) {
    let known = doc.dss();
    let mut data = Dss::default();
    let mut untold = Vec::new();
    for c in check(doc, doc.bytes(), roots) {
        let Ok(r) = &c.result else {
            continue;
        };
        let mut sent = r.certificates.clone();
        sent.extend(certificates(&known.certs));
        sent.extend(revocation::issuers(&r.signer, &sent, fetch));
        let (_, path) = trust::path(&r.signer, &sent, roots, checked_at(r));
        let mut kept = Dss::default();
        match revocation::check_with(&path, revoked_by(r), &known, Some(fetch), &mut kept) {
            Revocation::Unknown { name, why } => untold.push(format!("{name}: {why}")),
            Revocation::Revoked { name, .. } => untold.push(format!("{name}: revoked")),
            Revocation::Good => {}
        }
        kept.certs = path.iter().filter_map(|c| c.to_der().ok()).collect();
        data.merge(&kept);
    }
    (data, untold)
}

/// `doc` with its signatures' validation data added (PAdES B-LT), as an
/// incremental update; and what could not be told, by signer.
pub fn add_validation_data(
    doc: &Document,
    roots: &Roots,
    fetch: &dyn revocation::Fetch,
) -> Result<(Vec<u8>, Vec<String>), SigningError> {
    let (data, untold) = validation_data(doc, roots, fetch);
    let bytes = doc
        .add_dss(&data)
        .map_err(|e| SigningError::Pdf(SignError::Update(e)))?;
    Ok((bytes, untold))
}

/// The certificates of `ders` that can be read.
fn certificates(ders: &[Vec<u8>]) -> Vec<Certificate> {
    ders.iter()
        .filter_map(|d| Certificate::from_der(d).ok())
        .collect()
}

/// When a signer's certificates must not have been revoked by: when a
/// timestamp vouches the signature was made, or, without one, now: the
/// signer's own clock could say anything.
fn revoked_by(r: &Checked) -> std::time::SystemTime {
    r.timestamp
        .as_ref()
        .filter(|s| s.valid)
        .and_then(|s| s.at)
        .unwrap_or_else(std::time::SystemTime::now)
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
