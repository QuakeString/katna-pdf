// SPDX-License-Identifier: GPL-3.0-or-later

//! The files Setup carries: Katna PDF, katna-pdf-cli, the icon and the
//! README, each compressed with Deflate, one after the other. `build.rs`
//! writes it (with [`write`]) from the folder that
//! `$KATNA_PDF_SETUP_PAYLOAD` names; Setup reads it back (with
//! [`Payload`]). The layout is Katna Setup's, with Deflate (pure Rust)
//! for zstd.
//!
//! The format: `KATNA-PDF-PAYLOAD-1\n`, then for each file its path
//! (UTF-8, `/` between folders) as a `u16` length and the bytes, its size
//! as a `u64`, and the compressed bytes as a `u64` length and the bytes;
//! all numbers little-endian.

use std::io::{self, Read, Write};
use std::path::Path;

const MAGIC: &[u8] = b"KATNA-PDF-PAYLOAD-1\n";

/// One file, still compressed.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    /// Relative, with `/` between folders.
    pub path: &'a str,
    /// Size once unpacked.
    pub size: u64,
    packed: &'a [u8],
}

impl Entry<'_> {
    /// Unpacks the file into `out`.
    pub fn unpack(&self, out: &mut impl Write) -> io::Result<u64> {
        let mut decoder = flate2::read::DeflateDecoder::new(self.packed);
        io::copy(&mut decoder, out)
    }
}

/// The files of a payload.
#[derive(Debug, Clone, Default)]
pub struct Payload<'a> {
    pub entries: Vec<Entry<'a>>,
}

impl<'a> Payload<'a> {
    /// Reads the payload in `bytes`. An empty one (Setup built without
    /// `$KATNA_PDF_SETUP_PAYLOAD`) has no files.
    pub fn read(bytes: &'a [u8]) -> io::Result<Self> {
        if bytes.is_empty() {
            return Ok(Self::default());
        }
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "damaged payload");
        let mut rest = bytes.strip_prefix(MAGIC).ok_or_else(bad)?;
        let mut take = |n: usize| -> io::Result<&'a [u8]> {
            if rest.len() < n {
                return Err(bad());
            }
            let (head, tail) = rest.split_at(n);
            rest = tail;
            Ok(head)
        };
        let mut entries = Vec::new();
        while let Ok(len) = take(2) {
            let len = u16::from_le_bytes([len[0], len[1]]) as usize;
            let path = std::str::from_utf8(take(len)?).map_err(|_| bad())?;
            let size = u64::from_le_bytes(take(8)?.try_into().map_err(|_| bad())?);
            let packed_len = u64::from_le_bytes(take(8)?.try_into().map_err(|_| bad())?);
            let packed = take(usize::try_from(packed_len).map_err(|_| bad())?)?;
            if path.is_empty() || path.starts_with('/') || path.split('/').any(|p| p == "..") {
                return Err(bad());
            }
            entries.push(Entry { path, size, packed });
        }
        Ok(Self { entries })
    }

    /// The size of every file once unpacked.
    pub fn size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }
}

/// Writes the files under `dir`, in path order, as a payload to `out`.
#[cfg_attr(not(test), allow(dead_code))]
pub fn write(dir: &Path, out: &mut impl Write) -> io::Result<()> {
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    files.sort();
    out.write_all(MAGIC)?;
    for path in files {
        let name = path
            .strip_prefix(dir)
            .map_err(io::Error::other)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let mut raw = Vec::new();
        std::fs::File::open(&path)?.read_to_end(&mut raw)?;
        // Setup is downloaded once and unpacked once: pack it hard.
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&raw)?;
        let packed = encoder.finish()?;
        let len = u16::try_from(name.len()).map_err(io::Error::other)?;
        out.write_all(&len.to_le_bytes())?;
        out.write_all(name.as_bytes())?;
        out.write_all(&(raw.len() as u64).to_le_bytes())?;
        out.write_all(&(packed.len() as u64).to_le_bytes())?;
        out.write_all(&packed)?;
    }
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
fn collect(dir: &Path, files: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_come_back_as_they_went_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("katna-pdf.exe"), b"MZ pdf".repeat(1000)).unwrap();
        std::fs::create_dir(dir.path().join("share")).unwrap();
        std::fs::write(dir.path().join("share/katna-pdf.ico"), b"icon").unwrap();
        let mut bytes = Vec::new();
        write(dir.path(), &mut bytes).unwrap();
        let payload = Payload::read(&bytes).unwrap();
        let paths: Vec<_> = payload.entries.iter().map(|e| e.path).collect();
        assert_eq!(paths, ["katna-pdf.exe", "share/katna-pdf.ico"]);
        assert_eq!(payload.size(), 6000 + 4);
        let mut out = Vec::new();
        payload.entries[1].unpack(&mut out).unwrap();
        assert_eq!(out, b"icon");
    }

    #[test]
    fn empty_and_damaged_payloads() {
        assert!(Payload::read(&[]).unwrap().entries.is_empty());
        assert!(Payload::read(b"something else").is_err());
        let mut bad = MAGIC.to_vec();
        bad.extend_from_slice(&4u16.to_le_bytes());
        bad.extend_from_slice(b"../x");
        bad.extend_from_slice(&[0; 16]);
        assert!(Payload::read(&bad).is_err());
    }
}
