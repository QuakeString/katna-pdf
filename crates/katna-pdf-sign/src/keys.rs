// SPDX-License-Identifier: GPL-3.0-or-later

//! Keys that sign: a [`Signer`] holds a certificate and signs with its
//! key. [`SoftKey`] is a key from a certificate file; a token's key
//! never leaves the token and signs there, behind the same trait.

use der::Decode as _;
use pkcs8::PrivateKeyInfo;
use rand_core::OsRng;
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use x509_cert::Certificate;

use crate::p12::{Identity, P12Error};
use crate::{Digest, oid};

/// The kind of key, which sets how its signatures are named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Rsa,
    P256,
    P384,
}

impl KeyKind {
    /// The signature algorithm for signing with `digest`.
    pub fn algorithm(self, digest: Digest) -> const_oid::ObjectIdentifier {
        match (self, digest) {
            (KeyKind::Rsa, Digest::Sha256) => oid::SHA256_RSA,
            (KeyKind::Rsa, Digest::Sha384) => oid::SHA384_RSA,
            (KeyKind::Rsa, Digest::Sha512) => oid::SHA512_RSA,
            (_, Digest::Sha256) => oid::ECDSA_SHA256,
            (_, Digest::Sha384) => oid::ECDSA_SHA384,
            (_, Digest::Sha512) => oid::ECDSA_SHA512,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignerError {
    /// The key could not sign: a token's error, or a key the digest does
    /// not suit.
    Failed(String),
}

impl std::fmt::Display for SignerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerError::Failed(why) => write!(f, "the key could not sign: {why}"),
        }
    }
}

impl std::error::Error for SignerError {}

/// Something that signs: a certificate and its key.
pub trait Signer {
    /// The signer's certificate.
    fn certificate(&self) -> &Certificate;
    /// Certificates to send with the signature: the chain above the
    /// signer's, as far as it is known.
    fn chain(&self) -> &[Certificate];
    fn kind(&self) -> KeyKind;
    /// The signature over `message`, hashed with `digest`
    /// ([`KeyKind::digest`] of the digest asked for): PKCS#1 v1.5 for
    /// RSA, DER for ECDSA.
    fn sign(&self, digest: Digest, message: &[u8]) -> Result<Vec<u8>, SignerError>;
}

/// A private key held in memory, from a certificate file.
pub enum SoftKey {
    Rsa(Box<rsa::RsaPrivateKey>),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

impl SoftKey {
    /// A key from its PKCS#8 encoding.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, P12Error> {
        use pkcs8::DecodePrivateKey as _;
        let info = PrivateKeyInfo::from_der(der).map_err(|_| P12Error::Invalid)?;
        let alg = info.algorithm;
        if alg.oid == oid::RSA {
            let key = rsa::RsaPrivateKey::from_pkcs8_der(der).map_err(|_| P12Error::Invalid)?;
            return Ok(SoftKey::Rsa(Box::new(key)));
        }
        if alg.oid == oid::EC_PUBLIC_KEY {
            let curve = alg.parameters_oid().map_err(|_| P12Error::Invalid)?;
            if curve == oid::P256 {
                return p256::ecdsa::SigningKey::from_pkcs8_der(der)
                    .map(SoftKey::P256)
                    .map_err(|_| P12Error::Invalid);
            }
            if curve == oid::P384 {
                return p384::ecdsa::SigningKey::from_pkcs8_der(der)
                    .map(SoftKey::P384)
                    .map_err(|_| P12Error::Invalid);
            }
            return Err(P12Error::Unsupported(format!("the curve {curve}")));
        }
        Err(P12Error::Unsupported(format!("the key type {}", alg.oid)))
    }

    pub fn kind(&self) -> KeyKind {
        match self {
            SoftKey::Rsa(_) => KeyKind::Rsa,
            SoftKey::P256(_) => KeyKind::P256,
            SoftKey::P384(_) => KeyKind::P384,
        }
    }

    /// The public key's bits, as a certificate holds them: PKCS#1 for
    /// RSA, the uncompressed point for a curve.
    pub fn public_key_bits(&self) -> Vec<u8> {
        use rsa::pkcs1::EncodeRsaPublicKey as _;
        match self {
            SoftKey::Rsa(k) => k
                .to_public_key()
                .to_pkcs1_der()
                .map(|d| d.into_vec())
                .unwrap_or_default(),
            SoftKey::P256(k) => k
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
            SoftKey::P384(k) => k
                .verifying_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        }
    }

    pub fn sign(&self, digest: Digest, message: &[u8]) -> Result<Vec<u8>, SignerError> {
        let failed = |e: rsa::signature::Error| SignerError::Failed(e.to_string());
        Ok(match (self, digest) {
            (SoftKey::Rsa(k), Digest::Sha256) => {
                rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new((**k).clone())
                    .try_sign_with_rng(&mut OsRng, message)
                    .map_err(failed)?
                    .to_vec()
            }
            (SoftKey::Rsa(k), Digest::Sha384) => {
                rsa::pkcs1v15::SigningKey::<sha2::Sha384>::new((**k).clone())
                    .try_sign_with_rng(&mut OsRng, message)
                    .map_err(failed)?
                    .to_vec()
            }
            (SoftKey::Rsa(k), Digest::Sha512) => {
                rsa::pkcs1v15::SigningKey::<sha2::Sha512>::new((**k).clone())
                    .try_sign_with_rng(&mut OsRng, message)
                    .map_err(failed)?
                    .to_vec()
            }
            // ECDSA signs the digest asked for, cut to the curve's size
            // if longer (FIPS 186-4 §6.4), as CMS readers expect.
            (SoftKey::P256(k), _) => {
                let hashed = crate::Hash::from(digest).hash(&[message]);
                let sig: p256::ecdsa::Signature =
                    rsa::signature::hazmat::PrehashSigner::sign_prehash(k, &hashed)
                        .map_err(failed)?;
                sig.to_der().as_bytes().to_vec()
            }
            (SoftKey::P384(k), _) => {
                let hashed = crate::Hash::from(digest).hash(&[message]);
                let sig: p384::ecdsa::Signature =
                    rsa::signature::hazmat::PrehashSigner::sign_prehash(k, &hashed)
                        .map_err(failed)?;
                sig.to_der().as_bytes().to_vec()
            }
        })
    }
}

impl Signer for Identity {
    fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    fn chain(&self) -> &[Certificate] {
        &self.chain
    }

    fn kind(&self) -> KeyKind {
        self.key.kind()
    }

    fn sign(&self, digest: Digest, message: &[u8]) -> Result<Vec<u8>, SignerError> {
        self.key.sign(digest, message)
    }
}
