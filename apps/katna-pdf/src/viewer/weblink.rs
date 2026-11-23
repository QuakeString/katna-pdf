// SPDX-License-Identifier: GPL-3.0-or-later

//! Links in a file to web addresses. A file can carry a link to any
//! address, and a click on it (or a button's action, or a form's
//! script) would send the browser there and tell the site the file was
//! opened; so the app asks first, naming the site, unless Settings say
//! to open such links or never to, or the person trusts that site.
//! Mail addresses only start a message and open as they are.

use gpui::{AnyElement, Context, KeyDownEvent, Window, div, prelude::*};
use katna_ui::{
    px,
    tokens::{space, text},
};

use super::Viewer;
use crate::float;
use crate::prefs::{Prefs, WebLinks};
use crate::text::t;

/// The site (host name, lower case, without a port or a login) of a
/// web address; `None` for anything that is not one.
pub(crate) fn site_of(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let lower = uri.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme))?;
    let authority = rest.split(['/', '\\', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        host.split(':').next()?
    };
    let host = host.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_owned())
}

/// Whether `site` is one of `trusted`, or a part of one (a trusted
/// `example.org` covers `www.example.org`).
pub(crate) fn trusted(site: &str, trusted: &[String]) -> bool {
    trusted.iter().any(|t| {
        site == t
            || site
                .strip_suffix(t.as_str())
                .is_some_and(|head| head.ends_with('.'))
    })
}

impl Viewer {
    /// A link to `uri` was followed: opens it, asks first or refuses, as
    /// Settings say.
    pub(super) fn open_address(&mut self, uri: &str, cx: &mut Context<Self>) {
        let uri = uri.trim();
        if uri.to_ascii_lowercase().starts_with("mailto:") {
            cx.open_url(uri);
            return;
        }
        let Some(site) = site_of(uri) else {
            return;
        };
        let prefs = cx.global::<Prefs>();
        match prefs.web_links {
            WebLinks::Open => cx.open_url(uri),
            WebLinks::Ask if trusted(&site, &prefs.web_sites) => cx.open_url(uri),
            WebLinks::Ask => {
                self.asking_link = Some(uri.to_owned());
                self.link_focused = false;
                cx.notify();
            }
            WebLinks::Never => {
                self.notice = Some(super::said("weblink-blocked", &[("site", &site)]));
                cx.notify();
            }
        }
    }

    /// Answers the card: opens the address, and with `always` trusts
    /// its site from now on.
    fn allow_link(&mut self, always: bool, cx: &mut Context<Self>) {
        let Some(uri) = self.asking_link.take() else {
            return;
        };
        if always && let Some(site) = site_of(&uri) {
            Prefs::set(cx, |p| {
                if !p.web_sites.contains(&site) {
                    p.web_sites.push(site);
                }
            });
        }
        cx.open_url(&uri);
        cx.notify();
    }

    pub(super) fn close_link_ask(&mut self, cx: &mut Context<Self>) {
        self.asking_link = None;
        cx.notify();
    }

    /// Gives the keys back to the pages once the card is answered.
    fn link_keys_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    /// The card asking whether to open a web address the file links to.
    pub(super) fn link_ask_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let uri = self.asking_link.clone()?;
        let site = site_of(&uri)?;
        // The card takes the keys from whatever had them (a form field
        // under the link, say), once.
        if !self.link_focused {
            self.link_focused = true;
            window.focus(&self.link_focus, cx);
        }
        let uri = &uri;
        let th = self.theme;
        let body = div()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .child(
                div()
                    .text_size(px(text::BODY))
                    .text_color(th.text)
                    .child(t("weblink-ask", &[("site", &site)])),
            )
            .child(
                div()
                    .px(px(space::S2))
                    .py(px(space::S1))
                    .rounded(px(6.0))
                    .bg(th.hover)
                    .text_size(px(text::SMALL))
                    .text_color(th.text2)
                    .overflow_hidden()
                    .child(uri.chars().take(300).collect::<String>()),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(t("weblink-hint", &[])),
            );
        let foot = div()
            .flex()
            .gap(px(space::S2))
            .child(
                float::button("weblink-cancel", t("weblink-cancel", &[]), false, &th).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.close_link_ask(cx);
                        this.link_keys_back(window, cx);
                    }),
                ),
            )
            .child(
                float::button(
                    "weblink-always",
                    t("weblink-always", &[("site", &site)]),
                    false,
                    &th,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.allow_link(true, cx);
                    this.link_keys_back(window, cx);
                })),
            )
            .child(
                float::button("weblink-open", t("weblink-open", &[]), true, &th).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.allow_link(false, cx);
                        this.link_keys_back(window, cx);
                    }),
                ),
            );
        Some(
            float::dialog_card(
                "weblink-card",
                t("weblink-title", &[]),
                None,
                440.0,
                body,
                foot,
                &th,
            )
            .track_focus(&self.link_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_link_ask(cx),
                    "enter" => this.allow_link(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
                this.link_keys_back(window, cx);
            }))
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_of_addresses() {
        assert_eq!(
            site_of("https://Example.org/a?b#c").as_deref(),
            Some("example.org")
        );
        assert_eq!(
            site_of("http://me:pw@evil.example.com:8080/").as_deref(),
            Some("evil.example.com")
        );
        assert_eq!(site_of("http://[::1]:80/x").as_deref(), Some("::1"));
        assert_eq!(
            site_of("https://example.org.").as_deref(),
            Some("example.org")
        );
        assert_eq!(site_of("javascript:alert(1)"), None);
        assert_eq!(site_of("file:///etc/passwd"), None);
        assert_eq!(site_of("https:///"), None);
    }

    #[test]
    fn trusted_sites_cover_their_parts() {
        let sites = vec!["example.org".to_owned()];
        assert!(trusted("example.org", &sites));
        assert!(trusted("www.example.org", &sites));
        assert!(!trusted("badexample.org", &sites));
        assert!(!trusted("example.org.evil.com", &sites));
    }
}
