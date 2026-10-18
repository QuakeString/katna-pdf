// SPDX-License-Identifier: GPL-3.0-or-later

//! Signatures (docs/PLAN.md §4.4). The signatures a file holds are
//! checked as it opens, shown in a line over the pages and listed in the
//! side pane. Signing: a box dragged on a page (or none, for a signature
//! not seen), then the certificate file and its password; the signed
//! copy is saved as a new file and opened in place of this one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Focusable as _, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{Coverage, Placement, SignRequest, SigningInfo};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::annots::Draft;
use super::{Content, PaneTab, Viewer, file_name, icon_button};
use crate::text::t;

/// A drag shorter than this, in points, places nothing.
const CLICK: f32 = 4.0;

/// One signature of the file, checked.
#[derive(Debug, Clone)]
pub(super) struct SigRow {
    pub name: String,
    pub signer: Option<String>,
    pub time: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub page: Option<usize>,
    pub intact: bool,
    /// It covers the whole file: nothing was added after it.
    pub whole: bool,
    pub problem: Option<String>,
}

/// The signatures of `doc`, checked.
pub(super) fn check(doc: &katna_pdf_cos::Document) -> Vec<SigRow> {
    katna_pdf_sign::check(doc, doc.bytes())
        .into_iter()
        .map(|c| SigRow {
            name: c.signature.name.clone(),
            signer: c.signer(),
            time: c.time(),
            reason: c.signature.info.reason.clone(),
            location: c.signature.info.location.clone(),
            // A signature not seen has a box of no size.
            page: c.signature.page.filter(|_| {
                let r = c.signature.rect;
                (r[2] - r[0]).abs() > 0.0 && (r[3] - r[1]).abs() > 0.0
            }),
            intact: c.intact(),
            whole: c.coverage == Coverage::Whole,
            problem: c.problem(),
        })
        .collect()
}

/// Where the signature goes.
#[derive(Debug, Clone, Copy)]
enum Placed {
    /// In a box on page `page`: `shown` in points from its top left as
    /// shown, `rect` in its own space.
    Box {
        page: usize,
        shown: [f32; 4],
        rect: [f32; 4],
    },
    /// Not seen on any page.
    Hidden,
}

/// Signing, from placing the box to the signed file being written.
pub(super) struct Signing {
    placed: Option<Placed>,
    certificate: Option<PathBuf>,
    password: Entity<TextInput>,
    reason: Entity<TextInput>,
    location: Entity<TextInput>,
    error: Option<String>,
    busy: bool,
    /// Put the keys in the password field on the next frame.
    focus: bool,
    _inputs: Vec<Subscription>,
}

impl Viewer {
    /// Starts signing, or stops it.
    pub(super) fn toggle_signing(&mut self, cx: &mut Context<Self>) {
        if self.signing.take().is_some() {
            self.draft = None;
            cx.notify();
            return;
        }
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        self.finish_typing(true, cx);
        self.tool = None;
        self.tools = false;
        self.cropping = false;
        self.organizer = false;
        self.draft = None;
        let field = |placeholder: String, masked: bool, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(placeholder, cx);
                input.set_masked(masked, cx);
                input
            })
        };
        let password = field(t("sign-password", &[]), true, cx);
        let reason = field(t("sign-reason", &[]), false, cx);
        let location = field(t("sign-location", &[]), false, cx);
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let mut subs = Vec::new();
        for input in [&password, &reason, &location] {
            input.update(cx, |input, _| input.set_accent(accent));
            subs.push(cx.subscribe(input, |this, _, event, cx| match event {
                InputEvent::Submit => this.sign_now(cx),
                InputEvent::Cancel => this.toggle_signing(cx),
                InputEvent::Changed => {}
            }));
        }
        self.signing = Some(Signing {
            placed: None,
            certificate: remembered_certificate(),
            password,
            reason,
            location,
            error: None,
            busy: false,
            focus: false,
            _inputs: subs,
        });
        cx.notify();
    }

    /// A press on the pages while placing the signature starts its box.
    pub(super) fn sign_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.signing.as_ref().is_some_and(|s| s.placed.is_none()) {
            return false;
        }
        if let Some((page, point)) = self.page_point(position) {
            self.selection = None;
            self.draft = Some(Draft {
                page,
                path: vec![point],
            });
            cx.notify();
        }
        true
    }

    /// The pointer let go while placing: the box is placed, and the
    /// certificate is asked for.
    pub(super) fn sign_release(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.signing.as_ref().is_some_and(|s| s.placed.is_none()) {
            return false;
        }
        let Some(draft) = self.draft.take() else {
            return true;
        };
        cx.notify();
        let (Some(from), Some(to)) = (draft.path.first(), draft.path.last()) else {
            return true;
        };
        if (from[0] - to[0]).abs() < CLICK || (from[1] - to[1]).abs() < CLICK {
            return true;
        }
        let Some(inv) = self.inverse(draft.page) else {
            return true;
        };
        let shown = [
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]),
            from[1].max(to[1]),
        ];
        self.place(
            Placed::Box {
                page: draft.page,
                shown,
                rect: inv.rect(shown),
            },
            cx,
        );
        true
    }

    fn place(&mut self, placed: Placed, cx: &mut Context<Self>) {
        if let Some(signing) = self.signing.as_mut() {
            signing.placed = Some(placed);
            signing.focus = true;
            cx.notify();
        }
    }

    /// The box being dragged, or placed, on page `page`.
    pub(super) fn sign_overlay(&self, page: usize) -> Option<AnyElement> {
        let signing = self.signing.as_ref()?;
        let [x0, y0, x1, y1] = match (signing.placed, &self.draft) {
            (Some(Placed::Box { page: p, shown, .. }), _) if p == page => shown,
            (None, Some(draft)) if draft.page == page => {
                let (from, to) = (draft.path.first()?, draft.path.last()?);
                [
                    from[0].min(to[0]),
                    from[1].min(to[1]),
                    from[0].max(to[0]),
                    from[1].max(to[1]),
                ]
            }
            _ => return None,
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent;
        Some(
            div()
                .absolute()
                .left(desktop_px(x0 * shown))
                .top(desktop_px(y0 * shown))
                .w(desktop_px((x1 - x0) * shown))
                .h(desktop_px((y1 - y0) * shown))
                .border_2()
                .border_color(gpui::rgba(accent))
                .bg(gpui::rgba(accent & 0xffff_ff00 | 0x1a))
                .into_any_element(),
        )
    }

    /// Asks for the certificate file to sign with.
    fn choose_certificate(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    if let Some(signing) = this.signing.as_mut() {
                        signing.certificate = Some(file);
                        signing.error = None;
                        signing.focus = true;
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    /// Signs with what was given: asks where to save the signed copy,
    /// writes it, and opens it.
    fn sign_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        let Some(placed) = signing.placed.filter(|_| !signing.busy) else {
            return;
        };
        let Some(certificate) = signing.certificate.clone() else {
            signing.error = Some(t("sign-no-certificate", &[]));
            cx.notify();
            return;
        };
        let password = signing.password.read(cx).text().to_owned();
        let value = |input: &Entity<TextInput>| {
            Some(input.read(cx).text().trim().to_owned()).filter(|s| !s.is_empty())
        };
        let (reason, location) = (value(&signing.reason), value(&signing.location));
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let suggested = t("sign-file-name", &[("name", &stem)]);
        signing.busy = true;
        signing.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // The password is tried before asking where to save.
            let cert = certificate.clone();
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let p12 =
                        std::fs::read(&cert).map_err(|e| format!("{}: {e}", file_name(&cert)))?;
                    katna_pdf_sign::p12::open(&p12, &password).map_err(|e| match e {
                        katna_pdf_sign::P12Error::Password => t("sign-wrong-password", &[]),
                        e => e.to_string(),
                    })
                })
                .await;
            let stop =
                |this: gpui::WeakEntity<Viewer>, error: Option<String>, cx: &mut gpui::AsyncApp| {
                    this.update(cx, |this, cx| {
                        if let Some(signing) = this.signing.as_mut() {
                            signing.busy = false;
                            signing.focus = error.is_some();
                            signing.error = error;
                            cx.notify();
                        }
                    })
                    .ok();
                };
            let id = match opened {
                Ok(id) => id,
                Err(error) => return stop(this, Some(error), cx),
            };
            let Ok(chosen) = this.update(cx, |_, cx| {
                cx.prompt_for_new_path(&folder, Some(&suggested))
            }) else {
                return;
            };
            let target = match chosen.await {
                Ok(Ok(Some(target))) => target,
                Ok(Ok(None)) => return stop(this, None, cx),
                Ok(Err(error)) => {
                    let error = t("sign-no-dialog", &[("error", &error.to_string())]);
                    return stop(this, Some(error), cx);
                }
                Err(_) => {
                    let error = t("sign-no-dialog", &[("error", "")]);
                    return stop(this, Some(error), cx);
                }
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    let time = std::time::SystemTime::now();
                    let placement = match placed {
                        Placed::Hidden => Placement::Invisible,
                        Placed::Box { page, rect, .. } => Placement::Visible {
                            page,
                            rect,
                            lines: katna_pdf_sign::appearance_lines(
                                &id,
                                &katna_pdf_sign::shown_date(time),
                                reason.as_deref(),
                            ),
                        },
                    };
                    let name = katna_pdf_sign::common_name(&id.certificate.tbs_certificate.subject);
                    let mut request = SignRequest::new(
                        placement,
                        SigningInfo {
                            name: Some(name),
                            reason,
                            location,
                            contact: None,
                        },
                    );
                    request.time = time;
                    let signed =
                        katna_pdf_sign::sign(&doc, &request, &id, katna_pdf_sign::Digest::Sha256)
                            .map_err(|e| e.to_string())?;
                    crate::viewer::save_over(&target, &signed).map_err(|e| e.to_string())?;
                    remember_certificate(&certificate);
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(target) => {
                        this.signing = None;
                        this.open_with(target, String::new(), cx);
                    }
                    Err(error) => {
                        if let Some(signing) = this.signing.as_mut() {
                            signing.busy = false;
                            signing.error = Some(error);
                            signing.focus = true;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts the keys in the password field when asked to.
    pub(super) fn focus_signing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(signing) = self.signing.as_mut()
            && std::mem::take(&mut signing.focus)
        {
            let input = signing.password.clone();
            window.focus(&input.focus_handle(cx), cx);
        }
    }

    /// What to do while placing the signature, over the pages' top
    /// right corner, so the pages do not move as it comes and goes.
    fn placing_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        div()
            .id("signing-place")
            .absolute()
            .top(px(space::S3))
            .right(px(space::S5))
            .max_w(px(420.0))
            .p(px(space::S3))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow(widgets::elevation(&th.katna, 2.0))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(widgets::icon("signature", th.katna.accent, 18.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(text::SMALL))
                    .text_color(th.text)
                    .child(t("sign-place", &[])),
            )
            .child(
                widgets::outlined_button("sign-hidden", t("sign-hidden", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| this.place(Placed::Hidden, cx))),
            )
            .child(icon_button(
                "sign-cancel",
                "close",
                t("sign-cancel", &[]),
                th,
                cx.listener(|this, _, _, cx| this.toggle_signing(cx)),
            ))
            .into_any_element()
    }

    /// The card over the pages while signing: where to put the
    /// signature, then the certificate and its password.
    pub(super) fn signing_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let signing = self.signing.as_ref()?;
        if signing.placed.is_none() {
            return Some(self.placing_card(cx));
        }
        let th = self.theme;
        let field = |input: &Entity<TextInput>| {
            div()
                .h(px(36.0))
                .flex()
                .items_center()
                .px(px(space::S3))
                .rounded(px(radius::SM))
                .bg(th.field)
                .text_size(px(text::BODY))
                .text_color(th.text)
                .child(input.clone())
        };
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let certificate = signing
            .certificate
            .as_deref()
            .map_or_else(|| t("sign-certificate-none", &[]), file_name);
        let busy = signing.busy;
        Some(
            div()
                .id("signing-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(320.0))
                .p(px(space::S4))
                .flex()
                .flex_col()
                .gap(px(space::S2))
                .rounded(px(radius::MD))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .shadow(widgets::elevation(&th.katna, 2.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.text)
                        .child(t("sign-title", &[])),
                )
                .child(label("sign-certificate"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .text_size(px(text::BODY))
                                .text_color(if signing.certificate.is_some() {
                                    th.text
                                } else {
                                    th.dim
                                })
                                .child(certificate),
                        )
                        .child(
                            widgets::outlined_button(
                                "sign-choose",
                                t("sign-choose", &[]),
                                &th.katna,
                            )
                            .tip(t("sign-choose-tip", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_certificate(cx))),
                        ),
                )
                .child(field(&signing.password))
                .child(field(&signing.reason))
                .child(field(&signing.location))
                .children(signing.error.clone().map(|error| {
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.error)
                        .child(error)
                }))
                .child(
                    div()
                        .pt(px(space::S2))
                        .flex()
                        .justify_end()
                        .gap(px(space::S2))
                        .child(
                            widgets::outlined_button("sign-back", t("sign-cancel", &[]), &th.katna)
                                .on_click(cx.listener(|this, _, _, cx| this.toggle_signing(cx))),
                        )
                        .child(
                            widgets::filled_button(
                                "sign-go",
                                t(if busy { "sign-busy" } else { "sign-go" }, &[]),
                                &th.katna,
                            )
                            .when(busy, |b| b.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.sign_now(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// The line over the pages of a signed file: whether its signatures
    /// hold, with a button to list them.
    pub(super) fn signatures_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.signatures.as_ref().filter(|r| !r.is_empty())?;
        let th = self.theme;
        let bad = rows.iter().filter(|r| !r.intact).count();
        let changed = rows.last().is_some_and(|r| r.intact && !r.whole);
        let count = rows.len().to_string();
        let message = match (bad, rows.len(), &rows[0].signer) {
            (0, 1, Some(signer)) if !changed => t("signed-by", &[("name", signer)]),
            (0, _, _) if !changed => t("signed-all", &[("count", &count)]),
            (0, _, _) => t("signed-changed", &[("count", &count)]),
            _ if bad == 1 => t("signed-bad-one", &[]),
            _ => t("signed-bad", &[("count", &bad.to_string())]),
        };
        let (icon, colour) = if bad == 0 {
            ("shield-check", th.valid)
        } else {
            ("shield-alert", th.katna.error)
        };
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .py(px(space::S1))
                .border_b_1()
                .border_color(th.line)
                .child(widgets::icon(icon, colour, 18.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(message),
                )
                .child(
                    widgets::outlined_button(
                        "signatures-show",
                        t("signatures-show", &[]),
                        &th.katna,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pane = true;
                        this.tab = PaneTab::Signatures;
                        cx.notify();
                    })),
                )
                .into_any_element(),
        )
    }

    /// The side pane's list of signatures.
    pub(super) fn signatures_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let mut list = div()
            .id("signatures-list")
            .size_full()
            .overflow_y_scroll()
            .py(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1));
        let rows = self.signatures.clone().unwrap_or_default();
        for (i, row) in rows.iter().enumerate() {
            let (icon, colour, state) = match &row.problem {
                Some(problem) => (
                    "shield-alert",
                    th.katna.error,
                    t("signature-bad", &[("problem", problem)]),
                ),
                None if row.whole => ("shield-check", th.valid, t("signature-intact", &[])),
                None => ("shield-check", th.valid, t("signature-intact-changed", &[])),
            };
            let mut details = Vec::new();
            if let Some(time) = &row.time {
                details.push(t("signature-time", &[("time", time)]));
            }
            if let Some(reason) = &row.reason {
                details.push(t("signature-reason", &[("reason", reason)]));
            }
            if let Some(location) = &row.location {
                details.push(t("signature-location", &[("location", location)]));
            }
            if let Some(page) = row.page {
                details.push(t("comment-page", &[("page", &(page + 1).to_string())]));
            }
            let page = row.page;
            let signer = row.signer.clone().unwrap_or_else(|| row.name.clone());
            list = list.child(
                div()
                    .id(("signature", i))
                    .mx(px(space::S2))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::SM))
                    .flex()
                    .gap(px(space::S2))
                    .when(page.is_some(), |d| d.cursor_pointer())
                    .hover(move |s| s.bg(th.hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(page) = page {
                            this.go_to(katna_pdf_cos::Destination { page, top: None }, cx);
                        }
                    }))
                    .child(widgets::icon(icon, colour, 18.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(
                                div()
                                    .text_size(px(text::SMALL))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(signer),
                            )
                            .child(
                                div()
                                    .text_size(px(text::SMALL))
                                    .text_color(if row.intact { th.text } else { th.error })
                                    .child(state),
                            )
                            .children(details.into_iter().map(|d| {
                                div().text_size(px(text::SMALL)).text_color(th.dim).child(d)
                            })),
                    ),
            );
        }
        list = list.child(
            div()
                .px(px(space::S4))
                .py(px(space::S3))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("signatures-trust", &[])),
        );
        list.into_any_element()
    }
}

/// The file the last certificate chosen is remembered in (the path
/// only, never the password).
fn certificate_note() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join("certificate"))
}

fn remembered_certificate() -> Option<PathBuf> {
    let text = std::fs::read_to_string(certificate_note()?).ok()?;
    let path = PathBuf::from(text.trim_end_matches('\n'));
    path.is_file().then_some(path)
}

fn remember_certificate(path: &Path) {
    if let Some(note) = certificate_note() {
        if let Some(dir) = note.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(note, path.to_string_lossy().as_bytes());
    }
}

/// The signatures of the file shown, checked.
pub(super) type Rows = Arc<Vec<SigRow>>;
