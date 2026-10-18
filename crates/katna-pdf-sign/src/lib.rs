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
mod walk;

use katna_pdf_cos::{Coverage, Document, SignError, SignRequest, Signature};

pub use crate::cms::{CheckError, Checked, common_name};
pub use crate::keys::{KeyKind, Signer, SignerError, SoftKey};
pub use crate::p12::{Identity, P12Error};

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
    Encoding(String),
}

impl std::fmt::Display for SigningError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SigningError::Pdf(e) => e.fmt(f),
            SigningError::Signer(e) => e.fmt(f),
            SigningError::Encoding(e) => write!(f, "the signature could not be written: {e}"),
        }
    }
}

impl std::error::Error for SigningError {}

/// `doc` signed by `signer` as `request` says, with `digest`: the file
/// with the signature appended as an incremental update.
pub fn sign(
    doc: &Document,
    request: &SignRequest,
    signer: &dyn Signer,
    digest: Digest,
) -> Result<Vec<u8>, SigningError> {
    let prepared = doc.prepare_signature(request).map_err(SigningError::Pdf)?;
    let document = digest.hash(&prepared.signed_parts());
    let signature = cms::build(signer, digest, &document).map_err(|e| match e {
        cms::BuildError::Encoding(e) => SigningError::Encoding(e.to_string()),
        cms::BuildError::Signer(e) => SigningError::Signer(e),
    })?;
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

/// One signature of a file, checked.
#[derive(Debug, Clone)]
pub struct SignatureCheck {
    pub signature: Signature,
    pub coverage: Coverage,
    pub result: Result<Checked, CheckError>,
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
}

/// Every signature of `doc`, whose bytes are `file`, checked.
pub fn check(doc: &Document, file: &[u8]) -> Vec<SignatureCheck> {
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
            SignatureCheck {
                signature,
                coverage,
                result,
            }
        })
        .collect()
}
