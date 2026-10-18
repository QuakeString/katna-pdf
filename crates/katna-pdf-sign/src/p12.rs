// SPDX-License-Identifier: GPL-3.0-or-later

//! Certificate files (`.pfx`, `.p12`; RFC 7292): the key and the
//! certificates in them, unlocked with their password. Read with the
//! ciphers such files are written with: AES through PBES2, as current
//! tools write them, and the older triple DES and RC2 that Windows and
//! many Indian certificate authorities still use.

use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, InnerIvInit, KeyIvInit};
use der::asn1::OctetString;
use der::{Decode, Encode};
use hmac::{Hmac, Mac};
use pkcs12::kdf::{Pkcs12KeyType, derive_key_utf8};
use pkcs12::pbe_params::{EncryptedPrivateKeyInfo, Pkcs12PbeParams};
use pkcs12::pfx::Pfx;
use pkcs12::safe_bag::SafeBag;
use sha1::Sha1;
use sha2::{Sha256, Sha384, Sha512};
use spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;

use crate::keys::SoftKey;
use crate::oid;

/// Why a certificate file cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P12Error {
    /// Not a certificate file, or a damaged one.
    Invalid,
    /// The password is wrong.
    Password,
    /// It is protected or holds a key in a way not read here.
    Unsupported(String),
    /// It holds no private key, or no certificate for it.
    NoKey,
}

impl std::fmt::Display for P12Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            P12Error::Invalid => f.write_str("the file is not a certificate file, or is damaged"),
            P12Error::Password => f.write_str("the password is wrong"),
            P12Error::Unsupported(what) => {
                write!(f, "the file uses {what}, which is not supported")
            }
            P12Error::NoKey => f.write_str("the file holds no private key with its certificate"),
        }
    }
}

impl std::error::Error for P12Error {}

impl From<der::Error> for P12Error {
    fn from(_: der::Error) -> Self {
        P12Error::Invalid
    }
}

/// What a certificate file holds: its key, the key's certificate, and
/// the other certificates (usually the chain up to the root).
pub struct Identity {
    pub key: SoftKey,
    pub certificate: Certificate,
    pub chain: Vec<Certificate>,
}

/// Opens certificate file `bytes` with `password`.
pub fn open(bytes: &[u8], password: &str) -> Result<Identity, P12Error> {
    let pfx = Pfx::from_der(bytes)?;
    if pfx.auth_safe.content_type != oid::DATA {
        return Err(P12Error::Unsupported("public-key integrity".into()));
    }
    let auth_safe = pfx.auth_safe.content.decode_as::<OctetString>()?;
    let auth_safe = auth_safe.as_bytes();
    if let Some(mac) = &pfx.mac_data {
        check_mac(mac, auth_safe, password)?;
    }
    let mut keys = Vec::new();
    let mut certs = Vec::new();
    for info in Vec::<cms::content_info::ContentInfo>::from_der(auth_safe)? {
        let contents = if info.content_type == oid::DATA {
            info.content.decode_as::<OctetString>()?.into_bytes()
        } else if info.content_type == oid::ENCRYPTED_DATA {
            let data = info
                .content
                .decode_as::<cms::encrypted_data::EncryptedData>()?;
            let enc = data.enc_content_info;
            let content = enc.encrypted_content.ok_or(P12Error::Invalid)?;
            decrypt(&enc.content_enc_alg, content.as_bytes(), password)?
        } else {
            continue;
        };
        bags(&contents, password, &mut keys, &mut certs)?;
    }
    // The key's certificate: the one whose public key is the key's.
    for key in keys {
        let public = key.public_key_bits();
        if let Some(i) = certs.iter().position(|c: &Certificate| {
            c.tbs_certificate
                .subject_public_key_info
                .subject_public_key
                .raw_bytes()
                == public
        }) {
            let certificate = certs.remove(i);
            return Ok(Identity {
                key,
                certificate,
                chain: certs,
            });
        }
    }
    Err(P12Error::NoKey)
}

/// The keys and certificates in safe contents `der`.
fn bags(
    der: &[u8],
    password: &str,
    keys: &mut Vec<SoftKey>,
    certs: &mut Vec<Certificate>,
) -> Result<(), P12Error> {
    for bag in Vec::<SafeBag>::from_der(der)? {
        // The bag's value is wrapped in [0] EXPLICIT.
        let value = der::Any::from_der(&bag.bag_value)?;
        let value = value.value();
        if bag.bag_id == pkcs12::PKCS_12_PKCS8_KEY_BAG_OID {
            let info = EncryptedPrivateKeyInfo::from_der(value)?;
            let plain = decrypt(
                &info.encryption_algorithm,
                info.encrypted_data.as_bytes(),
                password,
            )?;
            keys.push(SoftKey::from_pkcs8(&plain)?);
        } else if bag.bag_id == pkcs12::PKCS_12_KEY_BAG_OID {
            keys.push(SoftKey::from_pkcs8(value)?);
        } else if bag.bag_id == pkcs12::PKCS_12_CERT_BAG_OID {
            let cert = pkcs12::cert_type::CertBag::from_der(value)?;
            if cert.cert_id == pkcs12::PKCS_12_X509_CERT_OID {
                certs.push(Certificate::from_der(cert.cert_value.as_bytes())?);
            }
        } else if bag.bag_id == pkcs12::PKCS_12_SAFE_CONTENTS_BAG_OID {
            bags(value, password, keys, certs)?;
        }
    }
    Ok(())
}

/// The MAC over the contents, which tells a wrong password from a
/// damaged file.
fn check_mac(mac: &pkcs12::mac_data::MacData, data: &[u8], password: &str) -> Result<(), P12Error> {
    let salt = mac.mac_salt.as_bytes();
    let rounds = mac.iterations;
    let expected = mac.mac.digest.as_bytes();
    let alg = mac.mac.algorithm.oid;
    macro_rules! check {
        ($d:ty, $len:expr) => {{
            let key = derive_key_utf8::<$d>(password, salt, Pkcs12KeyType::Mac, rounds, $len)?;
            let mut m = <Hmac<$d> as Mac>::new_from_slice(&key).map_err(|_| P12Error::Invalid)?;
            m.update(data);
            m.verify_slice(expected).map_err(|_| P12Error::Password)
        }};
    }
    if alg == oid::SHA1 {
        check!(Sha1, 20)
    } else if alg == oid::SHA256 {
        check!(Sha256, 32)
    } else if alg == oid::SHA384 {
        check!(Sha384, 48)
    } else if alg == oid::SHA512 {
        check!(Sha512, 64)
    } else {
        Err(P12Error::Unsupported(format!("the MAC {alg}")))
    }
}

/// `data` decrypted with `password` by the scheme `alg` names.
fn decrypt(
    alg: &AlgorithmIdentifierOwned,
    data: &[u8],
    password: &str,
) -> Result<Vec<u8>, P12Error> {
    if alg.oid == oid::PBES2 {
        let der = alg.to_der()?;
        let scheme = pkcs5::EncryptionScheme::try_from(der.as_slice())
            .map_err(|_| P12Error::Unsupported("an unknown cipher".into()))?;
        return scheme
            .decrypt(password.as_bytes(), data)
            .map_err(|_| P12Error::Password);
    }
    let params = alg
        .parameters
        .as_ref()
        .ok_or(P12Error::Invalid)?
        .decode_as::<Pkcs12PbeParams>()?;
    let salt = params.salt.as_bytes();
    let rounds = params.iterations;
    let derive = |kind, len| derive_key_utf8::<Sha1>(password, salt, kind, rounds, len);
    let iv = derive(Pkcs12KeyType::Iv, 8)?;
    let plain = if alg.oid == pkcs12::PKCS_12_PBE_WITH_SHAAND3_KEY_TRIPLE_DES_CBC {
        let key = derive(Pkcs12KeyType::EncryptionKey, 24)?;
        cbc::Decryptor::<des::TdesEde3>::new_from_slices(&key, &iv)
            .map_err(|_| P12Error::Invalid)?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
    } else if alg.oid == pkcs12::PKCS_12_PBE_WITH_SHAAND2_KEY_TRIPLE_DES_CBC {
        let key = derive(Pkcs12KeyType::EncryptionKey, 16)?;
        cbc::Decryptor::<des::TdesEde2>::new_from_slices(&key, &iv)
            .map_err(|_| P12Error::Invalid)?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
    } else if alg.oid == pkcs12::PKCS_12_PBEWITH_SHAAND40_BIT_RC2_CBC
        || alg.oid == pkcs12::PKCS_12_PBE_WITH_SHAAND128_BIT_RC2_CBC
    {
        let bits = if alg.oid == pkcs12::PKCS_12_PBEWITH_SHAAND40_BIT_RC2_CBC {
            40
        } else {
            128
        };
        let key = derive(Pkcs12KeyType::EncryptionKey, bits / 8)?;
        let cipher = rc2::Rc2::new_with_eff_key_len(&key, bits);
        cbc::Decryptor::<rc2::Rc2>::inner_iv_slice_init(cipher, &iv)
            .map_err(|_| P12Error::Invalid)?
            .decrypt_padded_vec_mut::<Pkcs7>(data)
    } else {
        return Err(P12Error::Unsupported(format!("the cipher {}", alg.oid)));
    };
    // Without a MAC, a wrong password shows only as bad padding.
    plain.map_err(|_| P12Error::Password)
}
