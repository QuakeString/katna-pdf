// SPDX-License-Identifier: GPL-3.0-or-later

//! PDF's standard security handler (ISO 32000-2 §7.6.4): opening files
//! locked with a password, RC4 (40 and 128 bit), AES-128 and AES-256
//! (revisions 2 to 6), and locking files with AES-256 (revision 6, PDF
//! 2.0's), which is all a new file should use.
//!
//! A file's strings and streams are each encrypted with a key made from
//! the file's key; `Crypt` turns objects read from the file back into
//! plain ones and plain ones written into an update into encrypted ones.

use aes::cipher::block_padding::{NoPadding, Pkcs7};
use aes::cipher::{BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit};
use md5::{Digest as _, Md5};
use sha2::{Sha256, Sha384, Sha512};

use crate::object::{Dict, Name, Object, Ref, Stream};

/// What padding a short password out to 32 bytes uses (Algorithm 2).
const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// How strings or streams are encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    None,
    Rc4,
    Aes128,
    Aes256,
}

/// Why a file could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockError {
    /// The password is not the file's.
    WrongPassword,
    /// A security handler other than the standard one, or a revision this
    /// does not know.
    Unsupported,
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UnlockError::WrongPassword => "the password is not right",
            UnlockError::Unsupported => "the file is locked in a way this cannot open",
        })
    }
}

impl std::error::Error for UnlockError {}

/// What someone who opens a locked file without its owner password may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub print: bool,
    /// Print at full quality, not only as a rough picture.
    pub print_well: bool,
    /// Change the pages' content.
    pub change: bool,
    pub copy: bool,
    /// Add and change comments, and fill in forms.
    pub comment: bool,
    pub fill_forms: bool,
    /// Insert, delete and turn pages.
    pub assemble: bool,
}

impl Permissions {
    pub const ALL: Permissions = Permissions {
        print: true,
        print_well: true,
        change: true,
        copy: true,
        comment: true,
        fill_forms: true,
        assemble: true,
    };

    /// The permissions in a `/P` value.
    pub fn from_bits(p: i32) -> Self {
        let bit = |n: u32| p & (1 << (n - 1)) != 0;
        Permissions {
            print: bit(3),
            print_well: bit(3) && bit(12),
            change: bit(4),
            copy: bit(5),
            comment: bit(6),
            fill_forms: bit(6) || bit(9),
            assemble: bit(4) || bit(11),
        }
    }

    /// As a `/P` value: the bits not used set, as the standard asks.
    pub fn bits(&self) -> i32 {
        let mut p: u32 = 0xFFFF_F0C0;
        let mut set = |n: u32, on: bool| {
            if on {
                p |= 1 << (n - 1);
            }
        };
        set(3, self.print);
        set(4, self.change);
        set(5, self.copy);
        // Text for people who cannot see the page is always allowed.
        set(10, true);
        set(6, self.comment);
        set(9, self.fill_forms);
        set(11, self.assemble);
        set(12, self.print_well);
        p as i32
    }
}

/// The key to a locked file, and how it encrypts.
#[derive(Clone)]
pub(crate) struct Crypt {
    key: Vec<u8>,
    revision: i64,
    strings: Method,
    streams: Method,
    encrypt_metadata: bool,
    /// Opened with the owner password (or a file without one): its
    /// locking may be changed.
    pub(crate) owner: bool,
    pub(crate) permissions: Permissions,
    /// The `/Encrypt` dictionary's object, which is not itself encrypted.
    pub(crate) dict_ref: Option<Ref>,
}

impl std::fmt::Debug for Crypt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Crypt")
            .field("revision", &self.revision)
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

/// The bytes of a string entry.
fn bytes_of(dict: &Dict, key: &str) -> Vec<u8> {
    match dict.get(key) {
        Some(Object::String { bytes, .. }) => bytes.clone(),
        _ => Vec::new(),
    }
}

/// A password prepared as revisions 5 and 6 ask (SASLprep, RFC 4013):
/// spaces of other kinds made plain, characters that mean nothing taken
/// out, and the rest normalized (NFKC).
fn sasl_prep(password: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    password
        .chars()
        .filter(|c| {
            !matches!(
                *c,
                '\u{ad}'
                    | '\u{34f}'
                    | '\u{1806}'
                    | '\u{180b}'..='\u{180d}'
                    | '\u{200b}'..='\u{200d}'
                    | '\u{2060}'
                    | '\u{fe00}'..='\u{fe0f}'
                    | '\u{feff}'
            )
        })
        .map(|c| {
            if c != ' ' && c.is_whitespace() {
                ' '
            } else {
                c
            }
        })
        .nfkc()
        .collect()
}

/// A password as the handler takes it: UTF-8 for revisions 5 and 6, at
/// most 127 bytes; Latin-1 for older ones.
fn password_bytes(password: &str, revision: i64) -> Vec<u8> {
    if revision >= 5 {
        let mut b = sasl_prep(password).into_bytes();
        b.truncate(127);
        b
    } else {
        password
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
            .collect()
    }
}

fn padded(password: &[u8]) -> [u8; 32] {
    let mut out = PAD;
    let n = password.len().min(32);
    out[..n].copy_from_slice(&password[..n]);
    out[n..].copy_from_slice(&PAD[..32 - n]);
    out
}

/// RC4, both ways.
fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len().max(1)]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|b| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            b ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

/// AES-CBC decrypted, the IV first, padding taken off; what cannot be
/// decrypted comes back as it was.
fn aes_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 || !data.len().is_multiple_of(16) {
        // Only an IV (an empty string), or broken.
        return Vec::new();
    }
    let (iv, body) = data.split_at(16);
    let out = match key.len() {
        16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
            .ok()
            .and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(body).ok()),
        32 => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
            .ok()
            .and_then(|d| d.decrypt_padded_vec_mut::<Pkcs7>(body).ok()),
        _ => None,
    };
    // Some writers leave the padding off; keep the bytes then.
    out.unwrap_or_else(|| {
        let raw = match key.len() {
            16 => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv)
                .ok()
                .and_then(|d| d.decrypt_padded_vec_mut::<NoPadding>(body).ok()),
            _ => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv)
                .ok()
                .and_then(|d| d.decrypt_padded_vec_mut::<NoPadding>(body).ok()),
        };
        raw.unwrap_or_default()
    })
}

/// AES-CBC encrypted with a new random IV, which goes first.
fn aes_encrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    random(&mut iv);
    let body = match key.len() {
        16 => cbc::Encryptor::<aes::Aes128>::new_from_slices(key, &iv)
            .map(|e| e.encrypt_padded_vec_mut::<Pkcs7>(data))
            .unwrap_or_default(),
        _ => cbc::Encryptor::<aes::Aes256>::new_from_slices(key, &iv)
            .map(|e| e.encrypt_padded_vec_mut::<Pkcs7>(data))
            .unwrap_or_default(),
    };
    let mut out = iv.to_vec();
    out.extend(body);
    out
}

/// AES-256-CBC with a zero IV and no padding, as `/UE` and `/OE` use.
fn aes256_no_iv(key: &[u8], data: &[u8], encrypt: bool) -> Vec<u8> {
    let iv = [0u8; 16];
    if encrypt {
        cbc::Encryptor::<aes::Aes256>::new_from_slices(key, &iv)
            .map(|e| e.encrypt_padded_vec_mut::<NoPadding>(data))
            .unwrap_or_default()
    } else {
        cbc::Decryptor::<aes::Aes256>::new_from_slices(key, &iv)
            .ok()
            .and_then(|d| d.decrypt_padded_vec_mut::<NoPadding>(data).ok())
            .unwrap_or_default()
    }
}

pub(crate) fn random(out: &mut [u8]) {
    use rand_core::RngCore;
    rand_core::OsRng.fill_bytes(out);
}

/// Revision 6's hash (Algorithm 2.B); revision 5 stops after the first
/// SHA-256.
fn hash_r6(password: &[u8], salt: &[u8], udata: &[u8], revision: i64) -> [u8; 32] {
    let mut k: Vec<u8> = Sha256::new()
        .chain_update(password)
        .chain_update(salt)
        .chain_update(udata)
        .finalize()
        .to_vec();
    if revision < 6 {
        let mut out = [0u8; 32];
        out.copy_from_slice(&k[..32]);
        return out;
    }
    let mut round = 0u32;
    loop {
        let mut k1 = Vec::with_capacity(64 * (password.len() + k.len() + udata.len()));
        for _ in 0..64 {
            k1.extend_from_slice(password);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(udata);
        }
        let e = cbc::Encryptor::<aes::Aes128>::new_from_slices(&k[..16], &k[16..32])
            .map(|c| c.encrypt_padded_vec_mut::<NoPadding>(&k1))
            .unwrap_or_default();
        let sum: u32 = e[..16].iter().map(|b| u32::from(*b)).sum();
        k = match sum % 3 {
            0 => Sha256::digest(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        round += 1;
        if round >= 64 && u32::from(*e.last().unwrap_or(&0)) <= round - 32 {
            break;
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&k[..32]);
    out
}

impl Crypt {
    /// The key to the file locked by `encrypt`, opened with `password`
    /// (as its owner's, or else as its user's).
    pub(crate) fn unlock(
        encrypt: &Dict,
        dict_ref: Option<Ref>,
        id: &[u8],
        password: &str,
    ) -> Result<Crypt, UnlockError> {
        if encrypt.name("Filter") != Some(b"Standard") {
            return Err(UnlockError::Unsupported);
        }
        let v = encrypt.int("V").unwrap_or(0);
        let revision = encrypt.int("R").unwrap_or(0);
        let p = encrypt.int("P").unwrap_or(-1) as i32;
        let encrypt_metadata = !matches!(encrypt.get("EncryptMetadata"), Some(Object::Bool(false)));
        let (strings, streams) = match v {
            1 | 2 => (Method::Rc4, Method::Rc4),
            4 | 5 => (
                filter_method(encrypt, "StrF"),
                filter_method(encrypt, "StmF"),
            ),
            _ => return Err(UnlockError::Unsupported),
        };
        let o = bytes_of(encrypt, "O");
        let u = bytes_of(encrypt, "U");
        let (key, owner) = match revision {
            2..=4 => {
                let length = match v {
                    1 => 5,
                    _ => (encrypt.int("Length").unwrap_or(40) / 8).clamp(5, 16) as usize,
                };
                let legacy = Legacy {
                    o: &o,
                    u: &u,
                    p,
                    id,
                    revision,
                    length,
                    encrypt_metadata,
                };
                let owner_try = legacy.user_from_owner(&password_bytes(password, revision));
                if let Some(key) = legacy.check_user(&owner_try) {
                    (key, true)
                } else if let Some(key) = legacy.check_user(&password_bytes(password, revision)) {
                    (key, false)
                } else {
                    return Err(UnlockError::WrongPassword);
                }
            }
            5 | 6 => {
                if o.len() < 48 || u.len() < 48 {
                    return Err(UnlockError::Unsupported);
                }
                let pw = password_bytes(password, revision);
                let oe = bytes_of(encrypt, "OE");
                let ue = bytes_of(encrypt, "UE");
                let (key, owner) = if hash_r6(&pw, &o[32..40], &u[..48], revision) == o[..32] {
                    let k = hash_r6(&pw, &o[40..48], &u[..48], revision);
                    (aes256_no_iv(&k, &oe, false), true)
                } else if hash_r6(&pw, &u[32..40], &[], revision) == u[..32] {
                    let k = hash_r6(&pw, &u[40..48], &[], revision);
                    (aes256_no_iv(&k, &ue, false), false)
                } else {
                    return Err(UnlockError::WrongPassword);
                };
                if key.len() != 32 {
                    return Err(UnlockError::Unsupported);
                }
                (key, owner)
            }
            _ => return Err(UnlockError::Unsupported),
        };
        // A file whose owner password is the user's (often both empty)
        // has no owner to ask.
        let owner = owner || o.is_empty();
        Ok(Crypt {
            key,
            revision,
            strings,
            streams,
            encrypt_metadata,
            owner,
            permissions: if owner {
                Permissions::ALL
            } else {
                Permissions::from_bits(p)
            },
            dict_ref,
        })
    }

    /// The key for object `r`'s strings and streams, and how they are
    /// encrypted.
    fn object_key(&self, r: Ref, method: Method) -> Vec<u8> {
        if method == Method::Aes256 {
            return self.key.clone();
        }
        let mut h = Md5::new();
        h.update(&self.key);
        h.update(&r.num.to_le_bytes()[..3]);
        h.update(r.generation.to_le_bytes());
        if method == Method::Aes128 {
            h.update(b"sAlT");
        }
        let n = (self.key.len() + 5).min(16);
        h.finalize()[..n].to_vec()
    }

    fn apply(&self, r: Ref, method: Method, data: &[u8], encrypt: bool) -> Vec<u8> {
        let key = self.object_key(r, method);
        match (method, encrypt) {
            (Method::None, _) => data.to_vec(),
            (Method::Rc4, _) => rc4(&key, data),
            (_, false) => aes_decrypt(&key, data),
            (_, true) => aes_encrypt(&key, data),
        }
    }

    /// Whether a stream with dictionary `dict` is encrypted: not the
    /// cross-reference stream, nor XMP left plain.
    fn stream_encrypted(&self, dict: &Dict) -> bool {
        match dict.name("Type") {
            Some(b"XRef") => false,
            Some(b"Metadata") => self.encrypt_metadata,
            _ => true,
        }
    }

    /// The same key, its `/Encrypt` dictionary at `r`.
    pub(crate) fn for_dict(mut self, r: Ref) -> Crypt {
        self.dict_ref = Some(r);
        self
    }

    /// Object `r` as read from the file, decrypted.
    pub(crate) fn decrypt(&self, r: Ref, obj: Object) -> Object {
        if self.dict_ref == Some(r) {
            return obj;
        }
        self.walk(r, obj, false)
    }

    /// Object `r` as written into the file, encrypted.
    pub(crate) fn encrypt(&self, r: Ref, obj: Object) -> Object {
        if self.dict_ref == Some(r) {
            return obj;
        }
        self.walk(r, obj, true)
    }

    fn walk(&self, r: Ref, obj: Object, encrypt: bool) -> Object {
        match obj {
            Object::String { bytes, hex } => Object::String {
                bytes: self.apply(r, self.strings, &bytes, encrypt),
                hex,
            },
            Object::Array(items) => Object::Array(
                items
                    .into_iter()
                    .map(|o| self.walk(r, o, encrypt))
                    .collect(),
            ),
            Object::Dict(dict) => Object::Dict(self.walk_dict(r, dict, encrypt)),
            Object::Stream(Stream { dict, data }) => {
                let data = if self.stream_encrypted(&dict) {
                    self.apply(r, self.streams, &data, encrypt)
                } else {
                    data
                };
                let mut dict = self.walk_dict(r, dict, encrypt);
                if encrypt {
                    dict.set("Length", Object::Int(data.len() as i64));
                }
                Object::Stream(Stream { dict, data })
            }
            other => other,
        }
    }

    fn walk_dict(&self, r: Ref, dict: Dict, encrypt: bool) -> Dict {
        // A signature's /Contents is never encrypted: it signs the
        // file's bytes as they are.
        let signature = matches!(dict.name("Type"), Some(b"Sig") | Some(b"DocTimeStamp"))
            || dict.get("ByteRange").is_some();
        Dict(
            dict.0
                .into_iter()
                .map(|(k, v)| {
                    if signature && k.as_bytes() == b"Contents" {
                        (k, v)
                    } else {
                        (k, self.walk(r, v, encrypt))
                    }
                })
                .collect(),
        )
    }
}

fn filter_method(encrypt: &Dict, which: &str) -> Method {
    let Some(name) = encrypt.name(which) else {
        return Method::None;
    };
    if name == b"Identity" {
        return Method::None;
    }
    let filter = match encrypt.get("CF") {
        Some(Object::Dict(cf)) => match cf.get(&String::from_utf8_lossy(name)) {
            Some(Object::Dict(f)) => f.clone(),
            _ => return Method::None,
        },
        _ => return Method::None,
    };
    match filter.name("CFM") {
        Some(b"V2") => Method::Rc4,
        Some(b"AESV2") => Method::Aes128,
        Some(b"AESV3") => Method::Aes256,
        _ => Method::None,
    }
}

/// Revisions 2 to 4's checks.
struct Legacy<'a> {
    o: &'a [u8],
    u: &'a [u8],
    p: i32,
    id: &'a [u8],
    revision: i64,
    length: usize,
    encrypt_metadata: bool,
}

impl Legacy<'_> {
    /// The file key from a user password (Algorithm 2).
    fn key(&self, password: &[u8]) -> Vec<u8> {
        let mut h = Md5::new();
        h.update(padded(password));
        h.update(&self.o[..self.o.len().min(32)]);
        h.update(self.p.to_le_bytes());
        h.update(self.id);
        if self.revision >= 4 && !self.encrypt_metadata {
            h.update([0xFF; 4]);
        }
        let mut key = h.finalize().to_vec();
        if self.revision >= 3 {
            for _ in 0..50 {
                key = Md5::digest(&key[..self.length]).to_vec();
            }
        }
        key.truncate(self.length);
        key
    }

    /// The file key, when `password` is the user's (Algorithms 4 to 6).
    fn check_user(&self, password: &[u8]) -> Option<Vec<u8>> {
        let key = self.key(password);
        if self.revision == 2 {
            return (rc4(&key, &PAD) == self.u.get(..32)?).then_some(key);
        }
        let mut h = Md5::new();
        h.update(PAD);
        h.update(self.id);
        let mut out = rc4(&key, &h.finalize());
        for i in 1..=19u8 {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            out = rc4(&k, &out);
        }
        (out[..16] == *self.u.get(..16)?).then_some(key)
    }

    /// The user password an owner password unlocks (Algorithm 7).
    fn user_from_owner(&self, owner: &[u8]) -> Vec<u8> {
        let mut key = Md5::digest(padded(owner)).to_vec();
        if self.revision >= 3 {
            for _ in 0..50 {
                key = Md5::digest(&key).to_vec();
            }
        }
        key.truncate(self.length);
        let o = &self.o[..self.o.len().min(32)];
        if self.revision == 2 {
            return rc4(&key, o);
        }
        let mut out = o.to_vec();
        for i in (0..=19u8).rev() {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            out = rc4(&k, &out);
        }
        out
    }
}

/// A new AES-256 lock: the `/Encrypt` dictionary and its key. An empty
/// `user` password opens the file for anyone, with `permissions`; the
/// `owner` password opens it fully. An empty owner password is made
/// random, so the permissions hold.
pub(crate) fn new_lock(user: &str, owner: &str, permissions: Permissions) -> (Dict, Crypt) {
    let mut key = [0u8; 32];
    random(&mut key);
    let user = password_bytes(user, 6);
    let owner = if owner.is_empty() {
        let mut r = [0u8; 24];
        random(&mut r);
        r.iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .into_bytes()
    } else {
        password_bytes(owner, 6)
    };
    let mut salts = [0u8; 32];
    random(&mut salts);
    let (u_valid, u_key, o_valid, o_key) =
        (&salts[0..8], &salts[8..16], &salts[16..24], &salts[24..32]);
    let mut u = hash_r6(&user, u_valid, &[], 6).to_vec();
    u.extend_from_slice(u_valid);
    u.extend_from_slice(u_key);
    let ue = aes256_no_iv(&hash_r6(&user, u_key, &[], 6), &key, true);
    let mut o = hash_r6(&owner, o_valid, &u, 6).to_vec();
    o.extend_from_slice(o_valid);
    o.extend_from_slice(o_key);
    let oe = aes256_no_iv(&hash_r6(&owner, o_key, &u, 6), &key, true);
    let p = permissions.bits();
    let mut perms = [0u8; 16];
    perms[..4].copy_from_slice(&p.to_le_bytes());
    perms[4..8].copy_from_slice(&[0xFF; 4]);
    perms[8] = b'T';
    perms[9..12].copy_from_slice(b"adb");
    random(&mut perms[12..]);
    let mut block = aes::Block::from(perms);
    aes::Aes256::new_from_slice(&key)
        .expect("a 32-byte key")
        .encrypt_block(&mut block);
    let string = |bytes: Vec<u8>| Object::String { bytes, hex: true };
    let std_cf = Dict(vec![
        (Name::new("AuthEvent"), Object::name("DocOpen")),
        (Name::new("CFM"), Object::name("AESV3")),
        (Name::new("Length"), Object::Int(32)),
    ]);
    let dict = Dict(vec![
        (Name::new("Filter"), Object::name("Standard")),
        (Name::new("V"), Object::Int(5)),
        (Name::new("R"), Object::Int(6)),
        (Name::new("Length"), Object::Int(256)),
        (
            Name::new("CF"),
            Object::Dict(Dict(vec![(Name::new("StdCF"), Object::Dict(std_cf))])),
        ),
        (Name::new("StmF"), Object::name("StdCF")),
        (Name::new("StrF"), Object::name("StdCF")),
        (Name::new("O"), string(o)),
        (Name::new("U"), string(u)),
        (Name::new("OE"), string(oe)),
        (Name::new("UE"), string(ue)),
        (Name::new("P"), Object::Int(i64::from(p))),
        (Name::new("Perms"), string(block.to_vec())),
        (Name::new("EncryptMetadata"), Object::Bool(true)),
    ]);
    let crypt = Crypt {
        key: key.to_vec(),
        revision: 6,
        strings: Method::Aes256,
        streams: Method::Aes256,
        encrypt_metadata: true,
        owner: true,
        permissions: Permissions::ALL,
        dict_ref: None,
    };
    (dict, crypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc4_matches_the_known_answer() {
        // From RFC 6229's first test vector (key 0x0102030405).
        let out = rc4(&[1, 2, 3, 4, 5], &[0; 8]);
        assert_eq!(out, [0xb2, 0x39, 0x63, 0x05, 0xf0, 0x3d, 0xc0, 0x27]);
    }

    #[test]
    fn permissions_round_trip() {
        let p = Permissions {
            print: true,
            print_well: false,
            change: false,
            copy: false,
            comment: true,
            fill_forms: true,
            assemble: false,
        };
        assert_eq!(Permissions::from_bits(p.bits()), p);
        assert_eq!(
            Permissions::from_bits(Permissions::ALL.bits()),
            Permissions::ALL
        );
    }

    #[test]
    fn a_new_lock_opens_with_either_password() {
        let (dict, made) = new_lock("asha", "boss", Permissions::ALL);
        let user = Crypt::unlock(&dict, None, b"", "asha").unwrap();
        assert!(!user.owner);
        assert_eq!(user.key, made.key);
        let owner = Crypt::unlock(&dict, None, b"", "boss").unwrap();
        assert!(owner.owner);
        assert_eq!(owner.key, made.key);
        assert_eq!(
            Crypt::unlock(&dict, None, b"", "nobody").unwrap_err(),
            UnlockError::WrongPassword
        );
        // Strings and streams go there and back.
        let r = Ref::new(7, 0);
        let s = Object::String {
            bytes: b"Dear Asha".to_vec(),
            hex: false,
        };
        let locked = made.encrypt(r, s.clone());
        assert_ne!(locked, s);
        assert_eq!(user.decrypt(r, locked), s);
    }
}
