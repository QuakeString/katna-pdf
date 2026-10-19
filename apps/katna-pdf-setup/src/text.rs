// SPDX-License-Identifier: GPL-3.0-or-later

//! Setup's text, the `setup-` messages of `i18n/en/katna-pdf.ftl`, read
//! the way the app's `text.rs` reads them.

use std::collections::HashMap;
use std::sync::OnceLock;

const ENGLISH: &str = include_str!("../../../i18n/en/katna-pdf.ftl");

fn messages() -> &'static HashMap<&'static str, &'static str> {
    static MESSAGES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MESSAGES.get_or_init(|| {
        ENGLISH
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(|line| line.split_once(" = "))
            .map(|(id, text)| (id.trim(), text))
            .collect()
    })
}

/// The text of message `id`, with each `{ $name }` replaced by its value
/// from `args`; the id itself when there is no such message.
pub fn t(id: &str, args: &[(&str, &str)]) -> String {
    let Some(text) = messages().get(id) else {
        return id.to_owned();
    };
    let mut out = (*text).to_owned();
    for (name, value) in args {
        out = out.replace(&format!("{{ ${name} }}"), value);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Every `setup-` id Setup's code names has its English.
    #[test]
    fn every_message_setup_uses_is_there() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut missing = Vec::new();
        for file in std::fs::read_dir(src).unwrap().flatten() {
            let code = std::fs::read_to_string(file.path()).unwrap();
            for part in code.split("t(\"setup-").skip(1) {
                let id = format!("setup-{}", part.split('"').next().unwrap());
                if !messages().contains_key(id.as_str()) {
                    missing.push(id);
                }
            }
        }
        assert!(missing.is_empty(), "no English for {missing:?}");
    }
}
