// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether a signer's certificates were revoked: asked of their issuers'
//! OCSP responders (RFC 6960), or read from their CRLs (RFC 5280), over
//! HTTP. And the issuers a signature did not send, fetched from where
//! each certificate says its issuer is (Authority Information Access).

use std::time::{Duration, SystemTime};

use der::asn1::OctetString;
use der::{Decode, Encode};
use sha1::Digest as _;
use x509_cert::Certificate;
use x509_cert::ext::pkix::name::{DistributionPointName, GeneralName};
use x509_cert::ext::pkix::{AuthorityInfoAccessSyntax, CrlDistributionPoints, ExtendedKeyUsage};
use x509_ocsp::{
    BasicOcspResponse, CertId, CertStatus, OcspRequest, OcspResponse, Request, TbsRequest,
};

use katna_pdf_cos::Dss;

use crate::cms::common_name;
use crate::oid;
use crate::trust::signed_by;

/// How things are fetched: over HTTP, or as the tests say.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, String>;
}

/// Fetching over HTTP (and HTTPS, through rustls).
#[derive(Debug, Clone)]
pub struct Http {
    pub timeout: Duration,
}

impl Default for Http {
    fn default() -> Self {
        Http {
            timeout: Duration::from_secs(10),
        }
    }
}

impl Http {
    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent()
    }
}

/// The most read of an answer: CRLs can be large.
const LIMIT: u64 = 32 << 20;

impl Fetch for Http {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut answer = self.agent().get(url).call().map_err(|e| e.to_string())?;
        answer
            .body_mut()
            .with_config()
            .limit(LIMIT)
            .read_to_vec()
            .map_err(|e| e.to_string())
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Result<Vec<u8>, String> {
        let mut answer = self
            .agent()
            .post(url)
            .header("Content-Type", content_type)
            .send(body)
            .map_err(|e| e.to_string())?;
        answer
            .body_mut()
            .with_config()
            .limit(LIMIT)
            .read_to_vec()
            .map_err(|e| e.to_string())
    }
}

/// Whether a chain's certificates were revoked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revocation {
    /// None was.
    Good,
    /// Certificate `name` was, at `when` (UTC).
    Revoked { name: String, when: Option<String> },
    /// Whether certificate `name` was could not be told: `why`.
    Unknown { name: String, why: String },
}

/// What one certificate's issuer says of it.
enum Status {
    Good,
    Revoked(SystemTime),
    Unknown(String),
}

/// Whether the certificates of `path` (a signer's certificate, its
/// issuer and so on up) were revoked before `at`. The last one, the
/// root, is not asked about: nothing above it could say.
pub fn check(path: &[Certificate], at: SystemTime, fetch: &dyn Fetch) -> Revocation {
    check_with(path, at, &Dss::default(), Some(fetch), &mut Dss::default())
}

/// The same, told first by validation data `known` (the file's own),
/// then asked of `fetch` when given; the OCSP answers and CRLs that
/// told are added to `kept`.
pub fn check_with(
    path: &[Certificate],
    at: SystemTime,
    known: &Dss,
    fetch: Option<&dyn Fetch>,
    kept: &mut Dss,
) -> Revocation {
    let mut unknown = None;
    for pair in path.windows(2) {
        let [cert, issuer] = pair else { continue };
        let name = common_name(&cert.tbs_certificate.subject);
        let status = match told(cert, issuer, known, kept) {
            Some(status) => status,
            None => match fetch {
                None => Status::Unknown("the file keeps no answer for it".into()),
                Some(fetch) => match ocsp(cert, issuer, fetch) {
                    (status, Some(der)) => {
                        kept.ocsps.push(der);
                        status
                    }
                    (Status::Unknown(why), None) => match crl(cert, issuer, fetch) {
                        (status, Some(der)) => {
                            kept.crls.push(der);
                            status
                        }
                        (_, None) => Status::Unknown(why),
                    },
                    (status, None) => status,
                },
            },
        };
        match status {
            Status::Good => {}
            Status::Revoked(when) if when <= at => {
                let when = katna_pdf_cos::pdf_date(when);
                return Revocation::Revoked {
                    name,
                    when: crate::readable_pdf_date(&when),
                };
            }
            // Revoked after the time it is checked at: it held then.
            Status::Revoked(_) => {}
            Status::Unknown(why) => {
                unknown.get_or_insert(Revocation::Unknown { name, why });
            }
        }
    }
    unknown.unwrap_or(Revocation::Good)
}

/// The issuers `signer` needs that `sent` lacks, fetched from where each
/// certificate says its issuer is, up to a few.
pub fn issuers(signer: &Certificate, sent: &[Certificate], fetch: &dyn Fetch) -> Vec<Certificate> {
    let mut found: Vec<Certificate> = Vec::new();
    let mut cert = signer.clone();
    for _ in 0..4 {
        let tbs = &cert.tbs_certificate;
        if tbs.subject == tbs.issuer {
            break;
        }
        let known = sent
            .iter()
            .chain(&found)
            .find(|c| c.tbs_certificate.subject == tbs.issuer)
            .cloned();
        let next = match known {
            Some(c) => c,
            None => {
                let Some(fetched) = urls(&cert, oid::CA_ISSUERS)
                    .iter()
                    .filter_map(|url| fetch.get(url).ok())
                    .flat_map(|bytes| certificates(&bytes))
                    .find(|c| c.tbs_certificate.subject == tbs.issuer)
                else {
                    break;
                };
                found.push(fetched.clone());
                fetched
            }
        };
        cert = next;
    }
    found
}

/// The certificates in `bytes`: one in DER or PEM, or several in a
/// PKCS#7 bundle (.p7c), as issuers' certificates are published.
fn certificates(bytes: &[u8]) -> Vec<Certificate> {
    if let Ok(cert) = Certificate::from_der(bytes) {
        return vec![cert];
    }
    if let Ok(certs) = Certificate::load_pem_chain(bytes) {
        return certs;
    }
    let Ok(info) = cms::content_info::ContentInfo::from_der(bytes) else {
        return Vec::new();
    };
    let Ok(signed) = info.content.decode_as::<cms::signed_data::SignedData>() else {
        return Vec::new();
    };
    signed
        .certificates
        .map(|set| {
            set.0
                .iter()
                .filter_map(|c| match c {
                    cms::cert::CertificateChoices::Certificate(c) => Some(c.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The HTTP addresses `cert`'s Authority Information Access gives for
/// `method` (OCSP, or its issuer's certificate).
fn urls(cert: &Certificate, method: const_oid::ObjectIdentifier) -> Vec<String> {
    let Some(aia) = extension::<AuthorityInfoAccessSyntax>(cert, oid::AUTHORITY_INFO_ACCESS) else {
        return Vec::new();
    };
    aia.0
        .into_iter()
        .filter(|a| a.access_method == method)
        .filter_map(|a| uri(&a.access_location))
        .collect()
}

/// Where `cert`'s CRLs are, over HTTP.
fn crl_urls(cert: &Certificate) -> Vec<String> {
    let Some(points) = extension::<CrlDistributionPoints>(cert, oid::CRL_DISTRIBUTION_POINTS)
    else {
        return Vec::new();
    };
    points
        .0
        .into_iter()
        .filter_map(|p| match p.distribution_point {
            Some(DistributionPointName::FullName(names)) => Some(names),
            _ => None,
        })
        .flatten()
        .filter_map(|n| uri(&n))
        .collect()
}

fn uri(name: &GeneralName) -> Option<String> {
    match name {
        GeneralName::UniformResourceIdentifier(u) => {
            let u = u.to_string();
            (u.starts_with("http://") || u.starts_with("https://")).then_some(u)
        }
        _ => None,
    }
}

/// `cert`'s extension `id`, read as `T`.
fn extension<T: for<'a> Decode<'a>>(
    cert: &Certificate,
    id: const_oid::ObjectIdentifier,
) -> Option<T> {
    cert.tbs_certificate
        .extensions
        .iter()
        .flatten()
        .find(|e| e.extn_id == id)
        .and_then(|e| T::from_der(e.extn_value.as_bytes()).ok())
}

/// How OCSP names `cert`: by hashes of its issuer's name and key, and
/// its serial number.
fn cert_id(cert: &Certificate, issuer: &Certificate) -> Option<CertId> {
    let name = issuer.tbs_certificate.subject.to_der().ok()?;
    let key = issuer
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    Some(CertId {
        hash_algorithm: spki::AlgorithmIdentifierOwned {
            oid: oid::SHA1,
            parameters: Some(der::asn1::Null.into()),
        },
        issuer_name_hash: OctetString::new(sha1::Sha1::digest(&name).to_vec()).ok()?,
        issuer_key_hash: OctetString::new(sha1::Sha1::digest(key).to_vec()).ok()?,
        serial_number: cert.tbs_certificate.serial_number.clone(),
    })
}

/// What validation data `known` says of `cert`, if it says: an OCSP
/// answer or CRL from `issuer`, which is added to `kept`.
fn told(cert: &Certificate, issuer: &Certificate, known: &Dss, kept: &mut Dss) -> Option<Status> {
    let decided = |s: &Status| matches!(s, Status::Good | Status::Revoked(_));
    if let Some(id) = cert_id(cert, issuer) {
        for der in &known.ocsps {
            if let Some(status) = read_ocsp(der, &id, issuer).filter(decided) {
                kept.ocsps.push(der.clone());
                return Some(status);
            }
        }
    }
    for der in &known.crls {
        if let Some(status) = read_crl(der, cert, issuer).filter(decided) {
            kept.crls.push(der.clone());
            return Some(status);
        }
    }
    None
}

/// What `cert`'s issuer's OCSP responder says of it, and its answer
/// when the answer told.
fn ocsp(cert: &Certificate, issuer: &Certificate, fetch: &dyn Fetch) -> (Status, Option<Vec<u8>>) {
    let unknown = |why: String| (Status::Unknown(why), None);
    let urls = urls(cert, oid::OCSP);
    let Some(url) = urls.first() else {
        return unknown("it names no OCSP responder".into());
    };
    let Some(id) = cert_id(cert, issuer) else {
        return unknown("its issuer could not be read".into());
    };
    let request = OcspRequest {
        tbs_request: TbsRequest {
            version: Default::default(),
            requestor_name: None,
            request_list: vec![Request {
                req_cert: id.clone(),
                single_request_extensions: None,
            }],
            request_extensions: None,
        },
        optional_signature: None,
    };
    let Ok(body) = request.to_der() else {
        return unknown("the OCSP request could not be made".into());
    };
    let answer = match fetch.post(url, "application/ocsp-request", &body) {
        Ok(a) => a,
        Err(e) => return unknown(format!("{url}: {e}")),
    };
    match read_ocsp(&answer, &id, issuer) {
        Some(Status::Unknown(why)) => unknown(why),
        Some(status) => (status, Some(answer)),
        None => unknown(format!("{url}: the answer does not hold")),
    }
}

/// What OCSP answer `der` says of the certificate `id` names, if it is
/// signed by `issuer` or a responder `issuer` named for it.
fn read_ocsp(der: &[u8], id: &CertId, issuer: &Certificate) -> Option<Status> {
    let response = OcspResponse::from_der(der).ok()?;
    if response.response_status != x509_ocsp::OcspResponseStatus::Successful {
        return Some(Status::Unknown(format!(
            "the OCSP responder answered {:?}",
            response.response_status
        )));
    }
    let bytes = response.response_bytes?;
    if bytes.response_type != oid::OCSP_BASIC {
        return None;
    }
    let basic = BasicOcspResponse::from_der(bytes.response.as_bytes()).ok()?;
    let tbs = basic.tbs_response_data.to_der().ok()?;
    let signature = basic.signature.as_bytes()?;
    let holds = |key: &Certificate| signed_by(key, &basic.signature_algorithm, &tbs, signature);
    let signed = holds(issuer)
        || basic.certs.iter().flatten().any(|responder| {
            responder.tbs_certificate.issuer == issuer.tbs_certificate.subject
                && signed_by(
                    issuer,
                    &responder.signature_algorithm,
                    &responder.tbs_certificate.to_der().unwrap_or_default(),
                    responder.signature.as_bytes().unwrap_or_default(),
                )
                && may_answer_ocsp(responder)
                && holds(responder)
        });
    if !signed {
        return None;
    }
    let single = basic
        .tbs_response_data
        .responses
        .iter()
        .find(|r| r.cert_id == *id)?;
    Some(match &single.cert_status {
        CertStatus::Good(_) => Status::Good,
        CertStatus::Revoked(info) => Status::Revoked(info.revocation_time.0.to_system_time()),
        CertStatus::Unknown(_) => Status::Unknown("the OCSP responder does not know it".into()),
    })
}

/// Whether `cert` is meant for signing OCSP answers.
fn may_answer_ocsp(cert: &Certificate) -> bool {
    extension::<ExtendedKeyUsage>(cert, oid::EXTENDED_KEY_USAGE)
        .is_some_and(|eku| eku.0.contains(&oid::OCSP_SIGNING))
}

/// What `cert`'s issuer's CRL says of it, and the CRL when it told.
fn crl(cert: &Certificate, issuer: &Certificate, fetch: &dyn Fetch) -> (Status, Option<Vec<u8>>) {
    let urls = crl_urls(cert);
    if urls.is_empty() {
        return (Status::Unknown("it names no CRL".into()), None);
    }
    let mut why = String::new();
    for url in &urls {
        match fetch.get(url) {
            Ok(bytes) => match read_crl(&bytes, cert, issuer) {
                Some(status) => return (status, Some(bytes)),
                None => why = format!("{url}: the CRL does not hold"),
            },
            Err(e) => why = format!("{url}: {e}"),
        }
    }
    (Status::Unknown(why), None)
}

/// What CRL `bytes` (DER or PEM) says of `cert`, if `issuer` signed it.
/// Version 2 CRLs only, as certificate authorities have issued them for
/// decades.
fn read_crl(bytes: &[u8], cert: &Certificate, issuer: &Certificate) -> Option<Status> {
    use x509_cert::crl::CertificateList;
    let list = CertificateList::from_der(bytes).ok().or_else(|| {
        let (_, der) = der::pem::decode_vec(bytes).ok()?;
        CertificateList::from_der(&der).ok()
    })?;
    let tbs = &list.tbs_cert_list;
    if tbs.issuer != issuer.tbs_certificate.subject {
        return None;
    }
    let der = tbs.to_der().ok()?;
    if !signed_by(
        issuer,
        &list.signature_algorithm,
        &der,
        list.signature.as_bytes()?,
    ) {
        return None;
    }
    let serial = &cert.tbs_certificate.serial_number;
    Some(
        match tbs
            .revoked_certificates
            .iter()
            .flatten()
            .find(|r| r.serial_number == *serial)
        {
            Some(r) => Status::Revoked(r.revocation_date.to_system_time()),
            None => Status::Good,
        },
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The test PKI (`tests/data/pki`), served as its certificates say,
    /// at http://pki.katna.test/.
    pub(crate) struct Pki {
        pub files: HashMap<String, Vec<u8>>,
        /// Whether its OCSP responder answers.
        pub ocsp: bool,
    }

    pub(crate) fn pki_file(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/data/pki/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    impl Pki {
        pub fn new() -> Self {
            let files = ["root.cer", "inter.cer", "inter.crl", "root.crl"]
                .into_iter()
                .map(|f| (format!("http://pki.katna.test/{f}"), pki_file(f)))
                .collect();
            Pki { files, ocsp: true }
        }
    }

    impl Fetch for Pki {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.files
                .get(url)
                .cloned()
                .ok_or_else(|| "not found".into())
        }

        fn post(&self, url: &str, _: &str, body: &[u8]) -> Result<Vec<u8>, String> {
            if url != "http://pki.katna.test/ocsp" || !self.ocsp {
                return Err("connection refused".into());
            }
            let request = OcspRequest::from_der(body).map_err(|e| e.to_string())?;
            let serial = &request.tbs_request.request_list[0].req_cert.serial_number;
            for who in ["meera", "raj"] {
                let cert = Certificate::from_der(&pki_file(&format!("{who}.cer"))).unwrap();
                if cert.tbs_certificate.serial_number == *serial {
                    return Ok(pki_file(&format!("{who}.ocsp")));
                }
            }
            Err("unknown".into())
        }
    }

    fn cert(name: &str) -> Certificate {
        Certificate::from_der(&pki_file(name)).unwrap()
    }

    #[test]
    fn missing_issuers_are_fetched() {
        let meera = cert("meera.cer");
        let found = issuers(&meera, &[], &Pki::new());
        let names: Vec<String> = found
            .iter()
            .map(|c| common_name(&c.tbs_certificate.subject))
            .collect();
        assert_eq!(names, ["Katna Test PKI Signing CA", "Katna Test PKI Root"]);
        // Nothing to fetch when the issuer was sent.
        assert_eq!(issuers(&meera, &[cert("inter.cer")], &Pki::new()).len(), 1);
    }

    #[test]
    fn revoked_certificates_are_found_by_ocsp_or_crl() {
        let (inter, root) = (cert("inter.cer"), cert("root.cer"));
        let now = SystemTime::now();
        for ocsp in [true, false] {
            let pki = Pki { ocsp, ..Pki::new() };
            let good = [cert("meera.cer"), inter.clone(), root.clone()];
            assert_eq!(check(&good, now, &pki), Revocation::Good, "ocsp: {ocsp}");
            let revoked = [cert("raj.cer"), inter.clone(), root.clone()];
            assert!(
                matches!(
                    check(&revoked, now, &pki),
                    Revocation::Revoked { ref name, .. } if name == "Raj Kumar"
                ),
                "ocsp: {ocsp}"
            );
            // Revoked after the time asked about: it held then.
            let before = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
            assert_eq!(check(&revoked, before, &pki), Revocation::Good);
        }
        // Nobody answering: not known.
        let offline = Pki {
            files: HashMap::new(),
            ocsp: false,
        };
        let good = [cert("meera.cer"), inter, root];
        assert!(matches!(
            check(&good, now, &offline),
            Revocation::Unknown { ref name, .. } if name == "Meera Iyer"
        ));
    }

    /// Signs a one-page PDF with certificate file `p12`, and checks it
    /// offline, then online against the test PKI.
    fn signed_by_pki(p12: &str) -> (crate::SignatureCheck, crate::SignatureCheck) {
        use katna_pdf_cos::{Document, Placement, SignRequest, SigningInfo};
        let id = crate::p12::open(&pki_file(p12), "katna").unwrap();
        let doc = Document::load(crate::tsa::tests::pdf()).unwrap();
        let request = SignRequest::new(Placement::Invisible, SigningInfo::default());
        let file = crate::sign(&doc, &request, &id, crate::Digest::Sha256).unwrap();
        let doc = Document::load(file.clone()).unwrap();
        let mut roots = crate::Roots::new();
        roots.add_file(&pki_file("root.cer"));
        let offline = crate::check(&doc, &file, &roots).remove(0);
        let mut online = offline.clone();
        crate::check_online(&mut online, &roots, &Pki::new());
        (offline, online)
    }

    #[test]
    fn signers_are_checked_online() {
        let (offline, online) = signed_by_pki("meera.p12");
        // Only Meera's own certificate was sent: offline, her issuer is
        // missing; online it is fetched, and she is not revoked.
        assert!(matches!(
            offline.trust,
            Some(crate::Trust::Untrusted {
                complete: false,
                ..
            })
        ));
        assert_eq!(offline.revocation, None);
        assert_eq!(
            online.trust,
            Some(crate::Trust::Trusted {
                root: "Katna Test PKI Root".into()
            })
        );
        assert_eq!(online.revocation, Some(Revocation::Good));
        let (_, online) = signed_by_pki("raj.p12");
        assert!(online.trust.as_ref().is_some_and(crate::Trust::trusted));
        assert!(matches!(
            online.revocation,
            Some(Revocation::Revoked { ref name, .. }) if name == "Raj Kumar"
        ));
    }

    #[test]
    fn validation_data_kept_in_the_file_is_checked_offline() {
        use katna_pdf_cos::{Change, Coverage, Document, Placement, SignRequest, SigningInfo};
        let mut roots = crate::Roots::new();
        roots.add_file(&pki_file("root.cer"));
        let long_term = |p12: &str| {
            let id = crate::p12::open(&pki_file(p12), "katna").unwrap();
            let doc = Document::load(crate::tsa::tests::pdf()).unwrap();
            let request = SignRequest::new(Placement::Invisible, SigningInfo::default());
            let file = crate::sign(&doc, &request, &id, crate::Digest::Sha256).unwrap();
            let (file, untold) =
                crate::add_validation_data(&Document::load(file).unwrap(), &roots, &Pki::new())
                    .unwrap();
            let doc = Document::load(file.clone()).unwrap();
            (crate::check(&doc, &file, &roots).remove(0), untold, doc)
        };
        let (meera, untold, doc) = long_term("meera.p12");
        assert!(untold.is_empty(), "{untold:?}");
        let kept = doc.dss();
        assert_eq!((kept.certs.len(), kept.ocsps.len()), (3, 1));
        // Offline: the issuer and the OCSP answer come from the file.
        assert!(meera.intact());
        assert!(matches!(meera.coverage, Coverage::Revision(_)));
        assert_eq!(meera.later.iter().collect::<Vec<_>>(), [&Change::Dss]);
        assert!(meera.trust.as_ref().is_some_and(crate::Trust::trusted));
        assert_eq!(meera.revocation, Some(Revocation::Good));
        // Nothing to ask online any more.
        let mut again = meera.clone();
        crate::check_online(&mut again, &roots, &Unreachable);
        assert_eq!(again.revocation, Some(Revocation::Good));
        let (raj, untold, _) = long_term("raj.p12");
        assert_eq!(untold.len(), 1);
        assert!(matches!(raj.revocation, Some(Revocation::Revoked { .. })));
    }

    /// No network.
    struct Unreachable;

    impl Fetch for Unreachable {
        fn get(&self, _: &str) -> Result<Vec<u8>, String> {
            Err("offline".into())
        }
        fn post(&self, _: &str, _: &str, _: &[u8]) -> Result<Vec<u8>, String> {
            Err("offline".into())
        }
    }

    #[test]
    fn answers_not_signed_by_the_issuer_are_not_taken() {
        let (meera, inter) = (cert("meera.cer"), cert("inter.cer"));
        let id = cert_id(&meera, &inter).unwrap();
        // Raj's answer is for another certificate.
        let raj = cert("raj.cer");
        let raj_id = cert_id(&raj, &inter).unwrap();
        assert!(read_ocsp(&pki_file("raj.ocsp"), &id, &inter).is_none());
        assert!(matches!(
            read_ocsp(&pki_file("raj.ocsp"), &raj_id, &inter),
            Some(Status::Revoked(_))
        ));
        // As if another CA had issued Meera's certificate.
        let other = cert("root.cer");
        assert!(read_ocsp(&pki_file("meera.ocsp"), &id, &other).is_none());
        assert!(read_crl(&pki_file("inter.crl"), &meera, &other).is_none());
        let mut tampered = pki_file("meera.ocsp");
        let n = tampered.len();
        tampered[n - 10] ^= 1;
        assert!(read_ocsp(&tampered, &id, &inter).is_none());
    }
}
