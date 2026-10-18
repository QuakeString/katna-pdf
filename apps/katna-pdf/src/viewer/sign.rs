// SPDX-License-Identifier: GPL-3.0-or-later

//! Signatures (docs/PLAN.md §4.4). The signatures a file holds are
//! checked as it opens, shown in a line over the pages and listed in the
//! side pane. Signing: a box dragged on a page (or none, for a signature
//! not seen), then the certificate file and its password; the signed
//! copy is saved as a new file and opened in place of this one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use gpui::{AnyElement, Context, Entity, Focusable as _, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{Allowed, Change, Coverage, FieldKind, Placement, SignRequest, SigningInfo};
use katna_pdf_sign::{Certificate, Revocation, Roots, Trust, common_name, pkcs11};
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
    pub stamp: Option<katna_pdf_sign::Stamp>,
    /// Whether who signed is vouched for, when the signature could be
    /// read.
    pub vouched: Option<Vouched>,
    /// Whether their certificates were revoked, once asked online.
    pub revocation: Option<Revocation>,
    /// What it allows after it, when it certifies the file.
    pub certifies: Option<Allowed>,
}

/// What signing needs to know of the file.
#[derive(Debug, Clone, Default)]
pub(super) struct Signable {
    /// Its signature fields not signed yet.
    pub fields: Vec<EmptyField>,
    /// How it is certified, if it is.
    pub certified: Option<Allowed>,
    /// Whether it holds a signature.
    pub signed: bool,
}

#[derive(Debug, Clone)]
pub(super) struct EmptyField {
    pub name: String,
    pub page: usize,
    /// Left, bottom, right, top in the page's own space.
    pub rect: [f32; 4],
}

/// What signing `doc` needs to know of it.
pub(super) fn signable(doc: &katna_pdf_cos::Document) -> Signable {
    let signed: Vec<_> = doc.signatures().into_iter().map(|s| s.field).collect();
    let fields = doc
        .form()
        .into_iter()
        .filter(|f| f.kind == FieldKind::Signature && !signed.contains(&f.id) && !f.read_only)
        .filter_map(|f| {
            let w = f.widgets.first()?;
            let seen = w.rect[2] - w.rect[0] > 1.0 && w.rect[3] - w.rect[1] > 1.0;
            seen.then(|| EmptyField {
                name: f.name.clone(),
                page: w.page,
                rect: w.rect,
            })
        })
        .collect();
    Signable {
        fields,
        certified: doc.certification(),
        signed: !signed.is_empty(),
    }
}

/// The text id naming what a certification allows.
fn allowed_id(allowed: Allowed) -> &'static str {
    match allowed {
        Allowed::Nothing => "certify-nothing",
        Allowed::FormFilling => "certify-forms",
        Allowed::Comments => "certify-comments",
    }
}

fn change_id(change: Change) -> &'static str {
    match change {
        Change::FormFilled => "change-form",
        Change::Signed => "change-signed",
        Change::Commented => "change-comments",
        Change::NewField => "change-new-field",
        Change::Dss => "change-dss",
        Change::Pages => "change-pages",
        Change::Content => "change-content",
        Change::Other => "change-other",
    }
}

/// Whether a signer is who their certificate says.
#[derive(Debug, Clone)]
pub(super) enum Vouched {
    /// Their certificate was issued under this trusted root.
    By(String),
    /// It is not vouched for: why, and the root that, once trusted,
    /// would vouch for it.
    Not {
        why: String,
        root: Option<Arc<Certificate>>,
    },
}

impl SigRow {
    fn unverified(&self) -> bool {
        self.intact && matches!(self.vouched, Some(Vouched::Not { .. }))
    }
}

fn vouched(trust: &Trust) -> Vouched {
    let not = |why| Vouched::Not { why, root: None };
    match trust {
        Trust::Trusted { root } => Vouched::By(root.clone()),
        Trust::Untrusted { root, complete } => {
            let name = common_name(&root.tbs_certificate.subject);
            if *complete {
                Vouched::Not {
                    why: t("signer-root-untrusted", &[("root", &name)]),
                    root: Some(Arc::new((**root).clone())),
                }
            } else {
                not(t("signer-chain-incomplete", &[("name", &name)]))
            }
        }
        Trust::Expired { name } => not(t("signer-expired", &[("name", name)])),
        Trust::Forged { name } => not(t("signer-forged", &[("name", name)])),
    }
}

/// The folder of root certificates the person trusts, beside the
/// system's: the ones they trusted from a signature, and any they put
/// there (India's CCA roots, a company's own).
pub(super) fn trusted_folder() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.data_dir().join("pdf").join("trusted"))
}

/// The roots trusted: read once, then added to as the person trusts more.
static ROOTS: OnceLock<RwLock<Arc<Roots>>> = OnceLock::new();

fn roots_lock() -> &'static RwLock<Arc<Roots>> {
    ROOTS.get_or_init(|| {
        let mut roots = Roots::system();
        if let Some(folder) = trusted_folder() {
            roots.add_folder(&folder);
        }
        RwLock::new(Arc::new(roots))
    })
}

fn roots() -> Arc<Roots> {
    roots_lock().read().map(|r| r.clone()).unwrap_or_default()
}

/// Trusts `cert` as a root from now on.
fn trust_root(cert: &Certificate) -> std::io::Result<()> {
    let folder = trusted_folder().ok_or_else(|| std::io::Error::other("no data folder"))?;
    Roots::keep(&folder, cert)?;
    if let Ok(mut roots) = roots_lock().write() {
        let mut more = (**roots).clone();
        more.add(cert.clone());
        *roots = Arc::new(more);
    }
    Ok(())
}

/// The signatures of `doc`, checked.
pub(super) fn check(doc: &katna_pdf_cos::Document) -> Vec<SigRow> {
    rows(katna_pdf_sign::check(doc, doc.bytes(), &roots()))
}

/// The same, then checked further online: issuers the signatures did not
/// send are fetched, and whether the signers' certificates were revoked
/// is asked.
pub(super) fn check_online(doc: &katna_pdf_cos::Document) -> Vec<SigRow> {
    let roots = roots();
    let fetch = katna_pdf_sign::revocation::Http::default();
    let mut checks = katna_pdf_sign::check(doc, doc.bytes(), &roots);
    for c in &mut checks {
        katna_pdf_sign::check_online(c, &roots, &fetch);
    }
    rows(checks)
}

fn rows(checks: Vec<katna_pdf_sign::SignatureCheck>) -> Vec<SigRow> {
    checks
        .into_iter()
        .map(|c| {
            let problem = match &c.revocation {
                Some(Revocation::Revoked { name, when }) if c.intact() => Some(t(
                    "signer-revoked",
                    &[("name", name), ("when", when.as_deref().unwrap_or("?"))],
                )),
                _ if c.intact() && !c.forbidden.is_empty() => {
                    let changes: Vec<String> =
                        c.forbidden.iter().map(|f| t(change_id(*f), &[])).collect();
                    Some(t(
                        "signer-changed-after-certifying",
                        &[("changes", &changes.join(", "))],
                    ))
                }
                _ => c.problem(),
            };
            SigRow {
                name: c.signature.name.clone(),
                signer: c.signer(),
                time: c.time(),
                reason: c.signature.info.reason.clone(),
                location: c.signature.info.location.clone(),
                page: c.signature.page,
                intact: c.intact(),
                whole: c.coverage == Coverage::Whole,
                problem,
                stamp: c.result.as_ref().ok().and_then(|r| r.timestamp.clone()),
                vouched: c.trust.as_ref().map(vouched),
                revocation: c.revocation.clone(),
                certifies: c.signature.certify,
            }
        })
        .collect()
}

/// Where the signature goes.
#[derive(Debug, Clone)]
enum Placed {
    /// In the file's empty signature field `name`, shown on page `page`
    /// at `shown`.
    Field {
        name: String,
        page: usize,
        shown: [f32; 4],
    },
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

/// What the key is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// A certificate file (.pfx, .p12).
    File,
    /// A USB token, such as an ePass2003.
    Token,
}

/// A token plugged in, and the module that reaches it.
#[derive(Debug, Clone)]
struct Found {
    module: PathBuf,
    token: pkcs11::Token,
}

/// The tokens plugged in, as last looked for.
enum Tokens {
    NotLooked,
    Looking,
    Found(Vec<Found>),
    Failed(String),
}

/// Something that signs, made off the UI thread and used there.
type Key = Box<dyn katna_pdf_sign::Signer + Send>;

/// Signing, from placing the box to the signed file being written.
pub(super) struct Signing {
    placed: Option<Placed>,
    source: Source,
    certificate: Option<PathBuf>,
    tokens: Tokens,
    /// The token picked among those found.
    token: usize,
    password: Entity<TextInput>,
    reason: Entity<TextInput>,
    location: Entity<TextInput>,
    /// Whether to timestamp the signature, and the server to ask.
    timestamp: bool,
    server: Entity<TextInput>,
    /// Other files to sign at the same time, with the key unlocked once.
    others: Vec<PathBuf>,
    /// Certify the file, allowing these changes after.
    certify: Option<Allowed>,
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
        let server = field(t("sign-timestamp-server", &[]), false, cx);
        let (timestamp, url) = remembered_timestamp();
        server.update(cx, |input, cx| input.set_text(url, cx));
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let mut subs = Vec::new();
        for input in [&password, &reason, &location, &server] {
            input.update(cx, |input, _| input.set_accent(accent));
            subs.push(cx.subscribe(input, |this, _, event, cx| match event {
                InputEvent::Submit => this.sign_now(cx),
                InputEvent::Cancel => this.toggle_signing(cx),
                InputEvent::Changed => {}
            }));
        }
        let source = match read_note("source").as_deref().map(str::trim) {
            Some("token") => Source::Token,
            _ => Source::File,
        };
        self.signing = Some(Signing {
            placed: None,
            source: Source::File,
            certificate: remembered_certificate(),
            tokens: Tokens::NotLooked,
            token: 0,
            password,
            reason,
            location,
            timestamp,
            server,
            others: Vec::new(),
            certify: None,
            error: None,
            busy: false,
            focus: false,
            _inputs: subs,
        });
        self.set_source(source, cx);
        cx.notify();
    }

    /// Signs with a certificate file or a token from now on.
    fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        signing.source = source;
        signing.error = None;
        signing.focus = signing.placed.is_some();
        let placeholder = match source {
            Source::File => t("sign-password", &[]),
            Source::Token => t("sign-pin", &[]),
        };
        signing.password.update(cx, |input, cx| {
            input.set_placeholder(placeholder);
            input.set_text("", cx);
        });
        if source == Source::Token && matches!(signing.tokens, Tokens::NotLooked) {
            self.look_for_tokens(cx);
        }
        cx.notify();
    }

    /// Looks for tokens plugged in, through every token driver
    /// installed.
    fn look_for_tokens(&mut self, cx: &mut Context<Self>) {
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        if matches!(signing.tokens, Tokens::Looking) {
            return;
        }
        signing.tokens = Tokens::Looking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let modules = pkcs11::known_modules();
                    if modules.is_empty() {
                        return Tokens::Failed(t("sign-no-driver", &[]));
                    }
                    let mut found = Vec::new();
                    for path in modules {
                        let Ok(module) = pkcs11::Module::load(&path) else {
                            continue;
                        };
                        for token in module.tokens().unwrap_or_default() {
                            // The same token through two drivers: once.
                            if !found.iter().any(|f: &Found| f.token.serial == token.serial) {
                                found.push(Found {
                                    module: path.clone(),
                                    token,
                                });
                            }
                        }
                    }
                    Tokens::Found(found)
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(signing) = this.signing.as_mut() {
                    signing.token = 0;
                    signing.tokens = found;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
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
        // A certified file is signed in its own fields only.
        if self.signable.certified.is_some() {
            return true;
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

    /// The box being dragged, or placed, on page `page`; while placing,
    /// the file's empty signature fields there, to click.
    pub(super) fn sign_overlay(&self, page: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(signing) = self.signing.as_ref() else {
            return Vec::new();
        };
        let mut out: Vec<AnyElement> = self.sign_box(page).into_iter().collect();
        let Content::Ready(opened) = &self.content else {
            return out;
        };
        let (Some(transform), true) = (
            opened.doc.page_transform(page),
            signing.placed.is_none() && self.signable.certified != Some(Allowed::Nothing),
        ) else {
            return out;
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent;
        for (i, field) in self.signable.fields.iter().enumerate() {
            if field.page != page {
                continue;
            }
            let b = katna_pdf_render::map_rect(transform, field.rect);
            let placed = Placed::Field {
                name: field.name.clone(),
                page,
                shown: b,
            };
            out.push(
                div()
                    .id(("sign-field", i))
                    .absolute()
                    .left(desktop_px(b[0] * shown))
                    .top(desktop_px(b[1] * shown))
                    .w(desktop_px((b[2] - b[0]) * shown))
                    .h(desktop_px((b[3] - b[1]) * shown))
                    .border_2()
                    .border_dashed()
                    .border_color(gpui::rgba(accent))
                    .bg(gpui::rgba(accent & 0xffff_ff00 | 0x1a))
                    .hover(move |s| s.bg(gpui::rgba(accent & 0xffff_ff00 | 0x40)))
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| this.place(placed.clone(), cx)))
                    .tip(field.name.clone(), &self.theme.katna)
                    .into_any_element(),
            );
        }
        out
    }

    /// The box being dragged, or placed, on page `page`.
    fn sign_box(&self, page: usize) -> Option<AnyElement> {
        let signing = self.signing.as_ref()?;
        let [x0, y0, x1, y1] = match (&signing.placed, &self.draft) {
            (Some(Placed::Box { page: p, shown, .. }), _) if *p == page => *shown,
            (Some(Placed::Field { page: p, shown, .. }), _) if *p == page => *shown,
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

    /// Asks for other files to sign at the same time.
    fn choose_others(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                this.update(cx, |this, cx| {
                    let current = this.path.clone();
                    if let Some(signing) = this.signing.as_mut() {
                        for path in paths {
                            if Some(&path) != current.as_ref() && !signing.others.contains(&path) {
                                signing.others.push(path);
                            }
                        }
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
    /// writes it, and opens it. With other files to sign too, asks for a
    /// folder instead, and signs them all into it.
    fn sign_now(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.unsaved();
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        let Some(placed) = signing.placed.clone().filter(|_| !signing.busy) else {
            return;
        };
        let certify = signing.certify;
        let source = signing.source;
        let certificate = signing.certificate.clone();
        let found = match (source, &signing.tokens) {
            (Source::Token, Tokens::Found(f)) => f.get(signing.token).cloned(),
            _ => None,
        };
        let missing = match source {
            Source::File if certificate.is_none() => Some("sign-no-certificate"),
            Source::Token if found.is_none() => Some("sign-no-token"),
            _ => None,
        };
        if let Some(missing) = missing {
            signing.error = Some(t(missing, &[]));
            cx.notify();
            return;
        }
        let password = signing.password.read(cx).text().to_owned();
        let value = |input: &Entity<TextInput>| {
            Some(input.read(cx).text().trim().to_owned()).filter(|s| !s.is_empty())
        };
        let (reason, location) = (value(&signing.reason), value(&signing.location));
        let tsa = value(&signing.server).filter(|_| signing.timestamp);
        let others = signing.others.clone();
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
            // The password or PIN is tried before asking where to save.
            let cert = certificate.clone();
            let opened = cx
                .background_executor()
                .spawn(async move { unlock(cert, found, &password) })
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
                if others.is_empty() {
                    let file = cx.prompt_for_new_path(&folder, Some(&suggested));
                    cx.background_executor().spawn(file)
                } else {
                    let folder = cx.prompt_for_paths(gpui::PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some(t("sign-batch-folder", &[]).into()),
                    });
                    cx.background_executor().spawn(async move {
                        folder
                            .await
                            .map(|f| f.map(|f| f.and_then(|f| f.into_iter().next())))
                    })
                }
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
                    let what = What {
                        placed,
                        reason,
                        location,
                        tsa: tsa.clone(),
                        certify,
                    };
                    // This file: into the file chosen, or into the folder.
                    let first = if others.is_empty() {
                        target.clone()
                    } else {
                        free_name(&target, &path)
                    };
                    sign_one(bytes, &what, id.as_ref(), &first)?;
                    let mut done = 1;
                    let mut failed = Vec::new();
                    for other in &others {
                        let signed =
                            std::fs::read(other)
                                .map_err(|e| e.to_string())
                                .and_then(|bytes| {
                                    sign_one(bytes, &what, id.as_ref(), &free_name(&target, other))
                                });
                        match signed {
                            Ok(()) => done += 1,
                            Err(e) => failed.push(format!("{}: {e}", file_name(other))),
                        }
                    }
                    match &certificate {
                        Some(c) if source == Source::File => {
                            write_note("certificate", &c.to_string_lossy());
                            write_note("source", "file");
                        }
                        _ => write_note("source", "token"),
                    }
                    if let Some(url) = &tsa {
                        write_note("timestamp", &format!("on\n{url}\n"));
                    } else {
                        let (_, url) = remembered_timestamp();
                        write_note("timestamp", &format!("off\n{url}\n"));
                    }
                    let notice = if others.is_empty() {
                        None
                    } else if failed.is_empty() {
                        Some(t(
                            "sign-batch-done",
                            &[
                                ("count", &done.to_string()),
                                ("folder", &target.display().to_string()),
                            ],
                        ))
                    } else {
                        Some(t(
                            "sign-batch-partial",
                            &[
                                ("count", &done.to_string()),
                                ("total", &(others.len() + 1).to_string()),
                                ("folder", &target.display().to_string()),
                                ("failed", &failed.join("; ")),
                            ],
                        ))
                    };
                    Ok::<_, String>((first, notice))
                })
                .await;
            this.update(cx, |this, cx| {
                match written {
                    Ok((target, notice)) => {
                        this.signing = None;
                        this.open_with(target, String::new(), cx);
                        this.notice = notice;
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
        let fields = !self.signable.fields.is_empty();
        let (say, free) = match self.signable.certified {
            Some(Allowed::Nothing) => ("sign-certified-locked", false),
            Some(_) if fields => ("sign-place-field", false),
            Some(_) => ("sign-certified-full", false),
            None if fields => ("sign-place-or-field", true),
            None => ("sign-place", true),
        };
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
                    .child(t(say, &[])),
            )
            .when(free, |d| {
                d.child(
                    widgets::outlined_button("sign-hidden", t("sign-hidden", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.place(Placed::Hidden, cx))),
                )
            })
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
                .child(self.source_switch(cx))
                .when(signing.source == Source::File, |d| {
                    d.child(label("sign-certificate")).child(
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
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.choose_certificate(cx)),
                                ),
                            ),
                    )
                })
                .when(signing.source == Source::Token, |d| {
                    d.child(self.token_list(cx))
                })
                .child(field(&signing.password))
                .child(field(&signing.reason))
                .child(field(&signing.location))
                .child(
                    div()
                        .id("sign-timestamp")
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .py(px(space::S1))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(signing) = this.signing.as_mut() {
                                signing.timestamp = !signing.timestamp;
                                cx.notify();
                            }
                        }))
                        .child(check_box(signing.timestamp, th))
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(th.text)
                                .child(t("sign-timestamp", &[])),
                        ),
                )
                .when(signing.timestamp, |d| d.child(field(&signing.server)))
                .when(!self.signable.signed, |d| d.child(self.certify_choice(cx)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(text::SMALL))
                                .text_color(if signing.others.is_empty() {
                                    th.dim
                                } else {
                                    th.text
                                })
                                .child(if signing.others.is_empty() {
                                    t("sign-others-none", &[])
                                } else {
                                    t(
                                        "sign-others",
                                        &[("count", &signing.others.len().to_string())],
                                    )
                                }),
                        )
                        .when(!signing.others.is_empty(), |d| {
                            d.child(
                                widgets::outlined_button(
                                    "sign-others-clear",
                                    t("sign-others-clear", &[]),
                                    &th.katna,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        if let Some(signing) = this.signing.as_mut() {
                                            signing.others.clear();
                                            cx.notify();
                                        }
                                    },
                                )),
                            )
                        })
                        .child(
                            widgets::outlined_button(
                                "sign-others-choose",
                                t("sign-others-choose", &[]),
                                &th.katna,
                            )
                            .tip(t("sign-others-tip", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_others(cx))),
                        ),
                )
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
                                t(
                                    match (busy, signing.others.len()) {
                                        (true, _) => "sign-busy",
                                        (false, 0) => "sign-go",
                                        (false, _) => "sign-go-all",
                                    },
                                    &[("count", &(signing.others.len() + 1).to_string())],
                                ),
                                &th.katna,
                            )
                            .when(busy, |b| b.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.sign_now(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Whether to certify the file, and what it then allows.
    fn certify_choice(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let certify = self.signing.as_ref().and_then(|s| s.certify);
        let set = |allowed: Option<Allowed>| {
            cx.listener(move |this: &mut Self, _: &gpui::ClickEvent, _, cx| {
                if let Some(signing) = this.signing.as_mut() {
                    signing.certify = allowed;
                    cx.notify();
                }
            })
        };
        let choice = |allowed: Allowed| {
            let on = certify == Some(allowed);
            let id = allowed_id(allowed);
            let button = if on {
                widgets::filled_button(id, t(id, &[]), &th.katna)
            } else {
                widgets::outlined_button(id, t(id, &[]), &th.katna)
            };
            button
                .tip(t(&format!("{id}-tip"), &[]), &th.katna)
                .on_click(set(Some(allowed)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(
                div()
                    .id("sign-certify")
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .py(px(space::S1))
                    .cursor_pointer()
                    .on_click(set(match certify {
                        Some(_) => None,
                        None => Some(Allowed::FormFilling),
                    }))
                    .child(check_box(certify.is_some(), th))
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(t("sign-certify", &[])),
                    ),
            )
            .when(certify.is_some(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(space::S1))
                        .child(choice(Allowed::Nothing))
                        .child(choice(Allowed::FormFilling))
                        .child(choice(Allowed::Comments)),
                )
            })
            .into_any_element()
    }

    /// Certificate file or token: two buttons side by side, the one in
    /// use filled.
    fn source_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let on = self.signing.as_ref().map(|s| s.source);
        let button = |id: &'static str, source: Source, label: String| {
            let chosen = on == Some(source);
            div()
                .id(id)
                .flex_1()
                .h(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::FULL))
                .text_size(px(text::SMALL))
                .font_weight(if chosen {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(if chosen { th.text } else { th.dim })
                .when(chosen, |d| d.bg(th.field))
                .hover(move |s| s.bg(th.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.set_source(source, cx)))
                .child(label)
        };
        div()
            .flex()
            .gap(px(space::S1))
            .child(button(
                "sign-from-file",
                Source::File,
                t("sign-from-file", &[]),
            ))
            .child(button(
                "sign-from-token",
                Source::Token,
                t("sign-from-token", &[]),
            ))
            .into_any_element()
    }

    /// The tokens plugged in, to pick one, and a button to look again.
    fn token_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(signing) = self.signing.as_ref() else {
            return div().into_any_element();
        };
        let note = |text: String, colour| {
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(text::SMALL))
                .text_color(colour)
                .child(text)
        };
        let mut list = div().flex().flex_col().gap(px(space::S1));
        match &signing.tokens {
            Tokens::NotLooked | Tokens::Looking => {
                list = list.child(note(t("sign-looking", &[]), th.dim));
            }
            Tokens::Failed(error) => list = list.child(note(error.clone(), th.error)),
            Tokens::Found(found) if found.is_empty() => {
                list = list.child(note(t("sign-no-tokens", &[]), th.dim));
            }
            Tokens::Found(found) => {
                for (i, f) in found.iter().enumerate() {
                    let chosen = i == signing.token;
                    let what = [f.token.manufacturer.as_str(), f.token.model.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ");
                    list = list.child(
                        div()
                            .id(("token", i))
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .rounded(px(radius::SM))
                            .border_1()
                            .border_color(if chosen {
                                gpui::rgba(th.katna.accent).into()
                            } else {
                                th.line
                            })
                            .cursor_pointer()
                            .hover(move |s| s.bg(th.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(signing) = this.signing.as_mut() {
                                    signing.token = i;
                                    signing.focus = true;
                                    cx.notify();
                                }
                            }))
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .text_color(th.text)
                                    .child(f.token.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(text::SMALL))
                                    .text_color(if f.token.locked { th.error } else { th.dim })
                                    .child(if f.token.locked {
                                        t("sign-pin-locked", &[])
                                    } else {
                                        what
                                    }),
                            ),
                    );
                }
            }
        }
        div()
            .flex()
            .items_start()
            .gap(px(space::S2))
            .child(div().flex_1().min_w_0().child(list))
            .child(icon_button(
                "sign-look-again",
                "rotate-cw",
                t("sign-look-again", &[]),
                th,
                cx.listener(|this, _, _, cx| {
                    if let Some(signing) = this.signing.as_mut() {
                        signing.tokens = Tokens::NotLooked;
                    }
                    this.look_for_tokens(cx);
                }),
            ))
            .into_any_element()
    }

    /// The line over the pages of a signed file: whether its signatures
    /// hold, with a button to list them.
    pub(super) fn signatures_line(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rows = self.signatures.as_ref().filter(|r| !r.is_empty())?;
        let th = self.theme;
        let bad = rows.iter().filter(|r| r.problem.is_some()).count();
        let unverified = rows.iter().any(SigRow::unverified);
        let changed = rows.last().is_some_and(|r| r.intact && !r.whole);
        let count = rows.len().to_string();
        let message = match (bad, rows.len(), &rows[0].signer) {
            (0, 1, Some(signer)) if unverified => t("signed-by-unverified", &[("name", signer)]),
            (0, _, _) if unverified => t("signed-unverified", &[("count", &count)]),
            (0, 1, Some(signer)) if !changed => t("signed-by", &[("name", signer)]),
            (0, _, _) if !changed => t("signed-all", &[("count", &count)]),
            (0, _, _) => t("signed-changed", &[("count", &count)]),
            _ if bad == 1 => t("signed-bad-one", &[]),
            _ => t("signed-bad", &[("count", &bad.to_string())]),
        };
        let (icon, colour) = if bad == 0 && unverified {
            ("shield-check", th.katna.warning)
        } else if bad == 0 {
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
                None => {
                    let state = if row.whole {
                        t("signature-intact", &[])
                    } else {
                        t("signature-intact-changed", &[])
                    };
                    let colour = if row.unverified() {
                        th.katna.warning
                    } else {
                        th.valid
                    };
                    ("shield-check", colour, state)
                }
            };
            let (identity, root) = match (&row.vouched, row.problem.is_none()) {
                (Some(Vouched::By(root)), true) => (
                    Some((t("signer-vouched", &[("root", root)]), th.valid)),
                    None,
                ),
                (Some(Vouched::Not { why, root }), true) => {
                    (Some((why.clone(), th.katna.warning)), root.clone())
                }
                _ => (None, None),
            };
            let mut details = Vec::new();
            if let Some(allowed) = row.certifies {
                details.push(t(
                    "signer-certifies",
                    &[(
                        "allowed",
                        &t(&allowed_id(allowed).replace("certify", "allowed"), &[]),
                    )],
                ));
            }
            match &row.revocation {
                Some(Revocation::Good) => details.push(t("signer-not-revoked", &[])),
                Some(Revocation::Unknown { name, why }) => details.push(t(
                    "signer-revocation-unknown",
                    &[("name", name), ("why", why)],
                )),
                _ => {}
            }
            if let Some(time) = &row.time {
                details.push(t("signature-time", &[("time", time)]));
            }
            if let Some(stamp) = &row.stamp {
                let time = stamp.time.clone().unwrap_or_default();
                let by = stamp.authority.clone().unwrap_or_default();
                details.push(if stamp.valid {
                    t(
                        "signature-timestamp",
                        &[("time", &time), ("authority", &by)],
                    )
                } else {
                    t("signature-timestamp-bad", &[])
                });
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
                            .children(identity.map(|(line, colour)| {
                                div()
                                    .text_size(px(text::SMALL))
                                    .text_color(gpui::rgba(colour))
                                    .child(line)
                            }))
                            .children(root.map(|root| {
                                let name = common_name(&root.tbs_certificate.subject);
                                div().flex().child(
                                    widgets::outlined_button(
                                        ("trust-root", i),
                                        t("signer-trust-root", &[("root", &name)]),
                                        &th.katna,
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.trust(&root, cx);
                                        },
                                    )),
                                )
                            }))
                            .children(details.into_iter().map(|d| {
                                div().text_size(px(text::SMALL)).text_color(th.dim).child(d)
                            })),
                    ),
            );
        }
        let folder = trusted_folder()
            .map(|f| f.display().to_string())
            .unwrap_or_default();
        list = list.child(
            div()
                .px(px(space::S4))
                .py(px(space::S3))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("signatures-roots", &[("folder", &folder)])),
        );
        list.into_any_element()
    }
}

impl Viewer {
    /// Trusts `root` from now on, and so every signer it vouches for.
    fn trust(&mut self, root: &Certificate, cx: &mut Context<Self>) {
        if let Err(e) = trust_root(root) {
            self.notice = Some(t("signer-trust-failed", &[("error", &e.to_string())]));
            cx.notify();
            return;
        }
        let name = common_name(&root.tbs_certificate.subject);
        if let Some(rows) = &self.signatures {
            let rows = rows
                .iter()
                .cloned()
                .map(|mut row| {
                    if let Some(Vouched::Not { root: Some(r), .. }) = &row.vouched
                        && **r == *root
                    {
                        row.vouched = Some(Vouched::By(name.clone()));
                    }
                    row
                })
                .collect();
            self.signatures = Some(Arc::new(rows));
        }
        cx.notify();
    }
}

/// The key to sign with, unlocked: from certificate file `file`, or on
/// token `found`, with `secret`, its password or PIN.
fn unlock(file: Option<PathBuf>, found: Option<Found>, secret: &str) -> Result<Key, String> {
    if let Some(found) = found {
        let module = pkcs11::Module::load(&found.module).map_err(|e| e.to_string())?;
        let token = module
            .tokens()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|t| t.serial == found.token.serial && t.label == found.token.label)
            .ok_or_else(|| t("sign-token-gone", &[]))?;
        let ids = module.open(&token, secret).map_err(|e| match e {
            pkcs11::TokenError::Pin => t("sign-wrong-pin", &[]),
            pkcs11::TokenError::Locked => t("sign-pin-locked", &[]),
            pkcs11::TokenError::Gone => t("sign-token-gone", &[]),
            e => e.to_string(),
        })?;
        let id = ids
            .into_iter()
            .next()
            .ok_or_else(|| t("sign-no-key", &[]))?;
        return Ok(Box::new(id));
    }
    let file = file.ok_or_else(|| t("sign-no-certificate", &[]))?;
    let p12 = std::fs::read(&file).map_err(|e| format!("{}: {e}", file_name(&file)))?;
    let id = katna_pdf_sign::p12::open(&p12, secret).map_err(|e| match e {
        katna_pdf_sign::P12Error::Password => t("sign-wrong-password", &[]),
        e => e.to_string(),
    })?;
    Ok(Box::new(id))
}

/// A box ticked or not.
fn check_box(on: bool, th: super::Theme) -> gpui::Div {
    div()
        .size(px(18.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::XS))
        .border_2()
        .border_color(gpui::rgba(if on {
            th.katna.accent
        } else {
            th.katna.text_dim
        }))
        .when(on, |d| {
            d.bg(gpui::rgba(th.katna.accent)).child(widgets::icon(
                "check",
                th.katna.on_accent,
                14.0,
            ))
        })
}

/// What goes into each signature of one signing.
struct What {
    placed: Placed,
    reason: Option<String>,
    location: Option<String>,
    tsa: Option<String>,
    certify: Option<Allowed>,
}

/// Signs PDF `bytes` with `id` as `what` says, and writes the signed
/// copy to `target`. A box goes on the same page, or the last one when
/// the file is shorter.
fn sign_one(
    bytes: Vec<u8>,
    what: &What,
    id: &dyn katna_pdf_sign::Signer,
    target: &Path,
) -> Result<(), String> {
    let doc = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
    let time = std::time::SystemTime::now();
    let lines = || {
        katna_pdf_sign::appearance_lines(
            id,
            &katna_pdf_sign::shown_date(time),
            what.reason.as_deref(),
        )
    };
    let placement = match &what.placed {
        Placed::Hidden => Placement::Invisible,
        Placed::Box { page, rect, .. } => Placement::Visible {
            page: (*page).min(doc.pages().len().saturating_sub(1)),
            rect: *rect,
            lines: lines(),
        },
        // The field of the same name, in each file.
        Placed::Field { name, .. } => Placement::Field {
            id: doc
                .form()
                .into_iter()
                .find(|f| &f.name == name && f.kind == FieldKind::Signature)
                .ok_or_else(|| t("sign-no-field", &[("name", name)]))?
                .id,
            lines: lines(),
        },
    };
    let mut request = SignRequest::new(
        placement,
        SigningInfo {
            name: Some(common_name(&id.certificate().tbs_certificate.subject)),
            reason: what.reason.clone(),
            location: what.location.clone(),
            contact: None,
        },
    );
    request.time = time;
    request.certify = what.certify;
    let server = what.tsa.as_deref().map(katna_pdf_sign::tsa::Server::new);
    let signed = katna_pdf_sign::sign_with(
        &doc,
        &request,
        id,
        katna_pdf_sign::Digest::Sha256,
        server
            .as_ref()
            .map(|s| s as &dyn katna_pdf_sign::tsa::Stamper),
    )
    .map_err(|e| e.to_string())?;
    crate::viewer::save_over(target, &signed).map_err(|e| e.to_string())
}

/// Where `file`'s signed copy goes in `folder`: "name (signed).pdf", or
/// "name (signed) 2.pdf" and on when that is taken.
fn free_name(folder: &Path, file: &Path) -> PathBuf {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = t("sign-file-name", &[("name", &stem)]);
    let mut target = folder.join(&name);
    let mut n = 2;
    while target.exists() {
        let base = name.strip_suffix(".pdf").unwrap_or(&name);
        target = folder.join(format!("{base} {n}.pdf"));
        n += 1;
    }
    target
}

/// A free timestamp server, offered until another is given.
const TSA: &str = "http://timestamp.digicert.com";

/// What signing remembers between runs, as small files in Katna PDF's
/// state folder: the certificate file last used (its path only, never
/// the password), and whether to timestamp and with which server.
fn note(name: &str) -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.state_dir().join("pdf").join(name))
}

fn read_note(name: &str) -> Option<String> {
    std::fs::read_to_string(note(name)?).ok()
}

fn write_note(name: &str, text: &str) {
    if let Some(note) = note(name) {
        if let Some(dir) = note.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(note, text);
    }
}

fn remembered_certificate() -> Option<PathBuf> {
    let path = PathBuf::from(read_note("certificate")?.trim_end_matches('\n'));
    path.is_file().then_some(path)
}

/// Whether to timestamp, and the server: `on` or `off`, then the URL.
fn remembered_timestamp() -> (bool, String) {
    let text = read_note("timestamp").unwrap_or_default();
    let mut lines = text.lines();
    let on = lines.next() == Some("on");
    let url = lines
        .next()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .unwrap_or(TSA);
    (on, url.to_owned())
}

/// The signatures of the file shown, checked.
pub(super) type Rows = Arc<Vec<SigRow>>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A PDF of two pages.
    fn two_pages() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            write!(out, "{} 0 obj\n{o}\nendobj\n", i + 1).unwrap();
        }
        let xref = out.len();
        write!(out, "xref\n0 5\n0000000000 65535 f \n").unwrap();
        for o in offsets {
            writeln!(out, "{o:010} 00000 n ").unwrap();
        }
        write!(
            out,
            "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .unwrap();
        out
    }

    /// Several files signed with one key: each into the folder under a
    /// name of its own, none written over, a box on a page they have.
    #[test]
    fn files_are_signed_into_a_folder() {
        let data = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/katna-pdf-sign/tests/data"
        );
        let p12 = std::fs::read(format!("{data}/rsa-aes.p12")).unwrap();
        let id = katna_pdf_sign::p12::open(&p12, "katna").unwrap();
        let folder = std::env::temp_dir().join(format!("katna-batch-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let pdf = two_pages();
        let what = What {
            placed: Placed::Box {
                page: 5,
                shown: [0.0; 4],
                rect: [20.0, 20.0, 120.0, 60.0],
            },
            reason: Some("Approved".into()),
            location: None,
            tsa: None,
            certify: None,
        };
        let file = Path::new("/somewhere/report.pdf");
        let first = free_name(&folder, file);
        sign_one(pdf.clone(), &what, &id, &first).unwrap();
        let second = free_name(&folder, file);
        assert_ne!(first, second);
        sign_one(pdf.clone(), &what, &id, &second).unwrap();
        for target in [first, second] {
            let doc = katna_pdf_cos::Document::load(std::fs::read(&target).unwrap()).unwrap();
            let checks = katna_pdf_sign::check(&doc, doc.bytes(), &Roots::new());
            assert!(checks[0].intact());
            // Page 6 asked for; the file has 2.
            assert_eq!(checks[0].signature.page, Some(1));
        }
        assert!(sign_one(b"nope".to_vec(), &what, &id, &folder.join("x.pdf")).is_err());

        // Into a field of the file, certifying it: then the field is
        // found as signed, and the file as certified.
        let form = katna_pdf_cos::Document::load(pdf)
            .unwrap()
            .add_signature_field(0, [20.0, 20.0, 120.0, 60.0], Some("Author".into()))
            .unwrap();
        let empty = signable(&katna_pdf_cos::Document::load(form.clone()).unwrap());
        assert_eq!(empty.fields.len(), 1);
        assert!(!empty.signed && empty.certified.is_none());
        let what = What {
            placed: Placed::Field {
                name: "Author".into(),
                page: 0,
                shown: [0.0; 4],
            },
            certify: Some(Allowed::FormFilling),
            ..what
        };
        let target = folder.join("certified.pdf");
        sign_one(form, &what, &id, &target).unwrap();
        let doc = katna_pdf_cos::Document::load(std::fs::read(&target).unwrap()).unwrap();
        let after = signable(&doc);
        assert!(after.fields.is_empty() && after.signed);
        assert_eq!(after.certified, Some(Allowed::FormFilling));
        assert_eq!(check(&doc)[0].certifies, Some(Allowed::FormFilling));
        let _ = std::fs::remove_dir_all(folder);
    }
}
