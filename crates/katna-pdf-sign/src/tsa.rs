// SPDX-License-Identifier: GPL-3.0-or-later

//! Timestamps (RFC 3161) for PAdES B-T: a timestamp authority's signed
//! statement that a signature existed at a time, so the signature can
//! be trusted after the signer's certificate expires.

use std::time::Duration;

use der::asn1::{Int, OctetString};
use der::{Decode, Encode};
use rand_core::{OsRng, RngCore};
use spki::AlgorithmIdentifier;
use x509_tsp::{MessageImprint, TimeStampReq, TimeStampResp, TspVersion};

use crate::Digest;

/// Why no timestamp was had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsaError {
    /// The server could not be reached, or answered with an HTTP error.
    Network(String),
    /// The server refused, with its reason.
    Refused(String),
    /// The answer is not a timestamp, or not for what was asked.
    Malformed,
}

impl std::fmt::Display for TsaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TsaError::Network(e) => write!(f, "the timestamp server could not be reached: {e}"),
            TsaError::Refused(e) => write!(f, "the timestamp server refused: {e}"),
            TsaError::Malformed => f.write_str("the timestamp server's answer is not readable"),
        }
    }
}

impl std::error::Error for TsaError {}

impl From<der::Error> for TsaError {
    fn from(_: der::Error) -> Self {
        TsaError::Malformed
    }
}

/// Something that timestamps: a server, or in tests a key at hand.
pub trait Stamper {
    /// A timestamp token (DER) over `data`, hashed with `digest`.
    fn stamp(&self, digest: Digest, data: &[u8]) -> Result<Vec<u8>, TsaError>;
}

/// A timestamp server, by its URL.
#[derive(Debug, Clone)]
pub struct Server {
    pub url: String,
    pub timeout: Duration,
}

impl Server {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            timeout: Duration::from_secs(30),
        }
    }
}

impl Stamper for Server {
    fn stamp(&self, digest: Digest, data: &[u8]) -> Result<Vec<u8>, TsaError> {
        let nonce = nonce();
        let body = request(digest, data, &nonce)?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(self.timeout))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .new_agent();
        let network = |e: ureq::Error| TsaError::Network(e.to_string());
        let mut answer = agent
            .post(&self.url)
            .header("Content-Type", "application/timestamp-query")
            .send(&body[..])
            .map_err(network)?;
        let bytes = answer
            .body_mut()
            .with_config()
            .limit(1 << 20)
            .read_to_vec()
            .map_err(network)?;
        token(&bytes, digest, data, &nonce)
    }
}

/// A random nonce, so an old answer cannot be passed off as new.
fn nonce() -> Vec<u8> {
    let mut bytes = [0u8; 9];
    OsRng.fill_bytes(&mut bytes[1..]);
    // A positive INTEGER in its shortest form: the leading zero byte
    // is needed, as the next has its top bit set.
    bytes[1] |= 0x80;
    bytes.to_vec()
}

/// A timestamp request (DER) for `data` hashed with `digest`, asking
/// for the authority's certificate in the token.
pub fn request(digest: Digest, data: &[u8], nonce: &[u8]) -> Result<Vec<u8>, TsaError> {
    Ok(TimeStampReq {
        version: TspVersion::V1,
        message_imprint: MessageImprint {
            hash_algorithm: AlgorithmIdentifier {
                oid: digest.oid(),
                parameters: None,
            },
            hashed_message: OctetString::new(digest.hash(&[data]))?,
        },
        req_policy: None,
        nonce: Some(Int::new(nonce)?),
        cert_req: true,
        extensions: None,
    }
    .to_der()?)
}

/// The token in timestamp answer `response` (DER), once it is granted
/// for `data` hashed with `digest` and carries `nonce`.
pub fn token(
    response: &[u8],
    digest: Digest,
    data: &[u8],
    nonce: &[u8],
) -> Result<Vec<u8>, TsaError> {
    let answer = TimeStampResp::from_der(response)?;
    // 0 granted, 1 granted with changes.
    let status = answer.status.status as u32;
    if status > 1 {
        let text = answer
            .status
            .status_string
            .as_ref()
            .and_then(|s| s.iter().next().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("status {status}"));
        return Err(TsaError::Refused(text));
    }
    let token = answer.time_stamp_token.ok_or(TsaError::Malformed)?;
    let der = token.to_der()?;
    let info = crate::cms::token_info(&der).ok_or(TsaError::Malformed)?;
    let imprint = &info.message_imprint;
    let nonce_matches = info
        .nonce
        .as_ref()
        .is_some_and(|n| Int::new(nonce).as_ref() == Ok(n));
    if imprint.hashed_message.as_bytes() != digest.hash(&[data]) || !nonce_matches {
        return Err(TsaError::Malformed);
    }
    Ok(der)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use der::asn1::GeneralizedTime;
    use katna_pdf_cos::{Document, Placement, SignRequest, SigningInfo};

    use super::*;
    use crate::p12::{self, Identity};
    use crate::{cms, oid};

    fn data(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// A timestamp authority at hand: the test TSA's key, stamping
    /// what it is given (or, when `wrong`, something else).
    struct Local {
        id: Identity,
        nonce: Option<Vec<u8>>,
        wrong: bool,
    }

    impl Local {
        fn new() -> Self {
            Self {
                id: p12::open(&data("tsa.p12"), "katna").unwrap(),
                nonce: None,
                wrong: false,
            }
        }

        fn token(&self, digest: Digest, data: &[u8]) -> Vec<u8> {
            let data = if self.wrong { b"other" } else { data };
            let info = x509_tsp::TstInfo {
                version: TspVersion::V1,
                policy: const_oid::ObjectIdentifier::new_unwrap("1.3.6.1.4.1.99999.1"),
                message_imprint: MessageImprint {
                    hash_algorithm: AlgorithmIdentifier {
                        oid: digest.oid(),
                        parameters: None,
                    },
                    hashed_message: OctetString::new(digest.hash(&[data])).unwrap(),
                },
                serial_number: Int::new(&[7]).unwrap(),
                gen_time: GeneralizedTime::from_unix_duration(Duration::from_secs(1_792_327_845))
                    .unwrap(),
                accuracy: None,
                ordering: false,
                nonce: self.nonce.as_ref().map(|n| Int::new(n).unwrap()),
                tsa: None,
                extensions: None,
            }
            .to_der()
            .unwrap();
            let hash = digest.hash(&[&info]);
            cms::build_signed(&self.id, digest, oid::TST_INFO, Some(info), &hash).unwrap()
        }
    }

    impl Stamper for Local {
        fn stamp(&self, digest: Digest, data: &[u8]) -> Result<Vec<u8>, TsaError> {
            Ok(self.token(digest, data))
        }
    }

    fn pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            write!(out, "{} 0 obj\n{o}\nendobj\n", i + 1).unwrap();
        }
        let xref = out.len();
        write!(out, "xref\n0 4\n0000000000 65535 f \n").unwrap();
        for o in offsets {
            writeln!(out, "{o:010} 00000 n ").unwrap();
        }
        write!(
            out,
            "trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .unwrap();
        out
    }

    fn signed(stamper: &Local) -> crate::SignatureCheck {
        let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
        let doc = Document::load(pdf()).unwrap();
        let request = SignRequest::new(Placement::Invisible, SigningInfo::default());
        let file = crate::sign_with(&doc, &request, &id, Digest::Sha256, Some(stamper)).unwrap();
        let doc = Document::load(file.clone()).unwrap();
        crate::check(&doc, &file).remove(0)
    }

    #[test]
    fn signatures_carry_timestamps_that_are_checked() {
        let c = signed(&Local::new());
        assert!(c.intact(), "{c:?}");
        let stamp = c.result.unwrap().timestamp.unwrap();
        assert!(stamp.valid);
        assert_eq!(stamp.authority.as_deref(), Some("Katna Test TSA"));
        assert_eq!(stamp.time.as_deref(), Some("2026-10-18 12:50:45 UTC"));

        // A token over something else: the signature holds, its
        // timestamp does not.
        let c = signed(&Local {
            wrong: true,
            ..Local::new()
        });
        assert!(c.intact());
        assert!(!c.result.unwrap().timestamp.unwrap().valid);
    }

    /// A timestamp answer: `status`, and `token` if any.
    fn answer(status: u8, token: Option<&[u8]>) -> Vec<u8> {
        let mut body = vec![0x30, 0x03, 0x02, 0x01, status];
        body.extend(token.unwrap_or_default());
        let mut out = vec![0x30];
        out.extend(der::Length::try_from(body.len()).unwrap().to_der().unwrap());
        out.extend(body);
        out
    }

    #[test]
    fn answers_are_taken_only_when_they_fit_the_request() {
        let nonce = nonce();
        assert!(request(Digest::Sha256, b"sig", &nonce).is_ok());
        let tsa = Local {
            nonce: Some(nonce.clone()),
            ..Local::new()
        };
        let token = tsa.token(Digest::Sha256, b"sig");
        let ok = answer(0, Some(&token));
        assert_eq!(
            super::token(&ok, Digest::Sha256, b"sig", &nonce),
            Ok(token.clone())
        );
        // For other data, with another nonce, or refused.
        assert_eq!(
            super::token(&ok, Digest::Sha256, b"other", &nonce),
            Err(TsaError::Malformed)
        );
        assert_eq!(
            super::token(&ok, Digest::Sha256, b"sig", &[1, 2, 3]),
            Err(TsaError::Malformed)
        );
        assert!(matches!(
            super::token(&answer(2, None), Digest::Sha256, b"sig", &nonce),
            Err(TsaError::Refused(_))
        ));
    }
}
