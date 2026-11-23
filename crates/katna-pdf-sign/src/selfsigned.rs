// SPDX-License-Identifier: GPL-3.0-or-later

//! A digital ID of one's own: a new RSA key and a certificate it signs
//! itself, written as a certificate file (`.pfx`) with a password, as
//! Acrobat's "Create a new digital ID" makes one. Nobody vouches for
//! such an ID, so readers trust it only once they have been given its
//! certificate (the public half, written as a `.cer`) and chosen to.
//!
//! The file is written the way current tools write them: the key in a
//! PKCS#8 bag under PBES2 (PBKDF2-SHA256, AES-256-CBC), the certificate
//! in a plain bag, a localKeyId tying the two, and an HMAC-SHA256 MAC.

use std::time::Duration;

use const_oid::ObjectIdentifier;
use der::asn1::{Any, BitString, Ia5StringRef, OctetString, SetOfVec, Utf8StringRef};
use der::{Decode, Encode};
use hmac::{Hmac, Mac};
use pkcs12::kdf::{Pkcs12KeyType, derive_key_utf8};
use rand_core::{OsRng, RngCore};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use x509_cert::Certificate;
use x509_cert::attr::{Attribute, AttributeTypeAndValue};
use x509_cert::certificate::{TbsCertificate, Version};
use x509_cert::ext::pkix::{BasicConstraints, KeyUsage, KeyUsages, SubjectKeyIdentifier};
use x509_cert::ext::{AsExtension, Extension};
use x509_cert::name::{Name, RdnSequence, RelativeDistinguishedName};
use x509_cert::serial_number::SerialNumber;
use x509_cert::time::Validity;

use crate::keys::SoftKey;
use crate::{Digest, oid};

/// Who the ID says it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdName {
    pub name: String,
    pub email: String,
    pub organisation: String,
    pub unit: String,
    /// The two-letter country code (IN, GB…), or empty.
    pub country: String,
}

/// A new digital ID: the certificate file, and its certificate.
pub struct NewId {
    /// The `.pfx` file, locked with the password given.
    pub file: Vec<u8>,
    pub certificate: Certificate,
}

/// Why an ID could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// No name was given.
    NoName,
    /// No password was given: the key would lie open.
    NoPassword,
    /// The country is not two letters.
    Country,
    Failed(String),
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdError::NoName => f.write_str("a digital ID needs a name"),
            IdError::NoPassword => f.write_str("a digital ID needs a password"),
            IdError::Country => f.write_str("the country is two letters, such as IN"),
            IdError::Failed(why) => write!(f, "the ID could not be made: {why}"),
        }
    }
}

impl std::error::Error for IdError {}

impl From<der::Error> for IdError {
    fn from(e: der::Error) -> Self {
        IdError::Failed(e.to_string())
    }
}

/// How long a new ID is good for, as Acrobat's are: five years.
pub const YEARS: u64 = 5;

/// Rounds of PBKDF2 and of the MAC's key derivation.
const ROUNDS: u32 = 10_000;

const SHA256_WITH_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
const COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");
const COUNTRY: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.6");
const ORGANISATION: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.10");
const UNIT: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.11");
const EMAIL: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.1");
const LOCAL_KEY_ID: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.21");

/// Makes a new digital ID for `who`, locked with `password`: an RSA
/// 2048-bit key and a certificate for it, good for [`YEARS`] years.
pub fn new_id(who: &IdName, password: &str) -> Result<NewId, IdError> {
    if who.name.trim().is_empty() {
        return Err(IdError::NoName);
    }
    if password.is_empty() {
        return Err(IdError::NoPassword);
    }
    let country = who.country.trim().to_ascii_uppercase();
    if !country.is_empty()
        && (country.len() != 2 || !country.chars().all(|c| c.is_ascii_uppercase()))
    {
        return Err(IdError::Country);
    }
    let key =
        rsa::RsaPrivateKey::new(&mut OsRng, 2048).map_err(|e| IdError::Failed(e.to_string()))?;
    let certificate = certificate(who, &country, &key)?;
    let file = pfx(&key, &certificate, password)?;
    Ok(NewId { file, certificate })
}

/// The certificate for `key`, signed by itself.
fn certificate(
    who: &IdName,
    country: &str,
    key: &rsa::RsaPrivateKey,
) -> Result<Certificate, IdError> {
    use rsa::pkcs8::EncodePublicKey as _;
    let public = key
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| IdError::Failed(e.to_string()))?;
    let spki = SubjectPublicKeyInfoOwned::from_der(public.as_bytes())?;
    let name = name(who, country)?;
    let mut serial = [0u8; 16];
    OsRng.fill_bytes(&mut serial);
    serial[0] &= 0x7f; // positive
    serial[0] |= 0x01; // and no leading zero byte
    let signature = AlgorithmIdentifierOwned {
        oid: SHA256_WITH_RSA,
        parameters: Some(Any::null()),
    };
    let key_bits = spki.subject_public_key.raw_bytes();
    let key_id = SubjectKeyIdentifier(OctetString::new(Sha1::digest(key_bits).to_vec())?);
    let usage = KeyUsage(KeyUsages::DigitalSignature | KeyUsages::NonRepudiation);
    let basic = BasicConstraints {
        ca: false,
        path_len_constraint: None,
    };
    let extensions: Vec<Extension> = vec![
        basic.to_extension(&name, &[])?,
        usage.to_extension(&name, &[])?,
        key_id.to_extension(&name, &[])?,
    ];
    let tbs = TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::new(&serial)?,
        signature: signature.clone(),
        issuer: name.clone(),
        validity: Validity::from_now(Duration::from_secs(YEARS * 365 * 24 * 3600 + 24 * 3600))?,
        subject: name,
        subject_public_key_info: spki,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: Some(extensions),
    };
    let signed = SoftKey::Rsa(Box::new(key.clone()))
        .sign(Digest::Sha256, &tbs.to_der()?)
        .map_err(|e| IdError::Failed(e.to_string()))?;
    Ok(Certificate {
        tbs_certificate: tbs,
        signature_algorithm: signature,
        signature: BitString::from_bytes(&signed)?,
    })
}

/// The certificate's name: common name, unit, organisation, email and
/// country, those given.
fn name(who: &IdName, country: &str) -> Result<Name, IdError> {
    let mut rdns = Vec::new();
    let mut push = |oid: ObjectIdentifier, value: Any| -> Result<(), IdError> {
        let mut set = SetOfVec::new();
        set.insert(AttributeTypeAndValue { oid, value })?;
        rdns.push(RelativeDistinguishedName::from(set));
        Ok(())
    };
    if !country.is_empty() {
        push(
            COUNTRY,
            Any::new(der::Tag::PrintableString, country.as_bytes())?,
        )?;
    }
    let organisation = who.organisation.trim();
    if !organisation.is_empty() {
        push(ORGANISATION, Any::from(Utf8StringRef::new(organisation)?))?;
    }
    let unit = who.unit.trim();
    if !unit.is_empty() {
        push(UNIT, Any::from(Utf8StringRef::new(unit)?))?;
    }
    push(COMMON_NAME, Any::from(Utf8StringRef::new(who.name.trim())?))?;
    let email = who.email.trim();
    if !email.is_empty() && email.is_ascii() {
        push(EMAIL, Any::from(Ia5StringRef::new(email)?))?;
    }
    Ok(RdnSequence(rdns))
}

/// `key` and `certificate` as a certificate file locked with `password`.
fn pfx(
    key: &rsa::RsaPrivateKey,
    certificate: &Certificate,
    password: &str,
) -> Result<Vec<u8>, IdError> {
    use rsa::pkcs8::EncodePrivateKey as _;
    let failed = |e: &dyn std::fmt::Display| IdError::Failed(e.to_string());
    let cert_der = certificate.to_der()?;
    let local_id = Sha1::digest(&cert_der).to_vec();
    let attributes = || -> Result<SetOfVec<Attribute>, IdError> {
        let mut values = SetOfVec::new();
        values.insert(Any::encode_from(&OctetString::new(local_id.clone())?)?)?;
        let mut set = SetOfVec::new();
        set.insert(Attribute {
            oid: LOCAL_KEY_ID,
            values,
        })?;
        Ok(set)
    };

    // The certificate, in a plain bag.
    let cert_bag = pkcs12::cert_type::CertBag {
        cert_id: pkcs12::PKCS_12_X509_CERT_OID,
        cert_value: OctetString::new(cert_der)?,
    };
    let certs = vec![pkcs12::safe_bag::SafeBag {
        bag_id: pkcs12::PKCS_12_CERT_BAG_OID,
        bag_value: cert_bag.to_der()?,
        bag_attributes: Some(attributes()?),
    }];

    // The key, encrypted.
    let plain = key.to_pkcs8_der().map_err(|e| failed(&e))?;
    let mut salt = [0u8; 16];
    let mut iv = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut iv);
    let scheme: pkcs5::EncryptionScheme<'_> =
        pkcs5::pbes2::Parameters::pbkdf2_sha256_aes256cbc(ROUNDS, &salt, &iv)
            .map_err(|e| failed(&e))?
            .into();
    let sealed = scheme
        .encrypt(password.as_bytes(), plain.as_bytes())
        .map_err(|e| failed(&e))?;
    let shrouded = pkcs12::pbe_params::EncryptedPrivateKeyInfo {
        encryption_algorithm: AlgorithmIdentifierOwned::from_der(&scheme.to_der()?)?,
        encrypted_data: OctetString::new(sealed)?,
    };
    let keys = vec![pkcs12::safe_bag::SafeBag {
        bag_id: pkcs12::PKCS_12_PKCS8_KEY_BAG_OID,
        bag_value: shrouded.to_der()?,
        bag_attributes: Some(attributes()?),
    }];

    let data = |contents: Vec<u8>| -> Result<cms::content_info::ContentInfo, IdError> {
        Ok(cms::content_info::ContentInfo {
            content_type: oid::DATA,
            content: Any::encode_from(&OctetString::new(contents)?)?,
        })
    };
    let auth_safe = vec![data(certs.to_der()?)?, data(keys.to_der()?)?].to_der()?;

    // The MAC over it all, which tells a wrong password from damage.
    let mut mac_salt = [0u8; 16];
    OsRng.fill_bytes(&mut mac_salt);
    let mac_key =
        derive_key_utf8::<Sha256>(password, &mac_salt, Pkcs12KeyType::Mac, ROUNDS as i32, 32)
            .map_err(|e| failed(&e))?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&mac_key).map_err(|e| failed(&e))?;
    mac.update(&auth_safe);
    let digest = mac.finalize().into_bytes().to_vec();

    let pfx = pkcs12::pfx::Pfx {
        version: pkcs12::pfx::Version::V3,
        auth_safe: data(auth_safe)?,
        mac_data: Some(pkcs12::mac_data::MacData {
            mac: pkcs12::digest_info::DigestInfo {
                algorithm: AlgorithmIdentifierOwned {
                    oid: oid::SHA256,
                    parameters: Some(Any::null()),
                },
                digest: OctetString::new(digest)?,
            },
            mac_salt: OctetString::new(mac_salt.to_vec())?,
            iterations: ROUNDS as i32,
        }),
    };
    Ok(pfx.to_der()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_id_opens_and_signs() {
        let who = IdName {
            name: "Asha Verma".into(),
            email: "asha@example.in".into(),
            organisation: "Katna, Ltd.".into(),
            country: "in".into(),
            ..IdName::default()
        };
        let id = new_id(&who, "secret").expect("made");
        let opened = crate::p12::open(&id.file, "secret").expect("opens");
        assert_eq!(opened.certificate, id.certificate);
        assert!(opened.chain.is_empty());
        assert_eq!(
            crate::common_name(&opened.certificate.tbs_certificate.subject),
            "Asha Verma"
        );
        let subject = opened.certificate.tbs_certificate.subject.to_string();
        assert!(subject.contains("O=Katna\\, Ltd."), "{subject}");
        assert!(subject.contains("C=IN"), "{subject}");
        assert_eq!(
            crate::p12::open(&id.file, "wrong").err(),
            Some(crate::P12Error::Password)
        );
        // It signs, and the signature is its own.
        let tbs = opened.certificate.tbs_certificate.to_der().unwrap();
        use rsa::pkcs8::DecodePublicKey as _;
        use rsa::signature::Verifier as _;
        let public = rsa::RsaPublicKey::from_public_key_der(
            &opened
                .certificate
                .tbs_certificate
                .subject_public_key_info
                .to_der()
                .unwrap(),
        )
        .unwrap();
        let verifier = rsa::pkcs1v15::VerifyingKey::<Sha256>::new(public);
        let sig =
            rsa::pkcs1v15::Signature::try_from(opened.certificate.signature.raw_bytes()).unwrap();
        verifier.verify(&tbs, &sig).expect("self-signed");
        assert!(matches!(
            new_id(&IdName::default(), "x"),
            Err(IdError::NoName)
        ));
        assert!(matches!(new_id(&who, ""), Err(IdError::NoPassword)));
    }
}
