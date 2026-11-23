// SPDX-License-Identifier: GPL-3.0-or-later

//! On Windows, Katna PDF's icon and description go into the program, where
//! Explorer, the taskbar and the window's title bar find them, and its
//! main thread gets the stack a form's scripts need (Windows gives 1 MB).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        stack();
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/windows/katna-pdf.ico");
        println!("cargo:rerun-if-changed={}", icon.display());
        let mut res = winresource::WindowsResource::new();
        res.set_icon(&icon.to_string_lossy())
            .set("FileDescription", "Katna PDF")
            .set("ProductName", "Katna PDF");
        res.compile().expect("compiling the Windows resources");
    }
}

/// An 8 MB main thread on Windows, as on Linux: running a form's scripts
/// (boa) takes more than Windows' 1 MB.
fn stack() {
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:8388608"),
        Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608"),
        _ => {}
    }
}
