// SPDX-License-Identifier: GPL-3.0-or-later

//! The object identifiers signing and checking use.

use const_oid::ObjectIdentifier as Oid;

// Content types (RFC 5652).
pub const DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.1");
pub const SIGNED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.2");
pub const ENCRYPTED_DATA: Oid = Oid::new_unwrap("1.2.840.113549.1.7.6");
pub const PBES2: Oid = Oid::new_unwrap("1.2.840.113549.1.5.13");

// Digests.
pub const SHA1: Oid = Oid::new_unwrap("1.3.14.3.2.26");
pub const SHA256: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.1");
pub const SHA384: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.2");
pub const SHA512: Oid = Oid::new_unwrap("2.16.840.1.101.3.4.2.3");

// Keys and signatures.
pub const RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.1");
pub const SHA1_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.5");
pub const RSA_PSS: Oid = Oid::new_unwrap("1.2.840.113549.1.1.10");
pub const SHA256_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.11");
pub const SHA384_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.12");
pub const SHA512_RSA: Oid = Oid::new_unwrap("1.2.840.113549.1.1.13");
pub const EC_PUBLIC_KEY: Oid = Oid::new_unwrap("1.2.840.10045.2.1");
pub const P256: Oid = Oid::new_unwrap("1.2.840.10045.3.1.7");
pub const P384: Oid = Oid::new_unwrap("1.3.132.0.34");
pub const ECDSA_SHA256: Oid = Oid::new_unwrap("1.2.840.10045.4.3.2");
pub const ECDSA_SHA384: Oid = Oid::new_unwrap("1.2.840.10045.4.3.3");
pub const ECDSA_SHA512: Oid = Oid::new_unwrap("1.2.840.10045.4.3.4");

// Signed attributes (RFC 5652, RFC 5035).
pub const CONTENT_TYPE: Oid = Oid::new_unwrap("1.2.840.113549.1.9.3");
pub const MESSAGE_DIGEST: Oid = Oid::new_unwrap("1.2.840.113549.1.9.4");
pub const SIGNING_TIME: Oid = Oid::new_unwrap("1.2.840.113549.1.9.5");
pub const SIGNING_CERTIFICATE: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.2.12");
pub const SIGNING_CERTIFICATE_V2: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.2.47");
pub const TIMESTAMP_TOKEN: Oid = Oid::new_unwrap("1.2.840.113549.1.9.16.2.14");

// Names.
pub const COMMON_NAME: Oid = Oid::new_unwrap("2.5.4.3");
pub const ORGANIZATION: Oid = Oid::new_unwrap("2.5.4.10");
