// SPDX-License-Identifier: GPL-3.0-or-later

//! On Windows, an 8 MB main thread, as on Linux: running a form's scripts
//! (boa) takes more than Windows' 1 MB.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:8388608"),
            Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608"),
            _ => {}
        }
    }
}
