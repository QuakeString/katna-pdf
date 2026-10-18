// SPDX-License-Identifier: GPL-3.0-or-later

//! Printing (docs/PLAN.md, Phase 1, basic printing): the open file is
//! handed as it is to the desktop's print dialog (the XDG print portal,
//! as Katna Mail prints), which asks for the printer, the pages and the
//! copies, and prints it.

use std::path::Path;

/// How printing went. Windows only ever says there is no dialog.
#[derive(Debug)]
#[cfg_attr(windows, allow(dead_code))]
pub enum Printed {
    /// Sent to the printer, or the dialog was closed.
    Done,
    /// There is no print dialog on this desktop.
    NoDialog,
    Failed(String),
}

/// Asks the desktop to print the PDF at `path`, titled `title`.
#[cfg(not(windows))]
pub async fn print(title: &str, path: &Path) -> Printed {
    use std::os::fd::AsFd;

    use ashpd::desktop::ResponseError;
    use ashpd::desktop::print::{PreparePrintOptions, PrintOptions, PrintProxy};

    let Ok(proxy) = PrintProxy::new().await else {
        return Printed::NoDialog;
    };
    let prepared = match proxy
        .prepare_print(
            None,
            title,
            Default::default(),
            Default::default(),
            PreparePrintOptions::default().set_modal(false),
        )
        .await
        .and_then(|request| request.response())
    {
        Ok(prepared) => prepared,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Printed::Done,
        Err(err) => return Printed::Failed(err.to_string()),
    };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => return Printed::Failed(err.to_string()),
    };
    match proxy
        .print(
            None,
            title,
            &file.as_fd(),
            PrintOptions::default()
                .set_token(prepared.token)
                .set_modal(false),
        )
        .await
    {
        Ok(_) => Printed::Done,
        Err(err) => Printed::Failed(err.to_string()),
    }
}

/// Windows has no print dialog to hand a PDF to yet.
#[cfg(windows)]
pub async fn print(_title: &str, _path: &Path) -> Printed {
    Printed::NoDialog
}
