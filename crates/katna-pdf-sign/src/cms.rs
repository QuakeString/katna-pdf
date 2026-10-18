// SPDX-License-Identifier: GPL-3.0-or-later

//! CMS signatures (RFC 5652) as PAdES makes them (ETSI EN 319 142-1,
//! B-B): detached, over the PDF's signed bytes, with the content type,
//! the message digest and the signer's certificate (signing-certificate
//! v2, RFC 5035) as signed attributes.
//!
//! Signatures are made with RustCrypto's types, and checked by walking
//! their DER bytes as they are: re-encoding a signature from other
//! software can change what was signed (attributes out of order).

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{
    CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos,
};
use const_oid::ObjectIdentifier;
use der::asn1::{OctetString, SetOfVec};
use der::{Any, Decode, Encode, Sequence};
use spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;
use x509_cert::attr::Attribute;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::serial_number::SerialNumber;

use crate::keys::{KeyKind, Signer, SignerError};
use crate::{Digest, Hash, oid, walk};

/// ESSCertIDv2 (RFC 5035): which certificate signed, by its hash.
#[derive(Clone, Debug, Sequence)]
struct EssCertIdV2 {
    /// Left out for SHA-256, the default, as DER requires.
    hash_algorithm: Option<AlgorithmIdentifierOwned>,
    cert_hash: OctetString,
    issuer_serial: Option<IssuerSerial>,
}

#[derive(Clone, Debug, Sequence)]
struct IssuerSerial {
    issuer: Vec<GeneralName>,
    serial_number: SerialNumber,
}

#[derive(Clone, Debug, Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

fn algorithm(oid: ObjectIdentifier) -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid,
        parameters: None,
    }
}

fn attribute(oid: ObjectIdentifier, value: &impl Encode) -> der::Result<Attribute> {
    Ok(Attribute {
        oid,
        values: SetOfVec::try_from(vec![Any::from_der(&value.to_der()?)?])?,
    })
}

#[derive(Debug)]
pub enum BuildError {
    Encoding(der::Error),
    Signer(SignerError),
}

impl From<der::Error> for BuildError {
    fn from(e: der::Error) -> Self {
        BuildError::Encoding(e)
    }
}

/// The CMS signature, DER, of a document whose signed bytes hash to
/// `document` with `digest`.
pub fn build(signer: &dyn Signer, digest: Digest, document: &[u8]) -> Result<Vec<u8>, BuildError> {
    build_signed(signer, digest, oid::DATA, None, document)
}

/// A CMS signature over content of type `content_type` that hashes to
/// `document`, carrying the content itself when `content` is given (as
/// a timestamp token carries its TSTInfo).
pub(crate) fn build_signed(
    signer: &dyn Signer,
    digest: Digest,
    content_type: ObjectIdentifier,
    content: Option<Vec<u8>>,
    document: &[u8],
) -> Result<Vec<u8>, BuildError> {
    let cert = signer.certificate();
    let kind = signer.kind();
    let cert_der = cert.to_der()?;
    let tbs = &cert.tbs_certificate;
    let ess = EssCertIdV2 {
        hash_algorithm: (digest != Digest::Sha256).then(|| algorithm(digest.oid())),
        cert_hash: OctetString::new(digest.hash(&[&cert_der]))?,
        issuer_serial: Some(IssuerSerial {
            issuer: vec![GeneralName::DirectoryName(tbs.issuer.clone())],
            serial_number: tbs.serial_number.clone(),
        }),
    };
    let signed_attrs = SetOfVec::try_from(vec![
        attribute(oid::CONTENT_TYPE, &content_type)?,
        attribute(oid::MESSAGE_DIGEST, &OctetString::new(document)?)?,
        attribute(
            oid::SIGNING_CERTIFICATE_V2,
            &SigningCertificateV2 { certs: vec![ess] },
        )?,
    ])?;
    // What is signed: the attributes as a SET, not as [0].
    let to_sign = signed_attrs.to_der()?;
    let signature = signer.sign(digest, &to_sign).map_err(BuildError::Signer)?;
    let signature_algorithm = AlgorithmIdentifierOwned {
        oid: kind.algorithm(digest),
        // RSA's algorithms take NULL parameters; ECDSA's none.
        parameters: (kind == KeyKind::Rsa).then(Any::null),
    };
    let info = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: tbs.issuer.clone(),
            serial_number: tbs.serial_number.clone(),
        }),
        digest_alg: algorithm(digest.oid()),
        signed_attrs: Some(signed_attrs),
        signature_algorithm,
        signature: OctetString::new(signature)?,
        unsigned_attrs: None,
    };
    let mut certs = vec![CertificateChoices::Certificate(cert.clone())];
    certs.extend(
        signer
            .chain()
            .iter()
            .cloned()
            .map(CertificateChoices::Certificate),
    );
    let econtent = match content {
        Some(c) => Some(Any::new(der::Tag::OctetString, c)?),
        None => None,
    };
    let data = SignedData {
        // Content of a type other than data takes version 3 (RFC 5652
        // §5.1).
        version: if content_type == oid::DATA {
            CmsVersion::V1
        } else {
            CmsVersion::V3
        },
        digest_algorithms: SetOfVec::try_from(vec![algorithm(digest.oid())])?,
        encap_content_info: EncapsulatedContentInfo {
            econtent_type: content_type,
            econtent,
        },
        certificates: Some(CertificateSet(SetOfVec::try_from(certs)?)),
        crls: None,
        signer_infos: SignerInfos(SetOfVec::try_from(vec![info])?),
    };
    Ok(ContentInfo {
        content_type: oid::SIGNED_DATA,
        content: Any::encode_from(&data)?,
    }
    .to_der()?)
}

/// The signature value of CMS signature `der`, which a timestamp is
/// made over (RFC 3161 Appendix A).
pub(crate) fn signature_value(der: &[u8]) -> Result<Vec<u8>, der::Error> {
    let info = ContentInfo::from_der(der)?;
    let data = info.content.decode_as::<SignedData>()?;
    let signer = data.signer_infos.0.get(0).ok_or(der::ErrorKind::Failed)?;
    Ok(signer.signature.as_bytes().to_vec())
}

/// CMS signature `der` with timestamp token `token` added to its
/// signer as an unsigned attribute.
pub(crate) fn add_timestamp(der: &[u8], token: &[u8]) -> Result<Vec<u8>, der::Error> {
    let info = ContentInfo::from_der(der)?;
    let mut data = info.content.decode_as::<SignedData>()?;
    let mut infos: Vec<SignerInfo> = data.signer_infos.0.into_vec();
    let signer = infos.first_mut().ok_or(der::ErrorKind::Failed)?;
    signer.unsigned_attrs = Some(SetOfVec::try_from(vec![Attribute {
        oid: oid::TIMESTAMP_TOKEN,
        values: SetOfVec::try_from(vec![Any::from_der(token)?])?,
    }])?);
    data.signer_infos = SignerInfos(SetOfVec::try_from(infos)?);
    ContentInfo {
        content_type: oid::SIGNED_DATA,
        content: Any::encode_from(&data)?,
    }
    .to_der()
}

/// A timestamp on a signature: when, by whom, and whether it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// When, as the timestamp authority states it (UTC).
    pub time: Option<String>,
    /// The timestamp authority, by its certificate's common name.
    pub authority: Option<String>,
    /// The token is the authority's, and is over this signature.
    pub valid: bool,
}

/// The content timestamp token `token` (DER) carries: its TSTInfo.
fn token_content(token: &[u8]) -> Option<Vec<u8>> {
    let (info, _) = walk::read(token)?;
    let [_, content] = walk::children(info.value)?[..] else {
        return None;
    };
    let (signed_data, _) = walk::read(content.value)?;
    let fields = walk::children(signed_data.value)?;
    let encap = fields.get(2)?;
    let [kind, wrapped] = walk::children(encap.value)?[..] else {
        return None;
    };
    if walk::oid(&kind) != Some(oid::TST_INFO) {
        return None;
    }
    let (octets, _) = walk::read(wrapped.value)?;
    Some(octets.value.to_vec())
}

/// What timestamp token `token` (DER) states.
pub(crate) fn token_info(token: &[u8]) -> Option<x509_tsp::TstInfo> {
    x509_tsp::TstInfo::from_der(&token_content(token)?).ok()
}

/// Checks timestamp token `token` (DER) over `signature`, the signature
/// value it should be made on.
fn check_stamp(token: &[u8], signature: &[u8]) -> Stamp {
    let none = Stamp {
        time: None,
        authority: None,
        valid: false,
    };
    let Some(content) = token_content(token) else {
        return none;
    };
    let Ok(tst) = x509_tsp::TstInfo::from_der(&content) else {
        return none;
    };
    let time = katna_pdf_cos::pdf_date(tst.gen_time.to_system_time());
    let time = crate::readable_pdf_date(&time);
    let imprint = &tst.message_imprint;
    let imprint_matches = Hash::from_oid(imprint.hash_algorithm.oid)
        .is_some_and(|h| h.hash(&[signature]) == imprint.hashed_message.as_bytes());
    match check(token, &[&content]) {
        Ok(c) => Stamp {
            time,
            authority: Some(common_name(&c.signer.tbs_certificate.subject)),
            valid: imprint_matches
                && c.digest_matches
                && c.signature_valid
                && c.certificate_named != Some(false),
        },
        Err(_) => Stamp { time, ..none },
    }
}

/// What a CMS signature says, and whether it holds.
#[derive(Debug, Clone)]
pub struct Checked {
    /// The signer's certificate.
    pub signer: Certificate,
    /// The certificates sent with the signature.
    pub certificates: Vec<Certificate>,
    /// The signed bytes hash to the digest the signature signed.
    pub digest_matches: bool,
    /// The signature is the signer's key's over what it signs.
    pub signature_valid: bool,
    /// The signer's certificate is the one the signature names
    /// (signing-certificate v2 or v1), when it names one.
    pub certificate_named: Option<bool>,
    /// The signing time the signature itself states, if any (UTC).
    pub signing_time: Option<String>,
    /// The timestamp it carries, checked, if any.
    pub timestamp: Option<Stamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// Not a CMS signature, or one encoded in a way not read here.
    Malformed,
    /// Made with a digest or key not checked here.
    Unsupported(String),
    /// The certificate that signed is not in the signature.
    NoCertificate,
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Malformed => f.write_str("the signature is damaged or not readable"),
            CheckError::Unsupported(what) => write!(f, "the signature uses {what}"),
            CheckError::NoCertificate => {
                f.write_str("the signer's certificate is not in the signature")
            }
        }
    }
}

impl std::error::Error for CheckError {}

/// Checks CMS signature `der` over `parts`, the bytes it says it signs.
pub fn check(der: &[u8], parts: &[&[u8]]) -> Result<Checked, CheckError> {
    use CheckError::Malformed;
    let (info, _) = walk::read(der).ok_or(Malformed)?;
    let [content_type, content] = walk::children(info.value).ok_or(Malformed)?[..] else {
        return Err(Malformed);
    };
    if walk::oid(&content_type) != Some(oid::SIGNED_DATA) {
        return Err(Malformed);
    }
    let (signed_data, _) = walk::read(content.value).ok_or(Malformed)?;
    let fields = walk::children(signed_data.value).ok_or(Malformed)?;
    let certificates: Vec<Certificate> = fields
        .iter()
        .find(|f| f.tag == 0xa0)
        .and_then(|f| walk::children(f.value))
        .unwrap_or_default()
        .iter()
        .filter(|c| c.tag == 0x30)
        .filter_map(|c| Certificate::from_der(c.raw).ok())
        .collect();
    let signer_infos = fields.last().filter(|f| f.tag == 0x31).ok_or(Malformed)?;
    let signer_info = walk::children(signer_infos.value)
        .and_then(|s| s.into_iter().next())
        .ok_or(Malformed)?;
    let parts_of = walk::children(signer_info.value).ok_or(Malformed)?;
    let mut it = parts_of.iter();
    let _version = it.next().ok_or(Malformed)?;
    let sid = it.next().ok_or(Malformed)?;
    let digest_alg = it.next().ok_or(Malformed)?;
    let mut next = it.next().ok_or(Malformed)?;
    let signed_attrs = if next.tag == 0xa0 {
        let attrs = *next;
        next = it.next().ok_or(Malformed)?;
        Some(attrs)
    } else {
        None
    };
    let signature_alg = next;
    let signature = it.next().filter(|s| s.tag == 0x04).ok_or(Malformed)?;
    let timestamp = it
        .next()
        .filter(|u| u.tag == 0xa1)
        .and_then(|u| walk::children(u.value))
        .and_then(|attrs| {
            attrs.into_iter().find_map(|a| {
                let [kind, values] = walk::children(a.value)?[..] else {
                    return None;
                };
                if walk::oid(&kind) != Some(oid::TIMESTAMP_TOKEN) {
                    return None;
                }
                let token = walk::children(values.value)?.into_iter().next()?;
                Some(check_stamp(token.raw, signature.value))
            })
        });

    let digest_oid = walk::algorithm(digest_alg).ok_or(Malformed)?;
    let hash = Hash::from_oid(digest_oid)
        .ok_or_else(|| CheckError::Unsupported(format!("the digest {digest_oid}")))?;
    let signer = find_signer(sid, &certificates).ok_or(CheckError::NoCertificate)?;
    let document = hash.hash(parts);

    let (digest_matches, certificate_named, signing_time, message) = match signed_attrs {
        Some(attrs) => {
            let mut digest_matches = false;
            let mut named = None;
            let mut time = None;
            for attr in walk::children(attrs.value).ok_or(Malformed)? {
                let [kind, values] = walk::children(attr.value).ok_or(Malformed)?[..] else {
                    return Err(Malformed);
                };
                let Some(value) = walk::children(values.value).and_then(|v| v.into_iter().next())
                else {
                    continue;
                };
                match walk::oid(&kind) {
                    Some(o) if o == oid::MESSAGE_DIGEST => {
                        digest_matches = value.tag == 0x04 && value.value == document.as_slice();
                    }
                    Some(o) if o == oid::SIGNING_CERTIFICATE_V2 => {
                        named = Some(names(&value, &signer, true));
                    }
                    Some(o) if o == oid::SIGNING_CERTIFICATE && named.is_none() => {
                        named = Some(names(&value, &signer, false));
                    }
                    Some(o) if o == oid::SIGNING_TIME => time = walk::time(&value),
                    _ => {}
                }
            }
            // Signed as a SET.
            let mut message = attrs.raw.to_vec();
            message[0] = 0x31;
            (digest_matches, named, time, message)
        }
        // Without attributes the signature is over the bytes themselves.
        None => (true, None, None, parts.concat()),
    };
    let signature_valid = verify(&signer, signature_alg, hash, &message, signature.value)?;
    Ok(Checked {
        signer,
        certificates,
        digest_matches,
        signature_valid,
        certificate_named,
        signing_time,
        timestamp,
    })
}

/// The certificate `sid` names.
fn find_signer(sid: &walk::Tlv<'_>, certificates: &[Certificate]) -> Option<Certificate> {
    match sid.tag {
        // IssuerAndSerialNumber.
        0x30 => {
            let [issuer, serial] = walk::children(sid.value)?[..] else {
                return None;
            };
            certificates
                .iter()
                .find(|c| {
                    let tbs = &c.tbs_certificate;
                    tbs.issuer.to_der().is_ok_and(|d| d == issuer.raw)
                        && tbs.serial_number.to_der().is_ok_and(|d| d == serial.raw)
                })
                .cloned()
        }
        // [0] SubjectKeyIdentifier.
        0x80 => certificates
            .iter()
            .find(|c| key_id(c).is_some_and(|id| id == sid.value))
            .cloned(),
        _ => None,
    }
}

/// A certificate's subject key identifier extension.
fn key_id(cert: &Certificate) -> Option<Vec<u8>> {
    use x509_cert::ext::pkix::SubjectKeyIdentifier;
    let ext = cert
        .tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|e| e.extn_id == const_oid::db::rfc5280::ID_CE_SUBJECT_KEY_IDENTIFIER)?;
    let id = SubjectKeyIdentifier::from_der(ext.extn_value.as_bytes()).ok()?;
    Some(id.0.as_bytes().to_vec())
}

/// Whether signing-certificate attribute `value` names `cert` first.
fn names(value: &walk::Tlv<'_>, cert: &Certificate, v2: bool) -> bool {
    let Ok(der) = cert.to_der() else {
        return false;
    };
    let first = walk::children(value.value)
        .and_then(|c| c.into_iter().next())
        .and_then(|certs| walk::children(certs.value))
        .and_then(|c| c.into_iter().next());
    let Some(id) = first.and_then(|id| walk::children(id.value)) else {
        return false;
    };
    let (hash, cert_hash) = match (v2, &id[..]) {
        (false, [h, ..]) => (Hash::Sha1, h),
        (true, [alg, h, ..]) if alg.tag == 0x30 => {
            match walk::algorithm(alg).and_then(Hash::from_oid) {
                Some(hash) => (hash, h),
                None => return false,
            }
        }
        (true, [h, ..]) => (Hash::Sha256, h),
        _ => return false,
    };
    hash.hash(&[&der]) == cert_hash.value
}

/// Whether `signature` is `cert`'s key's over `message`.
fn verify(
    cert: &Certificate,
    alg: &walk::Tlv<'_>,
    digest: Hash,
    message: &[u8],
    signature: &[u8],
) -> Result<bool, CheckError> {
    let alg_oid = walk::algorithm(alg).ok_or(CheckError::Malformed)?;
    let spki = &cert.tbs_certificate.subject_public_key_info;
    let key_der = spki.to_der().map_err(|_| CheckError::Malformed)?;
    let unsupported = || CheckError::Unsupported(format!("the signature algorithm {alg_oid}"));
    // What is signed is hashed with the signer's digest algorithm, as
    // OpenSSL and NSS do, whatever digest the signature algorithm names.
    let hash = digest;
    if spki.algorithm.oid == oid::RSA {
        use rsa::pkcs8::DecodePublicKey as _;
        let key =
            rsa::RsaPublicKey::from_public_key_der(&key_der).map_err(|_| CheckError::Malformed)?;
        let hashed = hash.hash(&[message]);
        if alg_oid == oid::RSA_PSS {
            let (hash, salt) = pss_params(alg).ok_or_else(unsupported)?;
            let hashed = hash.hash(&[message]);
            return Ok(key.verify(hash.pss(salt), &hashed, signature).is_ok());
        }
        let rsa_algs = [
            oid::RSA,
            oid::SHA1_RSA,
            oid::SHA256_RSA,
            oid::SHA384_RSA,
            oid::SHA512_RSA,
        ];
        if !rsa_algs.contains(&alg_oid) {
            return Err(unsupported());
        }
        return Ok(key.verify(hash.pkcs1v15(), &hashed, signature).is_ok());
    }
    if spki.algorithm.oid == oid::EC_PUBLIC_KEY {
        use rsa::pkcs8::DecodePublicKey as _;
        use rsa::signature::hazmat::PrehashVerifier as _;
        let hashed = hash.hash(&[message]);
        let curve = spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())
            .ok_or(CheckError::Malformed)?;
        if curve == oid::P256 {
            let key = p256::ecdsa::VerifyingKey::from_public_key_der(&key_der)
                .map_err(|_| CheckError::Malformed)?;
            let Ok(sig) = p256::ecdsa::Signature::from_der(signature) else {
                return Ok(false);
            };
            return Ok(key.verify_prehash(&hashed, &sig).is_ok());
        }
        if curve == oid::P384 {
            let key = p384::ecdsa::VerifyingKey::from_public_key_der(&key_der)
                .map_err(|_| CheckError::Malformed)?;
            let Ok(sig) = p384::ecdsa::Signature::from_der(signature) else {
                return Ok(false);
            };
            return Ok(key.verify_prehash(&hashed, &sig).is_ok());
        }
        return Err(CheckError::Unsupported(format!("the curve {curve}")));
    }
    Err(CheckError::Unsupported(format!(
        "the key type {}",
        spki.algorithm.oid
    )))
}

/// RSASSA-PSS parameters (RFC 4055): the digest and the salt length;
/// SHA-1 and 20 when left out.
fn pss_params(alg: &walk::Tlv<'_>) -> Option<(Hash, usize)> {
    let fields = walk::children(alg.value)?;
    let mut hash = Hash::Sha1;
    let mut salt = 20;
    if let Some(params) = fields.get(1).filter(|p| p.tag == 0x30) {
        for field in walk::children(params.value)? {
            let (inner, _) = walk::read(field.value)?;
            match field.tag {
                0xa0 => hash = Hash::from_oid(walk::algorithm(&inner)?)?,
                0xa2 => salt = walk::uint(&inner)?,
                _ => {}
            }
        }
    }
    Some((hash, salt))
}

/// A certificate's common name (or its organisation), for people.
pub fn common_name(name: &x509_cert::name::Name) -> String {
    let mut org = None;
    for rdn in name.0.iter() {
        for atv in rdn.0.iter() {
            let text = atv
                .value
                .decode_as::<String>()
                .ok()
                .or_else(|| {
                    atv.value
                        .decode_as::<der::asn1::PrintableStringRef<'_>>()
                        .ok()
                        .map(|s| s.as_str().to_owned())
                })
                .or_else(|| {
                    atv.value
                        .decode_as::<der::asn1::Ia5StringRef<'_>>()
                        .ok()
                        .map(|s| s.as_str().to_owned())
                });
            if atv.oid == oid::COMMON_NAME
                && let Some(text) = text
            {
                return text;
            }
            if atv.oid == oid::ORGANIZATION && org.is_none() {
                org = text;
            }
        }
    }
    org.unwrap_or_else(|| name.to_string())
}
