// SPDX-License-Identifier: GPL-3.0-or-later

//! Packs the files Setup installs and, for Windows, Setup's icon and
//! version resource.
//!
//! `$KATNA_PDF_SETUP_PAYLOAD` names the folder with the files to install
//! (`.github/workflows/windows-package.yml` fills it); without it Setup
//! carries nothing and says so when it starts.

use std::path::Path;

// Only the writer is used here.
#[allow(dead_code)]
#[path = "src/payload.rs"]
mod payload;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    pack(Path::new(&out_dir));
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        resources();
    }
    // Setup is usually run from Downloads, where a page can leave a DLL
    // with a Windows DLL's name. Windows then looks for Setup's DLLs only
    // in System32 (LOAD_LIBRARY_SEARCH_SYSTEM32), not beside Setup.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}

fn pack(out: &Path) {
    println!("cargo:rerun-if-env-changed=KATNA_PDF_SETUP_PAYLOAD");
    let dest = out.join("payload.bin");
    let Some(dir) = std::env::var_os("KATNA_PDF_SETUP_PAYLOAD") else {
        std::fs::write(dest, []).unwrap();
        return;
    };
    let dir = Path::new(&dir);
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut file = std::io::BufWriter::new(std::fs::File::create(&dest).unwrap());
    payload::write(dir, &mut file).expect("packing the payload");
}

/// The icon Explorer shows for Setup, and its description.
fn resources() {
    let icon = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/katna-pdf.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.to_string_lossy())
        .set("FileDescription", "Katna PDF Setup")
        .set("ProductName", "Katna PDF");
    res.compile().expect("compiling the Windows resources");
}
