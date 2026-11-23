// SPDX-License-Identifier: GPL-3.0-or-later

//! What a certificate says, for a certificate viewer: the chain from a
//! signer up as far as it was sent, and each certificate's names, uses,
//! policies and addresses, plus its text as PEM to save. Words are the
//! app's; this gives the facts.

use const_oid::ObjectIdentifier as Oid;
use der::{Encode, EncodePem, pem::LineEnding};
use sha1::Digest as _;
use x509_cert::Certificate;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{
    BasicConstraints, CertificatePolicies, ExtendedKeyUsage, KeyUsage, SubjectAltName,
};

use crate::cms::name_text;
use crate::oid;
use crate::revocation::{crl_urls, extension, urls};

/// A part of a name (a certificate's subject or issuer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePart {
    Name,
    Organization,
    Unit,
    Title,
    Locality,
    State,
    PostalCode,
    Country,
    Email,
    SerialNumber,
    Pseudonym,
    Other,
}

/// What a certificate's key may do (its key usage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyUse {
    DigitalSignature,
    NonRepudiation,
    KeyEncipherment,
    DataEncipherment,
    KeyAgreement,
    CertificateSigning,
    CrlSigning,
}

/// What a certificate is for (its extended key usage).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    ServerAuth,
    ClientAuth,
    CodeSigning,
    EmailProtection,
    TimeStamping,
    OcspSigning,
    DocumentSigning,
    AdobeDocuments,
    Any,
    Other(String),
}

/// Everything a certificate viewer shows of one certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Details {
    pub subject: Vec<(NamePart, String)>,
    pub issuer: Vec<(NamePart, String)>,
    /// X.509 version, 1 to 3.
    pub version: u8,
    /// The signature algorithm, as "SHA-256 with RSA".
    pub algorithm: String,
    pub key_use: Vec<KeyUse>,
    pub purposes: Vec<Purpose>,
    /// Policy identifiers, dotted.
    pub policies: Vec<String>,
    /// Mail addresses among its other names.
    pub emails: Vec<String>,
    /// It may issue certificates, and how many below it, when limited.
    pub authority: Option<Option<u8>>,
    pub ocsp: Vec<String>,
    pub crls: Vec<String>,
    pub issuer_urls: Vec<String>,
    /// SHA-1 of the whole certificate, as hex pairs.
    pub sha1: String,
}

/// `signer`, then who issued it, and so on, from the certificates sent
/// with the signature: as far as they go, or to one that issued itself.
pub fn chain_certificates(signer: &Certificate, sent: &[Certificate]) -> Vec<Certificate> {
    let mut chain = vec![signer.clone()];
    let mut at = signer;
    for _ in 0..8 {
        let tbs = &at.tbs_certificate;
        if tbs.issuer == tbs.subject {
            break;
        }
        match sent
            .iter()
            .find(|c| c.tbs_certificate.subject == tbs.issuer && *c != at)
        {
            Some(next) if !chain.contains(next) => {
                chain.push(next.clone());
                at = next;
            }
            _ => break,
        }
    }
    chain
}

/// The details of `cert`.
pub fn details(cert: &Certificate) -> Details {
    let tbs = &cert.tbs_certificate;
    let key_use = match tbs.get::<KeyUsage>() {
        Ok(Some((_, u))) => [
            (u.digital_signature(), KeyUse::DigitalSignature),
            (u.non_repudiation(), KeyUse::NonRepudiation),
            (u.key_encipherment(), KeyUse::KeyEncipherment),
            (u.data_encipherment(), KeyUse::DataEncipherment),
            (u.key_agreement(), KeyUse::KeyAgreement),
            (u.key_cert_sign(), KeyUse::CertificateSigning),
            (u.crl_sign(), KeyUse::CrlSigning),
        ]
        .into_iter()
        .filter_map(|(on, what)| on.then_some(what))
        .collect(),
        _ => Vec::new(),
    };
    let purposes = extension::<ExtendedKeyUsage>(cert, oid::EXTENDED_KEY_USAGE)
        .map(|eku| eku.0.iter().map(purpose).collect())
        .unwrap_or_default();
    let policies = match tbs.get::<CertificatePolicies>() {
        Ok(Some((_, p))) => {
            p.0.iter()
                .map(|p| p.policy_identifier.to_string())
                .collect()
        }
        _ => Vec::new(),
    };
    let emails = match tbs.get::<SubjectAltName>() {
        Ok(Some((_, san))) => san
            .0
            .iter()
            .filter_map(|n| match n {
                GeneralName::Rfc822Name(m) => Some(m.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let authority = match tbs.get::<BasicConstraints>() {
        Ok(Some((_, bc))) if bc.ca => Some(bc.path_len_constraint),
        _ => None,
    };
    let der = cert.to_der().unwrap_or_default();
    let sha1 = sha1::Sha1::digest(&der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    Details {
        subject: name_parts(&tbs.subject),
        issuer: name_parts(&tbs.issuer),
        version: match tbs.version {
            x509_cert::Version::V1 => 1,
            x509_cert::Version::V2 => 2,
            x509_cert::Version::V3 => 3,
        },
        algorithm: algorithm(&cert.signature_algorithm.oid),
        key_use,
        purposes,
        policies,
        emails,
        authority,
        ocsp: urls(cert, oid::OCSP),
        crls: crl_urls(cert),
        issuer_urls: urls(cert, oid::CA_ISSUERS),
        sha1,
    }
}

/// `cert` as PEM text, as a `.cer` or `.pem` file holds it.
pub fn pem(cert: &Certificate) -> String {
    cert.to_pem(LineEnding::LF).unwrap_or_default()
}

fn name_parts(name: &x509_cert::name::Name) -> Vec<(NamePart, String)> {
    const PARTS: [(&str, NamePart); 11] = [
        ("2.5.4.3", NamePart::Name),
        ("2.5.4.10", NamePart::Organization),
        ("2.5.4.11", NamePart::Unit),
        ("2.5.4.12", NamePart::Title),
        ("2.5.4.7", NamePart::Locality),
        ("2.5.4.8", NamePart::State),
        ("2.5.4.17", NamePart::PostalCode),
        ("2.5.4.6", NamePart::Country),
        ("1.2.840.113549.1.9.1", NamePart::Email),
        ("2.5.4.5", NamePart::SerialNumber),
        ("2.5.4.65", NamePart::Pseudonym),
    ];
    name.0
        .iter()
        .flat_map(|rdn| rdn.0.iter())
        .filter_map(|atv| {
            let text = name_text(atv)?;
            let id = atv.oid.to_string();
            let part = PARTS
                .iter()
                .find(|(o, _)| *o == id)
                .map_or(NamePart::Other, |(_, p)| *p);
            Some((part, text))
        })
        .collect()
}

fn purpose(id: &Oid) -> Purpose {
    match id.to_string().as_str() {
        "1.3.6.1.5.5.7.3.1" => Purpose::ServerAuth,
        "1.3.6.1.5.5.7.3.2" => Purpose::ClientAuth,
        "1.3.6.1.5.5.7.3.3" => Purpose::CodeSigning,
        "1.3.6.1.5.5.7.3.4" => Purpose::EmailProtection,
        "1.3.6.1.5.5.7.3.8" => Purpose::TimeStamping,
        "1.3.6.1.5.5.7.3.9" => Purpose::OcspSigning,
        "1.3.6.1.5.5.7.3.36" | "1.3.6.1.4.1.311.10.3.12" => Purpose::DocumentSigning,
        "1.2.840.113583.1.1.5" => Purpose::AdobeDocuments,
        "2.5.29.37.0" => Purpose::Any,
        other => Purpose::Other(other.to_owned()),
    }
}

fn algorithm(id: &Oid) -> String {
    match *id {
        oid::SHA1_RSA => "SHA-1 with RSA".into(),
        oid::SHA256_RSA => "SHA-256 with RSA".into(),
        oid::SHA384_RSA => "SHA-384 with RSA".into(),
        oid::SHA512_RSA => "SHA-512 with RSA".into(),
        oid::RSA_PSS => "RSA-PSS".into(),
        oid::ECDSA_SHA256 => "ECDSA with SHA-256".into(),
        oid::ECDSA_SHA384 => "ECDSA with SHA-384".into(),
        oid::ECDSA_SHA512 => "ECDSA with SHA-512".into(),
        _ if id.to_string() == "1.3.101.112" => "Ed25519".into(),
        _ => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use der::{Decode, DecodePem};

    #[test]
    fn a_root_and_its_details() {
        let der = include_bytes!("../roots/india/cca-india-2022.der");
        let root = Certificate::from_der(der).unwrap();
        let d = details(&root);
        assert_eq!(d.version, 3);
        assert!(d.authority.is_some(), "{d:?}");
        assert!(d.key_use.contains(&KeyUse::CertificateSigning), "{d:?}");
        assert!(d.subject.iter().any(|(p, _)| *p == NamePart::Name), "{d:?}");
        assert!(
            d.subject
                .iter()
                .any(|(p, v)| *p == NamePart::Country && v == "IN"),
            "{d:?}"
        );
        assert_eq!(d.sha1.len(), 59);
        // Chains to itself, alone.
        assert_eq!(
            chain_certificates(&root, std::slice::from_ref(&root)).len(),
            1
        );
        // Saved and read back.
        assert_eq!(Certificate::from_pem(pem(&root)).unwrap(), root);
    }
}
