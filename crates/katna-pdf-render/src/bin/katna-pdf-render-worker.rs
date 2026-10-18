// SPDX-License-Identifier: GPL-3.0-or-later

//! A render worker on its own, for tests and benchmarks: the app starts
//! itself as one instead (`katna-pdf --render-worker`). `--exit` ends it
//! at once, as a crash would.

fn main() -> std::io::Result<()> {
    if std::env::args().any(|a| a == "--exit") {
        std::process::exit(3);
    }
    katna_pdf_render::worker::serve()
}
