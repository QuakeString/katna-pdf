// SPDX-License-Identifier: GPL-3.0-or-later

//! The document security store (ISO 32000-2 §12.8.4.3, PAdES B-LT): the
//! certificates, OCSP answers and CRLs that show the file's signatures
//! were valid, kept in the file so they can be checked years later,
//! offline, after the certificate authorities' servers are gone.

use std::io::Write as _;

use crate::filter::decode;
use crate::object::{Dict, Name, Object, Stream};
use crate::{Document, UpdateError};

/// Validation data, each item DER.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dss {
    pub certs: Vec<Vec<u8>>,
    pub ocsps: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
}

impl Dss {
    pub fn is_empty(&self) -> bool {
        self.certs.is_empty() && self.ocsps.is_empty() && self.crls.is_empty()
    }

    /// Adds what `other` holds that this does not.
    pub fn merge(&mut self, other: &Dss) {
        for (mine, theirs) in [
            (&mut self.certs, &other.certs),
            (&mut self.ocsps, &other.ocsps),
            (&mut self.crls, &other.crls),
        ] {
            for item in theirs {
                if !mine.contains(item) {
                    mine.push(item.clone());
                }
            }
        }
    }
}

const KEYS: [&str; 3] = ["Certs", "OCSPs", "CRLs"];

impl Document {
    /// The validation data the file keeps.
    pub fn dss(&self) -> Dss {
        let mut out = Dss::default();
        let Some(catalog) = self.catalog() else {
            return out;
        };
        let Object::Dict(dss) = self.resolve_key(&catalog, "DSS") else {
            return out;
        };
        for (key, list) in KEYS
            .iter()
            .zip([&mut out.certs, &mut out.ocsps, &mut out.crls])
        {
            let Object::Array(items) = self.resolve_key(&dss, key) else {
                continue;
            };
            for item in items {
                if let Object::Stream(s) = self.resolve(&item)
                    && let Ok(data) = decode(&s.dict, &s.data)
                {
                    list.push(data);
                }
            }
        }
        out
    }

    /// The file with `data` added to the validation data it keeps, as an
    /// incremental update, which leaves its signatures valid.
    pub fn add_dss(&self, data: &Dss) -> Result<Vec<u8>, UpdateError> {
        let mut all = self.dss();
        all.merge(data);
        let mut update = self.update()?;
        let mut dss = Dict::new();
        for (key, list) in KEYS.iter().zip([&all.certs, &all.ocsps, &all.crls]) {
            if list.is_empty() {
                continue;
            }
            let refs: Vec<Object> = list
                .iter()
                .map(|item| Object::Ref(update.add(Object::Stream(compressed(item)))))
                .collect();
            dss.set(key, refs);
        }
        let dss = update.add(Object::Dict(dss));
        let mut catalog = self.catalog().unwrap_or_default();
        catalog.set("DSS", Object::Ref(dss));
        // Acrobat reads the store in files that say they use it.
        let mut extensions = match self.resolve_key(&catalog, "Extensions") {
            Object::Dict(d) => d,
            _ => Dict::new(),
        };
        if extensions.get("ESIC").is_none() {
            extensions.set(
                "ESIC",
                Dict(vec![
                    (Name::new("BaseVersion"), Object::name("1.7")),
                    (Name::new("ExtensionLevel"), Object::Int(5)),
                ]),
            );
            catalog.set("Extensions", extensions);
        }
        let catalog_ref = self
            .trailer()
            .get("Root")
            .and_then(Object::as_ref)
            .ok_or(UpdateError::Repaired)?;
        update.replace(catalog_ref, Object::Dict(catalog));
        Ok(update.finish())
    }
}

/// `data` in a stream of its own, compressed.
fn compressed(data: &[u8]) -> Stream {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let packed = e.write_all(data).and_then(|_| e.finish());
    match packed {
        Ok(packed) => Stream {
            dict: Dict(vec![(Name::new("Filter"), Object::name("FlateDecode"))]),
            data: packed,
        },
        Err(_) => Stream {
            dict: Dict::new(),
            data: data.to_vec(),
        },
    }
}
