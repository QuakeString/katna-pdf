// SPDX-License-Identifier: GPL-3.0-or-later

//! On Windows, Katna PDF's icon and description go into the program, where
//! Explorer, the taskbar and the window's title bar find them.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
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
