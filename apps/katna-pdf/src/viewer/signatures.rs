// SPDX-License-Identifier: GPL-3.0-or-later

//! The signatures a file holds, as Float shows them: a line over the
//! pages coloured by how they hold (G08–G10), the side pane's summary
//! and a card for each signer (G07), and the card one of them opens
//! with what was checked, its certificate and what can be done (G07–G09).

use gpui::{AnyElement, Context, Hsla, div, prelude::*};
use katna_pdf_sign::{Revocation, common_name};
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};

use super::sign::{SigRow, Vouched, allowed_id, state};
use super::{PaneTab, Viewer};
use crate::float::{self, FloatTip};
use crate::text::t;

/// How a file's signatures hold, all told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mood {
    /// Each holds, and each signer is vouched for.
    Good,
    /// They hold, but not every signer is vouched for.
    Unsure,
    /// One at least does not hold.
    Bad,
}

/// The width of a signature's card.
const CARD_W: f32 = 440.0;

impl Viewer {
    /// The signatures that count: document timestamps vouch for the
    /// others and are not counted, unless they do not hold.
    fn counted(rows: &[SigRow]) -> Vec<(usize, &SigRow)> {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| !r.stamp_only || r.problem.is_some())
            .collect()
    }

    /// What the file's signatures come to, in a line, and how they hold.
    fn signatures_summary(&self) -> Option<(String, Mood)> {
        let all = self.signatures.as_ref().filter(|r| !r.is_empty())?;
        let rows: Vec<&SigRow> = all.iter().filter(|r| !r.stamp_only).collect();
        if rows.is_empty() {
            return None;
        }
        let bad: Vec<&SigRow> = all.iter().filter(|r| r.problem.is_some()).collect();
        let unverified = rows.iter().any(|r| r.unverified());
        // Changed after the last signature: none covers the whole file
        // (the form may list them in any order).
        let changed = !rows.iter().any(|r| r.whole);
        let count = rows.len().to_string();
        let signer = |r: &SigRow| r.signer.clone().unwrap_or_else(|| r.name.clone());
        let certifier = rows.iter().find(|r| r.certifies.is_some());
        let message = match (bad.len(), rows.len()) {
            (0, 1) if unverified => t("signed-by-unverified", &[("name", &signer(rows[0]))]),
            (0, _) if unverified => t("signed-unverified", &[("count", &count)]),
            (0, _) if certifier.is_some() && !changed => {
                let r = certifier.copied().unwrap_or(rows[0]);
                let allowed = r.certifies.map(allowed_id).unwrap_or("certify-nothing");
                t(
                    &allowed.replace("certify", "certified-by"),
                    &[("name", &signer(r))],
                )
            }
            (0, 1) if !changed => t("signed-by", &[("name", &signer(rows[0]))]),
            (0, _) if !changed => t("signed-all", &[("count", &count)]),
            (0, _) => t("signed-changed", &[("count", &count)]),
            (1, _) if bad[0].revision.is_some() => {
                t("signed-changed-after", &[("name", &signer(bad[0]))])
            }
            (1, _) => t("signed-bad-one", &[]),
            (n, _) => t("signed-bad", &[("count", &n.to_string())]),
        };
        let mood = match (bad.is_empty(), unverified) {
            (false, _) => Mood::Bad,
            (true, true) => Mood::Unsure,
            (true, false) => Mood::Good,
        };
        Some((message, mood))
    }

    /// The colour a mood is told in, its soft background and its icon.
    fn mood_colours(&self, mood: Mood) -> (Hsla, Hsla, &'static str) {
        let th = self.theme;
        let soft = |c: u32| gpui::rgba(c & 0xffff_ff00 | if th.dark { 0x33 } else { 0x24 });
        match mood {
            Mood::Good => (gpui::rgba(th.valid).into(), soft(th.valid).into(), "shield"),
            Mood::Unsure => (
                gpui::rgba(th.katna.warning).into(),
                soft(th.katna.warning).into(),
                "shieldq",
            ),
            Mood::Bad => (th.error, th.error_soft, "shieldx"),
        }
    }

    fn close_signature_card(&mut self, cx: &mut Context<Self>) {
        self.sig_open = None;
        self.sig_certificate = false;
        cx.notify();
    }

    /// The mood of one signature.
    fn row_mood(row: &SigRow) -> Mood {
        if row.problem.is_some() {
            Mood::Bad
        } else if row.unverified() {
            Mood::Unsure
        } else {
            Mood::Good
        }
    }

    /// Opens the Signatures pane, and the card of the signature that
    /// most needs looking at.
    fn signature_details(&mut self, cx: &mut Context<Self>) {
        self.pane = true;
        self.tab = PaneTab::Signatures;
        let rows = self.signatures.clone().unwrap_or_default();
        let first = rows
            .iter()
            .position(|r| r.problem.is_some())
            .or_else(|| rows.iter().position(SigRow::unverified))
            .or_else(|| Self::counted(&rows).first().map(|(i, _)| *i));
        self.sig_open = first;
        self.sig_certificate = false;
        cx.notify();
    }

    /// The line over the pages of a signed file (G08–G10): whether its
    /// signatures hold, coloured to match, with a button to see why. It
    /// stays out of the way when they all hold and the pane lists them.
    /// While editing a signed file (E16): changing its pages breaks the
    /// signatures, so a copy can be saved to edit instead.
    fn signed_edit_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.text_editing
            || self.signatures.as_ref().is_none_or(|rows| rows.is_empty())
            || (self.signed_copy.is_some() && self.signed_copy == self.path)
        {
            return None;
        }
        let th = self.theme;
        let (colour, soft, _) = self.mood_colours(Mood::Unsure);
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .pl(px(space::S4))
                .pr(px(space::S2))
                .py(px(space::S2))
                .bg(soft)
                .child(float::icon("alert", 16.0, colour))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::SMALL))
                        .text_color(colour)
                        .child(t("signed-edit-warning", &[])),
                )
                .child(
                    float::button("signed-edit-copy", t("signed-edit-copy", &[]), false, &th)
                        .h(px(28.0))
                        .ftip(t("signed-edit-copy-tip", &[]), &th)
                        .on_click(cx.listener(|this, _, _, cx| this.save_copy_to_edit(cx))),
                )
                .into_any_element(),
        )
    }

    /// Asks where to save a copy of the signed file, writes it there and
    /// opens it in its place, to edit with the original left signed.
    fn save_copy_to_edit(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let folder = path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let name = t(
            "signed-edit-copy-name",
            &[(
                "name",
                &path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into()),
            )],
        );
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        let current = self.unsaved();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let to = target.clone();
            let written = cx
                .background_executor()
                .spawn(async move {
                    match current {
                        Some(bytes) => std::fs::write(&to, bytes.as_slice()),
                        None => std::fs::copy(&path, &to).map(|_| ()),
                    }
                })
                .await;
            this.update(cx, |this, cx| match written {
                Ok(()) => {
                    this.signed_copy = Some(target.clone());
                    this.open(target, cx);
                }
                Err(error) => {
                    this.notice = Some(super::said(
                        "signed-edit-copy-failed",
                        &[("error", &error.to_string())],
                    ));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn signatures_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.comparing.is_some() {
            return None;
        }
        if let Some(line) = self.signed_edit_line(cx) {
            return Some(line);
        }
        let (message, mood) = self.signatures_summary()?;
        if mood == Mood::Good && self.pane_shown() && self.tab == PaneTab::Signatures {
            return None;
        }
        let th = self.theme;
        let rows = self.signatures.clone().unwrap_or_default();
        let certified = rows.iter().any(|r| r.certifies.is_some());
        let (colour, soft, _) = match mood {
            Mood::Good if certified => (th.accent_text, th.accent_soft, ""),
            _ => self.mood_colours(mood),
        };
        let glyph = match mood {
            Mood::Good if certified => "info",
            Mood::Good => "shield",
            _ => "alert",
        };
        // A file changed after one signer signed: show what they signed.
        let signed_version = match mood {
            Mood::Bad => rows
                .iter()
                .find(|r| r.problem.is_some())
                .and_then(|r| Some((r.revision?, r.signer.clone().unwrap_or_default()))),
            _ => None,
        };
        let button = match signed_version {
            Some((end, signer)) => float::button(
                "signatures-signed-version",
                t("signature-signed-version", &[]),
                false,
                &th,
            )
            .ftip(t("signature-signed-version-tip", &[]), &th)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_signed_version(end, &signer, cx);
            })),
            None => float::button("signatures-show", t("signatures-details", &[]), false, &th)
                .on_click(cx.listener(|this, _, _, cx| this.signature_details(cx))),
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .pl(px(space::S4))
                .pr(px(space::S2))
                .py(px(space::S2))
                .bg(soft)
                .child(float::icon(glyph, 16.0, colour))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::SMALL))
                        .text_color(colour)
                        .child(message),
                )
                .child(button.h(px(28.0)))
                .into_any_element(),
        )
    }

    /// The side pane's Signatures tab (G07): what they come to, a card
    /// for each signer, and the report.
    pub(super) fn signatures_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let rows = self.signatures.clone().unwrap_or_default();
        let mut list = div()
            .id("signatures-list")
            .size_full()
            .overflow_y_scroll()
            .px(px(space::S3))
            .pb(px(space::S4))
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .h(px(32.0))
                    .flex()
                    .items_end()
                    .pl(px(space::S1))
                    .text_size(px(text::MICRO))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.dim)
                    .child(t("pane-signatures", &[]).to_uppercase()),
            );
        if let Some((message, mood)) = self.signatures_summary() {
            let (colour, soft, glyph) = self.mood_colours(mood);
            list = list.child(
                div()
                    .flex()
                    .gap(px(space::S3))
                    .p(px(space::S4))
                    .rounded(px(radius::SM))
                    .bg(soft)
                    .child(float::icon(glyph, 16.0, colour).mt(px(2.0)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(text::SMALL))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colour)
                            .child(message),
                    ),
            );
        }
        for (i, row) in Self::counted(&rows) {
            list = list.child(self.signer_card(i, row, cx));
        }
        if !rows.is_empty() {
            list = list.child(
                div().flex().child(
                    float::button("signatures-report", t("signatures-report", &[]), false, &th)
                        .ftip(t("signatures-report-tip", &[]), &th)
                        .on_click(cx.listener(|this, _, _, cx| this.save_report(cx))),
                ),
            );
        }
        list.into_any_element()
    }

    /// One signer's card in the pane: who, when, and whether who they
    /// are is vouched for. A click opens what was checked beside it.
    fn signer_card(&self, i: usize, row: &SigRow, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let mood = Self::row_mood(row);
        let (colour, _, glyph) = self.mood_colours(mood);
        let open = self.sig_open == Some(i);
        let signer = row.signer.clone().unwrap_or_else(|| row.name.clone());
        let mut when: Vec<String> = row.time.iter().map(|time| short_time(time)).collect();
        if row.stamp.as_ref().is_some_and(|s| s.valid) {
            when.push(t("signature-stamped", &[]));
        }
        if let Some(allowed) = row.certifies {
            when.push(t(
                "signature-certifies",
                &[(
                    "allowed",
                    &t(&allowed_id(allowed).replace("certify", "allowed"), &[]),
                )],
            ));
        }
        let (line, line_colour) = match (&row.problem, &row.vouched) {
            (Some(problem), _) => (t("signature-bad", &[("problem", problem)]), th.error),
            (None, Some(Vouched::By(root))) => {
                (t("signer-vouched-short", &[("root", root)]), th.text)
            }
            (None, Some(Vouched::Not { .. })) => (t("signer-not-vouched-short", &[]), colour),
            (None, None) => (state(row), th.text),
        };
        let page = row.page;
        let hover = th.hover;
        div()
            .id(("signer-card", i))
            .p(px(space::S4))
            .rounded(px(radius::SM))
            .border_1()
            .border_color(if open { th.accent } else { th.line })
            .when(open, |d| d.bg(th.accent_soft))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .cursor_pointer()
            .hover(move |s| if open { s } else { s.bg(hover) })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.sig_open = if this.sig_open == Some(i) {
                    None
                } else {
                    Some(i)
                };
                this.sig_certificate = false;
                if let Some(page) = page.filter(|_| this.sig_open.is_some()) {
                    this.go_to(katna_pdf_cos::Destination { page, top: None }, cx);
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(float::icon(glyph, 14.0, colour))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(signer),
                    ),
            )
            .when(!when.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(when.join(" · ")),
                )
            })
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(line_colour)
                    .child(line),
            )
            .into_any_element()
    }

    /// The card of the signature opened in the pane (G07–G09), beside
    /// the pane: what was checked, row by row, and what can be done.
    pub(super) fn signature_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let i = self.sig_open?;
        if !(self.pane_shown() && self.tab == PaneTab::Signatures) {
            return None;
        }
        let rows = self.signatures.clone()?;
        let row = rows.get(i)?.clone();
        let th = self.theme;
        let mood = Self::row_mood(&row);
        let (colour, _, glyph) = self.mood_colours(mood);
        let signer = row.signer.clone().unwrap_or_else(|| row.name.clone());
        let title = match mood {
            Mood::Bad => t("signature-card-broken", &[]),
            Mood::Unsure => t("signature-card-unsure", &[]),
            Mood::Good if row.stamp_only => signer.clone(),
            Mood::Good => t("signature-card-good", &[("name", &signer)]),
        };
        let mut rows_shown: Vec<(String, String, Option<Hsla>)> = Vec::new();
        let mut shown = |key: &str, value: String, tint: Option<Hsla>| {
            rows_shown.push((t(key, &[]), value, tint));
        };
        let signed = match (&row.time, &row.stamp) {
            (Some(time), Some(stamp)) if stamp.valid => t(
                "signature-card-signed-stamped",
                &[
                    ("time", time),
                    ("authority", stamp.authority.as_deref().unwrap_or("?")),
                ],
            ),
            (Some(time), _) => time.clone(),
            (None, _) => String::new(),
        };
        let since = if row.later.is_empty() {
            t("signature-card-nothing", &[])
        } else {
            row.later.join(", ")
        };
        let revocation = match &row.revocation {
            Some(Revocation::Good) => (t("signature-card-not-revoked", &[]), None),
            Some(Revocation::Revoked { when, .. }) => (
                t(
                    "signature-card-revoked",
                    &[("when", when.as_deref().unwrap_or("?"))],
                ),
                Some(th.error),
            ),
            Some(Revocation::Unknown { why, .. }) => (
                t("signature-card-unknown", &[("why", why)]),
                Some(gpui::rgba(th.katna.warning).into()),
            ),
            None => (t("signature-card-not-asked", &[]), None),
        };
        let identity = match &row.vouched {
            Some(Vouched::By(root)) => t("signature-card-chained", &[("root", root)]),
            Some(Vouched::Not { .. }) => t("signer-not-vouched-short", &[]),
            None => String::new(),
        };
        let allowed = row
            .certifies
            .map(|a| t(&allowed_id(a).replace("certify", "allowed"), &[]));
        match mood {
            Mood::Bad => {
                shown(
                    "signature-card-problem",
                    capital(row.problem.as_deref().unwrap_or_default()),
                    Some(th.error),
                );
                if let Some(allowed) = allowed {
                    shown("signature-card-allowed", allowed, None);
                }
                shown("signature-card-since", since, None);
                shown("signature-card-certificate", revocation.0, revocation.1);
            }
            Mood::Good | Mood::Unsure => {
                if !signed.is_empty() {
                    shown("signature-card-signed", signed, None);
                }
                shown(
                    "signature-card-covers",
                    t(
                        if row.revision.is_some() || !row.whole {
                            "signature-card-covers-then"
                        } else {
                            "signature-card-covers-whole"
                        },
                        &[],
                    ),
                    None,
                );
                shown("signature-card-since", since, None);
                if mood == Mood::Good && !identity.is_empty() {
                    shown("signature-card-identity", identity, None);
                }
                shown("signature-card-revocation", revocation.0, revocation.1);
                if let Some(allowed) = allowed {
                    shown("signature-card-allowed", allowed, None);
                }
            }
        }
        if let Some(locks) = &row.locks {
            shown("signature-card-locks", locks.clone(), None);
        }
        if let Some(reason) = &row.reason {
            shown("signature-card-reason", reason.clone(), None);
        }
        if let Some(location) = &row.location {
            shown("signature-card-place", location.clone(), None);
        }
        if !row.kind.is_empty() {
            shown("signature-card-kind", row.kind.clone(), None);
        }
        if self.sig_certificate
            && let Some(cert) = &row.certificate
        {
            let tbs = &cert.tbs_certificate;
            // The day is enough.
            let day = |time: std::time::SystemTime| {
                katna_pdf_sign::shown_date(time)
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            };
            shown("certificate-to", common_name(&tbs.subject), None);
            shown("certificate-by", common_name(&tbs.issuer), None);
            shown(
                "certificate-valid",
                t(
                    "certificate-valid-dates",
                    &[
                        ("from", &day(tbs.validity.not_before.to_system_time())),
                        ("until", &day(tbs.validity.not_after.to_system_time())),
                    ],
                ),
                None,
            );
            shown(
                "certificate-serial",
                katna_pdf_sign::serial_text(cert),
                None,
            );
            shown("certificate-key", katna_pdf_sign::key_text(cert), None);
            shown(
                "certificate-fingerprint",
                katna_pdf_sign::fingerprint(cert),
                None,
            );
        }
        let table =
            div()
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .children(rows_shown.into_iter().map(|(key, value, tint)| {
                    div()
                        .flex()
                        .gap(px(space::S4))
                        .text_size(px(text::SMALL))
                        .child(div().w(px(130.0)).flex_none().text_color(th.dim).child(key))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(tint.unwrap_or(th.text))
                                .child(value),
                        )
                }));
        // Who signed can't be confirmed (G08): why, and the chain.
        let unsure = (mood == Mood::Unsure).then(|| {
            let (why, root) = match &row.vouched {
                Some(Vouched::Not { why, root }) => (why.clone(), root.clone()),
                _ => (String::new(), None),
            };
            let (names, whole) = row.chain.clone();
            let end = if whole {
                t("signature-chain-untrusted", &[])
            } else {
                t("signature-chain-not-sent", &[])
            };
            let intro = t(
                if row.whole {
                    "signature-card-unchanged"
                } else {
                    "signature-card-unchanged-then"
                },
                &[],
            );
            (
                format!("{intro} {why}"),
                names.join(" → "),
                end,
                root,
                whole,
            )
        });
        let head = div()
            .flex()
            .items_center()
            .gap(px(space::S3))
            .child(float::icon(glyph, 20.0, colour))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(text::SUBTITLE))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(title),
            )
            .when(mood == Mood::Good, |d| {
                d.child(
                    float::sized_ib(
                        "signature-card-x",
                        "x",
                        28.0,
                        16.0,
                        t("signature-close", &[]),
                        false,
                        &th,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.close_signature_card(cx))),
                )
            });
        let mut body = div()
            .p(px(space::S5))
            .flex()
            .flex_col()
            .gap(px(space::S4))
            .child(head);
        if let Some((words, chain, end, _, _)) = &unsure {
            body = body
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.text2)
                        .child(words.clone()),
                )
                .child(
                    div()
                        .p(px(space::S4))
                        .rounded(px(radius::SM))
                        .border_1()
                        .border_color(th.line)
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(th.dim)
                                .child(t("signature-chain", &[])),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_x(px(space::S2))
                                .text_size(px(text::SMALL))
                                .child(div().text_color(th.text).child(format!("{chain} →")))
                                .child(div().text_color(th.error).child(end.clone())),
                        ),
                );
        }
        body = body.child(table);
        // What can be done.
        let mut left = div().flex().items_center().gap(px(space::S2));
        let mut right = div().flex().items_center().gap(px(space::S2));
        let quiet = |b: gpui::Stateful<gpui::Div>| {
            b.border_color(gpui::transparent_black())
                .bg(gpui::transparent_black())
        };
        let signer_name = signer.clone();
        match &unsure {
            Some((_, _, _, root, whole)) => {
                if !whole && let Some(path) = self.path.clone() {
                    let generation = self.generation;
                    left = left.child(
                        quiet(float::button(
                            "signature-fetch",
                            t("signature-fetch-issuers", &[]),
                            false,
                            &th,
                        ))
                        .ftip(t("signature-fetch-issuers-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.check_signatures_online(path.clone(), generation, cx);
                        })),
                    );
                }
                right = right.child(
                    float::button("signature-close", t("signature-close", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.close_signature_card(cx))),
                );
                if let Some(root) = root.clone() {
                    let name = common_name(&root.tbs_certificate.subject);
                    right = right.child(
                        float::button(
                            "signature-trust",
                            t("signer-trust-root", &[("root", &name)]),
                            true,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.trust(&root, cx))),
                    );
                }
            }
            None if mood == Mood::Bad => {
                if let Some(end) = row.revision {
                    left = left.child(
                        quiet(float::button(
                            "signature-compare",
                            t("signature-compare-signed", &[]),
                            false,
                            &th,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.compare_signed_version(end, &signer_name, cx);
                        })),
                    );
                }
                right = right.child(
                    float::button("signature-close", t("signature-close", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.close_signature_card(cx))),
                );
            }
            None => {
                if row.certificate.is_some() {
                    let showing = self.sig_certificate;
                    left = left.child(
                        float::button(
                            "signature-certificate",
                            t(
                                if showing {
                                    "signature-hide-certificate"
                                } else {
                                    "signature-see-certificate"
                                },
                                &[],
                            ),
                            false,
                            &th,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.sig_certificate = !showing;
                            cx.notify();
                        })),
                    );
                }
                if let Some(end) = row.revision {
                    left = left.child(
                        float::button(
                            "signature-signed-version",
                            t("signature-signed-version", &[]),
                            false,
                            &th,
                        )
                        .ftip(t("signature-signed-version-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_signed_version(end, &signer_name, cx);
                        })),
                    );
                }
            }
        }
        let footer_line = mood != Mood::Good;
        let footer = div()
            .px(px(space::S5))
            .pb(px(space::S4))
            .when(footer_line, |d| {
                d.pt(px(space::S4)).border_t_1().border_color(th.line_soft)
            })
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::S2))
            .child(left)
            .child(right);
        Some(
            div()
                .id("signature-card")
                .w(px(CARD_W))
                .max_w_full()
                .max_h(px(560.0))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .rounded(px(radius::MD))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .shadow(th.lifted())
                .occlude()
                .child(body)
                .child(footer)
                .into_any_element(),
        )
    }
}

/// A time as a signature's card shows it: "14 Nov, 11:02", from
/// "2026-11-14 11:02:45 UTC".
fn short_time(time: &str) -> String {
    let shorter = || {
        let (date, clock) = time.split_once(' ')?;
        let mut parts = date.split('-');
        let (_, month, day) = (parts.next()?, parts.next()?, parts.next()?);
        let month: u32 = month.parse().ok()?;
        let name: String = t(&format!("date-month-{month}"), &[])
            .chars()
            .take(3)
            .collect();
        let day: u32 = day.parse().ok()?;
        Some(t(
            "signature-short-time",
            &[
                ("day", &day.to_string()),
                ("month", &name),
                ("time", clock.get(..5)?),
            ],
        ))
    };
    shorter().unwrap_or_else(|| time.to_owned())
}

/// `words` with a capital first letter, to stand on its own.
fn capital(words: &str) -> String {
    let mut letters = words.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_shortened_for_the_cards() {
        assert_eq!(short_time("2026-11-14 11:02:45 UTC"), "14 Nov, 11:02");
        assert_eq!(short_time("not a time"), "not a time");
    }
}
