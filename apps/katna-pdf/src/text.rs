// SPDX-License-Identifier: GPL-3.0-or-later

//! The interface's text, from `i18n/en/katna-pdf.ftl`.
//!
//! Katna's `katna-i18n` embeds the English of Katna's own binaries only,
//! so until it can take English from another repository, Katna PDF reads
//! its own file here, with the same ids and `{ $name }` variables, so the
//! move to `tr!` is a rename.

use std::collections::HashMap;
use std::sync::OnceLock;

const ENGLISH: &str = include_str!("../../../i18n/en/katna-pdf.ftl");

fn messages() -> &'static HashMap<&'static str, &'static str> {
    static MESSAGES: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MESSAGES.get_or_init(|| parse(ENGLISH))
}

fn parse(source: &'static str) -> HashMap<&'static str, &'static str> {
    source
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .filter_map(|line| line.split_once(" = "))
        .map(|(id, text)| (id.trim(), text))
        .collect()
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
    use super::*;

    #[test]
    fn every_line_is_a_message_or_a_comment() {
        for line in ENGLISH.lines() {
            let ok = line.is_empty() || line.starts_with('#') || line.contains(" = ");
            assert!(ok, "not a message: {line}");
        }
    }

    #[test]
    fn fills_in_variables() {
        assert_eq!(t("page-of", &[("page", "3"), ("count", "24")]), "3 / 24");
    }

    #[test]
    fn falls_back_to_the_id() {
        assert_eq!(t("no-such-message", &[]), "no-such-message");
    }
}
