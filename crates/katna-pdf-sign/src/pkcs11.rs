// SPDX-License-Identifier: GPL-3.0-or-later

//! Signing tokens: USB keys such as the ePass2003, ProxKey or mToken that
//! Indian certifying authorities issue signing certificates on, reached
//! through a PKCS#11 module, the same way Firefox and Acrobat reach
//! them. The key never leaves the token: it signs the hash we send.
//!
//! The module is OpenSC's (which drives the ePass2003 and many others)
//! or the token maker's own; [`known_modules`] lists those installed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use der::Decode;
use x509_cert::Certificate;

use crate::keys::{KeyKind, Signer, SignerError};
use crate::{Digest, Hash, oid};

/// Why a token cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The module could not be loaded or started.
    Module(String),
    /// The PIN is wrong.
    Pin,
    /// The PIN is locked after too many wrong tries.
    Locked,
    /// The token was taken out.
    Gone,
    /// The token holds no key with a certificate for it.
    NoKey,
    Failed(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::Module(e) => write!(f, "the token's driver could not be used: {e}"),
            TokenError::Pin => f.write_str("the PIN is wrong"),
            TokenError::Locked => f.write_str("the PIN is locked after too many wrong tries"),
            TokenError::Gone => f.write_str("the token was taken out"),
            TokenError::NoKey => f.write_str("the token holds no signing certificate"),
            TokenError::Failed(e) => write!(f, "the token failed: {e}"),
        }
    }
}

impl std::error::Error for TokenError {}

impl From<Error> for TokenError {
    fn from(e: Error) -> Self {
        match e {
            Error::Pkcs11(RvError::PinIncorrect | RvError::PinLenRange, _) => TokenError::Pin,
            Error::Pkcs11(RvError::PinLocked, _) => TokenError::Locked,
            Error::Pkcs11(
                RvError::TokenNotPresent | RvError::DeviceRemoved | RvError::SessionHandleInvalid,
                _,
            ) => TokenError::Gone,
            Error::LibraryLoading(e) => TokenError::Module(e.to_string()),
            e => TokenError::Failed(e.to_string()),
        }
    }
}

/// Where tokens' PKCS#11 modules are installed, most likely first:
/// those named in `KATNA_PDF_PKCS11`, OpenSC's, then makers' own for the
/// tokens Indian certifying authorities issue. Only those present are
/// returned.
pub fn known_modules() -> Vec<PathBuf> {
    #[cfg(windows)]
    let places: &[&str] = &[
        r"C:\Program Files\OpenSC Project\OpenSC\pkcs11\opensc-pkcs11.dll",
        // ePass2003 (Feitian / Longmai).
        r"C:\Windows\System32\eps2003csp11v2.dll",
        r"C:\Windows\System32\eps2003csp11.dll",
        // ProxKey (Watchdata).
        r"C:\Windows\System32\SignatureP11.dll",
        r"C:\Windows\System32\wdpkcs_icp.dll",
        // mToken (Longmai) and SafeNet eToken.
        r"C:\Windows\System32\CryptoIDA_pkcs11.dll",
        r"C:\Windows\System32\eTPKCS11.dll",
    ];
    #[cfg(not(windows))]
    let places: &[&str] = &[
        "/usr/lib/opensc-pkcs11.so",
        "/usr/lib/pkcs11/opensc-pkcs11.so",
        "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
        "/usr/lib64/opensc-pkcs11.so",
        "/usr/lib64/pkcs11/opensc-pkcs11.so",
        // ePass2003's own driver.
        "/usr/lib/libcastle.so.1.0.0",
        "/usr/local/lib/libcastle.so.1.0.0",
        "/usr/lib/libcastle_v2.so.1.0.0",
        // ProxKey (Watchdata).
        "/usr/lib/WatchData/ProxKey/lib/libwdpkcs_SignatureP11.so",
        "/usr/lib/libwdpkcs_SignatureP11.so",
        // mToken (Longmai) and SafeNet eToken.
        "/opt/CryptoIDA/lib/libcryptoid_pkcs11.so",
        "/usr/lib/libeTPkcs11.so",
    ];
    // Others named in KATNA_PDF_PKCS11, separated as in PATH, first.
    let named = std::env::var_os("KATNA_PDF_PKCS11")
        .map(|v| std::env::split_paths(&v).collect::<Vec<_>>())
        .unwrap_or_default();
    named
        .into_iter()
        .chain(places.iter().map(PathBuf::from))
        .filter(|p| p.is_file())
        .collect()
}

/// A loaded PKCS#11 module.
#[derive(Clone)]
pub struct Module {
    context: Pkcs11,
    path: PathBuf,
}

/// A token plugged in, as its module describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    slot: Slot,
    pub label: String,
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    /// The PIN is typed on the token's own keypad, not here.
    pub keypad: bool,
    /// The PIN is locked after too many wrong tries.
    pub locked: bool,
}

impl Module {
    /// Loads and starts the module at `path`.
    pub fn load(path: &Path) -> Result<Self, TokenError> {
        let context = Pkcs11::new(path)?;
        match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
            Ok(()) | Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(Self {
            context,
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The tokens plugged in.
    pub fn tokens(&self) -> Result<Vec<Token>, TokenError> {
        let mut out = Vec::new();
        for slot in self.context.get_slots_with_token()? {
            let info = self.context.get_token_info(slot)?;
            // An empty slot that only offers to make a new token.
            if !info.token_initialized() {
                continue;
            }
            out.push(Token {
                slot,
                label: info.label().trim().to_owned(),
                manufacturer: info.manufacturer_id().trim().to_owned(),
                model: info.model().trim().to_owned(),
                serial: info.serial_number().trim().to_owned(),
                keypad: info.protected_authentication_path(),
                locked: info.user_pin_locked(),
            });
        }
        Ok(out)
    }

    /// Logs in to `token` with `pin`: the signing identities on it, each
    /// a key with its certificate.
    pub fn open(&self, token: &Token, pin: &str) -> Result<Vec<TokenIdentity>, TokenError> {
        let session = self.context.open_ro_session(token.slot)?;
        // A token with a keypad takes the PIN there.
        let pin = (!token.keypad).then(|| AuthPin::from(pin.to_owned()));
        match session.login(UserType::User, pin.as_ref()) {
            Ok(()) | Err(Error::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
            Err(e) => return Err(e.into()),
        }
        let certificates: Vec<(Vec<u8>, Certificate)> = session
            .find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)])?
            .into_iter()
            .filter_map(|object| {
                let attrs = session
                    .get_attributes(object, &[AttributeType::Id, AttributeType::Value])
                    .ok()?;
                let mut id = Vec::new();
                let mut value = None;
                for a in attrs {
                    match a {
                        Attribute::Id(v) => id = v,
                        Attribute::Value(v) => value = Certificate::from_der(&v).ok(),
                        _ => {}
                    }
                }
                Some((id, value?))
            })
            .collect();
        let keys = session.find_objects(&[Attribute::Class(ObjectClass::PRIVATE_KEY)])?;
        let session = Arc::new(Mutex::new(session));
        let mut out = Vec::new();
        for key in keys {
            let id = {
                let s = session
                    .lock()
                    .map_err(|_| TokenError::Failed("lock".into()))?;
                s.get_attributes(key, &[AttributeType::Id])?
                    .into_iter()
                    .find_map(|a| match a {
                        Attribute::Id(v) => Some(v),
                        _ => None,
                    })
                    .unwrap_or_default()
            };
            let Some((_, certificate)) = certificates.iter().find(|(c, _)| *c == id) else {
                continue;
            };
            let Some(kind) = key_kind(certificate) else {
                continue;
            };
            out.push(TokenIdentity {
                session: session.clone(),
                key,
                chain: chain(certificate, &certificates),
                certificate: certificate.clone(),
                kind,
            });
        }
        if out.is_empty() {
            return Err(TokenError::NoKey);
        }
        Ok(out)
    }
}

/// The certificates above `leaf` among `all`, nearest first.
fn chain(leaf: &Certificate, all: &[(Vec<u8>, Certificate)]) -> Vec<Certificate> {
    let mut out: Vec<Certificate> = Vec::new();
    let mut at = leaf;
    while at.tbs_certificate.issuer != at.tbs_certificate.subject && out.len() < 8 {
        let Some((_, up)) = all.iter().find(|(_, c)| {
            c.tbs_certificate.subject == at.tbs_certificate.issuer && !out.contains(c) && c != leaf
        }) else {
            break;
        };
        out.push(up.clone());
        at = up;
    }
    out
}

/// The kind of key `cert` is for, if it is one signed with here.
fn key_kind(cert: &Certificate) -> Option<KeyKind> {
    let spki = &cert.tbs_certificate.subject_public_key_info;
    if spki.algorithm.oid == oid::RSA {
        return Some(KeyKind::Rsa);
    }
    if spki.algorithm.oid == oid::EC_PUBLIC_KEY {
        let curve = spki
            .algorithm
            .parameters
            .as_ref()?
            .decode_as::<const_oid::ObjectIdentifier>()
            .ok()?;
        return match curve {
            c if c == oid::P256 => Some(KeyKind::P256),
            c if c == oid::P384 => Some(KeyKind::P384),
            _ => None,
        };
    }
    None
}

/// A key on a token with its certificate, logged in to.
pub struct TokenIdentity {
    session: Arc<Mutex<Session>>,
    key: ObjectHandle,
    certificate: Certificate,
    chain: Vec<Certificate>,
    kind: KeyKind,
}

impl TokenIdentity {
    /// Who the certificate is for.
    pub fn name(&self) -> String {
        crate::common_name(&self.certificate.tbs_certificate.subject)
    }
}

impl Signer for TokenIdentity {
    fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    fn chain(&self) -> &[Certificate] {
        &self.chain
    }

    fn kind(&self) -> KeyKind {
        self.kind
    }

    fn sign(&self, digest: Digest, message: &[u8]) -> Result<Vec<u8>, SignerError> {
        let failed = |e: Error| SignerError::Failed(TokenError::from(e).to_string());
        let session = self
            .session
            .lock()
            .map_err(|_| SignerError::Failed("the token is in use".into()))?;
        let hash = Hash::from(digest);
        let hashed = hash.hash(&[message]);
        match self.kind {
            // The hash in a DigestInfo, padded and signed by the token:
            // the mechanism every RSA token has.
            KeyKind::Rsa => {
                let mut info = digest_info_prefix(digest).to_vec();
                info.extend(&hashed);
                session
                    .sign(&Mechanism::RsaPkcs, self.key, &info)
                    .map_err(failed)
            }
            // The token gives r and s side by side; CMS wants DER.
            KeyKind::P256 => {
                let raw = session
                    .sign(&Mechanism::Ecdsa, self.key, &hashed)
                    .map_err(failed)?;
                let sig = p256::ecdsa::Signature::from_slice(&raw)
                    .map_err(|e| SignerError::Failed(e.to_string()))?;
                Ok(sig.to_der().as_bytes().to_vec())
            }
            KeyKind::P384 => {
                let raw = session
                    .sign(&Mechanism::Ecdsa, self.key, &hashed)
                    .map_err(failed)?;
                let sig = p384::ecdsa::Signature::from_slice(&raw)
                    .map_err(|e| SignerError::Failed(e.to_string()))?;
                Ok(sig.to_der().as_bytes().to_vec())
            }
        }
    }
}

/// The DER of DigestInfo up to the hash itself (RFC 8017 §9.2, note 1).
fn digest_info_prefix(digest: Digest) -> &'static [u8] {
    match digest {
        Digest::Sha256 => &[
            0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x01, 0x05, 0x00, 0x04, 0x20,
        ],
        Digest::Sha384 => &[
            0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x02, 0x05, 0x00, 0x04, 0x30,
        ],
        Digest::Sha512 => &[
            0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x03, 0x05, 0x00, 0x04, 0x40,
        ],
    }
}
