// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a signer is who their certificate says: the certificate
//! chained, issuer by issuer, to a root certificate the person trusts,
//! each one valid when the file was signed. The roots are the system's
//! and the ones the person adds (India's CCA roots, a company's own),
//! as certificate files in a folder.
//!
//! India's CCA roots come built in (`roots/india`), with the issuing
//! certificates of authorities under them that signers' tokens often
//! leave out.
//!
//! Revocation is checked online, apart, in [`crate::revocation`].

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use der::{Decode, Encode};
use x509_cert::Certificate;

use crate::cms::{common_name, verify};
use crate::{Hash, oid, walk};

/// How many issuers a chain may climb before it is given up on.
const DEPTH: usize = 10;

/// The root certificates trusted.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    certs: Vec<Certificate>,
    /// Certificates known to complete chains with, not trusted
    /// themselves.
    issuers: Vec<Certificate>,
}

/// India's CCA roots (Controller of Certifying Authorities, Root CA of
/// India): every one, as old signatures were made under the old ones.
const INDIA: [&[u8]; 5] = [
    include_bytes!("../roots/india/cca-india-2007.der"),
    include_bytes!("../roots/india/cca-india-2011.der"),
    include_bytes!("../roots/india/cca-india-2014.der"),
    include_bytes!("../roots/india/cca-india-2015-spl.der"),
    include_bytes!("../roots/india/cca-india-2022.der"),
];

/// Licensed certifying authorities' issuing certificates under them.
const INDIA_ISSUERS: [&[u8]; 8] = [
    include_bytes!("../roots/india/issuers/emudhra-ca-2014.der"),
    include_bytes!("../roots/india/issuers/emudhra-class3-organisation-2014.der"),
    include_bytes!("../roots/india/issuers/emudhra-ca-2022.der"),
    include_bytes!("../roots/india/issuers/emudhra-class2-document-signer-2022.der"),
    include_bytes!("../roots/india/issuers/emudhra-class3-document-signer-2022.der"),
    include_bytes!("../roots/india/issuers/emudhra-class3-individual-2022.der"),
    include_bytes!("../roots/india/issuers/emudhra-class3-organisation-2022.der"),
    include_bytes!("../roots/india/issuers/emudhra-dgft-2022.der"),
];

impl Roots {
    /// No roots.
    pub fn new() -> Self {
        Self::default()
    }

    /// The system's roots: the ones web browsers trust on Linux, and
    /// Windows' own store, which holds India's CCA roots too.
    pub fn system() -> Self {
        let mut roots = Self::new();
        for cert in rustls_native_certs::load_native_certs().certs {
            if let Ok(cert) = Certificate::from_der(&cert) {
                roots.add(cert);
            }
        }
        roots
    }

    /// The roots trusted unless the person says otherwise: the system's
    /// and India's.
    pub fn standard() -> Self {
        let mut roots = Self::system();
        roots.add_india();
        roots
    }

    /// Trusts India's CCA roots, and knows the issuers under them.
    pub fn add_india(&mut self) {
        for der in INDIA {
            if let Ok(cert) = Certificate::from_der(der) {
                self.add(cert);
            }
        }
        for der in INDIA_ISSUERS {
            if let Ok(cert) = Certificate::from_der(der)
                && !self.issuers.contains(&cert)
            {
                self.issuers.push(cert);
            }
        }
    }

    /// Trusts `cert` as a root.
    pub fn add(&mut self, cert: Certificate) {
        if !self.contains(&cert) {
            self.certs.push(cert);
        }
    }

    /// Trusts the certificates in file `bytes` (DER, PEM with one or
    /// more, or bare Base64 as some authorities publish theirs); how
    /// many it held.
    pub fn add_file(&mut self, bytes: &[u8]) -> usize {
        let certs = match Certificate::from_der(bytes) {
            Ok(cert) => vec![cert],
            Err(_) => Certificate::load_pem_chain(bytes)
                .ok()
                .filter(|c| !c.is_empty())
                .or_else(|| bare_base64(bytes).map(|c| vec![c]))
                .unwrap_or_default(),
        };
        let n = certs.len();
        for cert in certs {
            self.add(cert);
        }
        n
    }

    /// Trusts the certificate files in folder `dir`; how many it held.
    pub fn add_folder(&mut self, dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        paths
            .iter()
            .filter_map(|p| std::fs::read(p).ok())
            .map(|bytes| self.add_file(&bytes))
            .sum()
    }

    /// Keeps `cert` in folder `dir`, where [`Roots::add_folder`] finds
    /// it; the file it is kept in.
    pub fn keep(dir: &Path, cert: &Certificate) -> std::io::Result<PathBuf> {
        let der = cert.to_der().map_err(std::io::Error::other)?;
        let name: String = common_name(&cert.tbs_certificate.subject)
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let serial: String = cert
            .tbs_certificate
            .serial_number
            .as_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{name}-{serial}.der"));
        std::fs::write(&path, der)?;
        Ok(path)
    }

    /// Whether `cert` is one of them.
    pub fn contains(&self, cert: &Certificate) -> bool {
        self.certs.iter().any(|c| c == cert)
    }

    pub fn len(&self) -> usize {
        self.certs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.certs.is_empty()
    }
}

/// Whether a signer's certificate is vouched for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// It chains to `root`, a trusted root.
    Trusted { root: String },
    /// It chains to `root`, which is not trusted; when `complete` is
    /// false, `root` is not a root at all but the last certificate
    /// found, whose issuer was not sent with the signature.
    Untrusted {
        root: Box<Certificate>,
        complete: bool,
    },
    /// Certificate `name` in the chain was not valid at the time.
    Expired { name: String },
    /// Certificate `name` in the chain was not issued by the one it
    /// names as its issuer.
    Forged { name: String },
}

impl Trust {
    pub fn trusted(&self) -> bool {
        matches!(self, Trust::Trusted { .. })
    }
}

/// Chains `signer`, through the certificates `sent` with the signature,
/// to one of `roots`, as things stood at time `at`.
pub fn chain(signer: &Certificate, sent: &[Certificate], roots: &Roots, at: SystemTime) -> Trust {
    path(signer, sent, roots, at).0
}

/// The same, with the chain itself: `signer`, its issuer, and so on up
/// to the root, or as far as it goes.
pub fn path(
    signer: &Certificate,
    sent: &[Certificate],
    roots: &Roots,
    at: SystemTime,
) -> (Trust, Vec<Certificate>) {
    let mut path = vec![signer.clone()];
    let trust = 'up: {
        let mut cert = signer;
        for _ in 0..DEPTH {
            let name = || common_name(&cert.tbs_certificate.subject);
            if !valid_at(cert, at) {
                break 'up Trust::Expired { name: name() };
            }
            if roots.contains(cert) {
                break 'up Trust::Trusted { root: name() };
            }
            let tbs = &cert.tbs_certificate;
            let issuers: Vec<&Certificate> = roots
                .certs
                .iter()
                .chain(sent)
                .chain(&roots.issuers)
                .filter(|c| c.tbs_certificate.subject == tbs.issuer && *c != cert)
                .collect();
            let Some(issuer) = issuers.iter().copied().find(|i| issued(cert, i)) else {
                if issuers.is_empty() {
                    let complete = tbs.subject == tbs.issuer && issued(cert, cert);
                    break 'up Trust::Untrusted {
                        root: Box::new(cert.clone()),
                        complete,
                    };
                }
                break 'up Trust::Forged { name: name() };
            };
            if !roots.contains(issuer) && !is_authority(issuer) {
                break 'up Trust::Forged { name: name() };
            }
            path.push(issuer.clone());
            cert = issuer;
        }
        Trust::Untrusted {
            root: Box::new(cert.clone()),
            complete: false,
        }
    };
    (trust, path)
}

/// A certificate written as Base64 with no PEM lines around it.
fn bare_base64(bytes: &[u8]) -> Option<Certificate> {
    let text: String = std::str::from_utf8(bytes)
        .ok()?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !text.starts_with("MII") {
        return None;
    }
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in text.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).ok()?);
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    der::DecodePem::from_pem(pem.as_bytes()).ok()
}

/// Whether `cert` was valid at `at`.
fn valid_at(cert: &Certificate, at: SystemTime) -> bool {
    let validity = &cert.tbs_certificate.validity;
    validity.not_before.to_system_time() <= at && at <= validity.not_after.to_system_time()
}

/// Whether `issuer`'s key signed `cert`.
fn issued(cert: &Certificate, issuer: &Certificate) -> bool {
    let Ok(tbs) = cert.tbs_certificate.to_der() else {
        return false;
    };
    let Some(signature) = cert.signature.as_bytes() else {
        return false;
    };
    signed_by(issuer, &cert.signature_algorithm, &tbs, signature)
}

/// Whether `signature`, made with algorithm `alg`, is `key`'s key's
/// over `message`: how certificates, CRLs and OCSP answers are signed.
pub(crate) fn signed_by(
    key: &Certificate,
    alg: &spki::AlgorithmIdentifierOwned,
    message: &[u8],
    signature: &[u8],
) -> bool {
    let hash = match alg.oid {
        a if a == oid::SHA1_RSA => Hash::Sha1,
        a if a == oid::SHA256_RSA || a == oid::ECDSA_SHA256 => Hash::Sha256,
        a if a == oid::SHA384_RSA || a == oid::ECDSA_SHA384 => Hash::Sha384,
        a if a == oid::SHA512_RSA || a == oid::ECDSA_SHA512 => Hash::Sha512,
        // PSS names its own digest in its parameters.
        a if a == oid::RSA_PSS => Hash::Sha256,
        _ => return false,
    };
    let Ok(alg) = alg.to_der() else {
        return false;
    };
    let Some((alg, _)) = walk::read(&alg) else {
        return false;
    };
    verify(key, &alg, hash, message, signature).unwrap_or(false)
}

/// Whether `cert` may issue certificates: its basic constraints say it
/// is an authority.
fn is_authority(cert: &Certificate) -> bool {
    use x509_cert::ext::pkix::BasicConstraints;
    cert.tbs_certificate
        .extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == oid::BASIC_CONSTRAINTS)
        .and_then(|e| BasicConstraints::from_der(e.extn_value.as_bytes()).ok())
        .is_some_and(|b| b.ca)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn cert(der: &[u8]) -> Certificate {
        Certificate::from_der(der).unwrap()
    }

    /// The test CA and Asha's certificate, which it issued.
    fn pair() -> (Certificate, Certificate) {
        (
            cert(include_bytes!("../tests/data/ca.der")),
            cert(include_bytes!("../tests/data/asha.der")),
        )
    }

    fn in_range(c: &Certificate) -> SystemTime {
        c.tbs_certificate.validity.not_before.to_system_time() + Duration::from_secs(3600)
    }

    #[test]
    fn chains_reach_the_roots_trusted() {
        let (ca, asha) = pair();
        let at = in_range(&asha);
        let mut roots = Roots::new();
        // Nobody trusted yet: the chain ends at the CA, which is not.
        let sent = [asha.clone(), ca.clone()];
        assert_eq!(
            chain(&asha, &sent, &roots, at),
            Trust::Untrusted {
                root: Box::new(ca.clone()),
                complete: true
            }
        );
        // The CA not sent: Asha's certificate is as far as it goes.
        assert_eq!(
            chain(&asha, &[], &roots, at),
            Trust::Untrusted {
                root: Box::new(asha.clone()),
                complete: false
            }
        );
        roots.add(ca.clone());
        let trusted = Trust::Trusted {
            root: common_name(&ca.tbs_certificate.subject),
        };
        assert_eq!(chain(&asha, &sent, &roots, at), trusted);
        // A trusted root need not be sent.
        assert_eq!(chain(&asha, &[], &roots, at), trusted);
        // Before the certificate was issued.
        assert!(matches!(
            chain(&asha, &[], &roots, UNIX_EPOCH),
            Trust::Expired { .. }
        ));
    }

    #[test]
    fn a_certificate_its_issuer_did_not_sign_is_caught() {
        let (ca, asha) = pair();
        let mut forged = asha.clone();
        // The same certificate, its validity stretched by a day.
        let after = forged.tbs_certificate.validity.not_after.to_unix_duration();
        forged.tbs_certificate.validity.not_after =
            x509_cert::time::Time::try_from(UNIX_EPOCH + after + Duration::from_secs(86_400))
                .unwrap();
        let mut roots = Roots::new();
        roots.add(ca);
        assert!(matches!(
            chain(&forged, &[], &roots, in_range(&asha)),
            Trust::Forged { .. }
        ));
    }

    #[test]
    fn india_s_roots_are_built_in() {
        let mut roots = Roots::new();
        roots.add_india();
        assert_eq!(roots.len(), 5);
        // A Class 3 issuer under e-Mudhra CA 2022 chains to CCA India
        // 2022 with what is built in.
        let class3 = cert(INDIA_ISSUERS[5]);
        assert_eq!(
            chain(&class3, &[], &roots, in_range(&class3)),
            Trust::Trusted {
                root: "CCA India 2022".into()
            }
        );
    }

    #[test]
    fn roots_are_read_from_der_and_pem() {
        let (ca, asha) = pair();
        let mut roots = Roots::new();
        assert_eq!(roots.add_file(include_bytes!("../tests/data/ca.der")), 1);
        let pem = [&ca, &asha]
            .iter()
            .map(|c| der::EncodePem::to_pem(*c, der::pem::LineEnding::LF).unwrap())
            .collect::<String>();
        assert_eq!(roots.add_file(pem.as_bytes()), 2);
        // The CA only once.
        assert_eq!(roots.len(), 2);
        assert_eq!(roots.add_file(b"not a certificate"), 0);
        // Bare Base64, as India's CCA publishes some of its roots.
        let der = include_bytes!("../tests/data/ca.der");
        let bare = der::pem::encode_string("CERTIFICATE", der::pem::LineEnding::LF, der)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect::<String>();
        assert_eq!(Roots::new().add_file(bare.as_bytes()), 1);
        // Kept in a folder, and found there again.
        let dir = std::env::temp_dir().join(format!("katna-roots-{}", std::process::id()));
        Roots::keep(&dir, &ca).unwrap();
        let mut again = Roots::new();
        assert_eq!(again.add_folder(&dir), 1);
        assert!(again.contains(&ca));
        let _ = std::fs::remove_dir_all(dir);
    }
}
