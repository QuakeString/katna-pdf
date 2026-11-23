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
use crate::float::{self, FloatTip};
use crate::text::t;

mod newid;

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
    /// The form fields it locks, in words.
    pub locks: Option<String>,
    /// It is a document timestamp (PAdES B-LTA), not a signature.
    pub stamp_only: bool,
    /// The bytes of the file it signed, when the file was changed after
    /// it.
    pub revision: Option<usize>,
    /// The signer's certificate, when the signature could be read.
    pub certificate: Option<Arc<Certificate>>,
    /// The signer's certificate and those that issued it, as far as
    /// they were sent with the signature.
    pub certificates: Vec<Arc<Certificate>>,
    /// Who issued whom, from the signer up, and whether that ends in a
    /// certificate that issued itself (else the last was not sent).
    pub chain: (Vec<String>, bool),
    /// How it is made, in a few words: "PAdES B-T · RSA 2048".
    pub kind: String,
    /// What was changed after it, in words; validation data left out.
    pub later: Vec<String>,
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
    /// Whether it has form fields to fill, which signing may lock.
    pub has_form: bool,
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
        has_form: doc.form().iter().any(|f| f.kind != FieldKind::Signature),
    }
}

/// The text id naming what a certification allows.
pub(super) fn allowed_id(allowed: Allowed) -> &'static str {
    match allowed {
        Allowed::Nothing => "certify-nothing",
        Allowed::FormFilling => "certify-forms",
        Allowed::Comments => "certify-comments",
    }
}

pub(super) fn change_id(change: Change) -> &'static str {
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
    pub(super) fn unverified(&self) -> bool {
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
        let mut roots = Roots::standard();
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
                _ if c.intact() && !c.unlocked.is_empty() => {
                    Some(t("signer-unlocked", &[("fields", &c.unlocked.join(", "))]))
                }
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
                signer: match c.signer() {
                    Some(tsa) if c.is_document_timestamp() => {
                        Some(t("document-timestamp-by", &[("authority", &tsa)]))
                    }
                    signer => signer,
                },
                stamp_only: c.is_document_timestamp(),
                time: c.time(),
                reason: c.signature.info.reason.clone(),
                location: c.signature.info.location.clone(),
                page: c.signature.page,
                intact: c.intact(),
                // Validation data added after it changes nothing signed.
                whole: c.coverage == Coverage::Whole
                    || matches!(c.coverage, Coverage::Revision(_))
                        && c.later.iter().all(|l| *l == Change::Dss),
                problem,
                stamp: c.result.as_ref().ok().and_then(|r| r.timestamp.clone()),
                vouched: c.trust.as_ref().map(vouched),
                revocation: c.revocation.clone(),
                certifies: c.signature.certify,
                locks: c.signature.lock.as_ref().map(|lock| match lock {
                    katna_pdf_cos::Lock::All => t("signer-locks-all", &[]),
                    katna_pdf_cos::Lock::Include(names) => {
                        t("signer-locks", &[("fields", &names.join(", "))])
                    }
                    katna_pdf_cos::Lock::Exclude(names) => {
                        t("signer-locks-but", &[("fields", &names.join(", "))])
                    }
                }),
                revision: match c.coverage {
                    Coverage::Revision(end) if c.later.iter().any(|l| *l != Change::Dss) => {
                        Some(end)
                    }
                    _ => None,
                },
                certificate: c.result.as_ref().ok().map(|r| Arc::new(r.signer.clone())),
                certificates: c
                    .result
                    .as_ref()
                    .map(|r| {
                        katna_pdf_sign::certview::chain_certificates(&r.signer, &r.certificates)
                            .into_iter()
                            .map(Arc::new)
                            .collect()
                    })
                    .unwrap_or_default(),
                chain: c
                    .result
                    .as_ref()
                    .ok()
                    .map(|r| katna_pdf_sign::chain_names(&r.signer, &r.certificates))
                    .unwrap_or_default(),
                kind: kind_text(&c),
                later: c
                    .later
                    .iter()
                    .filter(|l| **l != Change::Dss)
                    .map(|l| t(change_id(*l), &[]))
                    .collect(),
            }
        })
        .collect()
}

/// How a signature is made, in a few words: its format and level, then
/// its key.
fn kind_text(c: &katna_pdf_sign::SignatureCheck) -> String {
    let mut parts = Vec::new();
    if c.is_document_timestamp() {
        parts.push(t("signature-kind-stamp", &[]));
    } else if c.signature.sub_filter.starts_with("ETSI.CAdES") {
        let stamped = c
            .result
            .as_ref()
            .is_ok_and(|r| r.timestamp.as_ref().is_some_and(|s| s.valid));
        let level = if c.later.contains(&Change::Dss) {
            "B-LT"
        } else if stamped {
            "B-T"
        } else {
            "B-B"
        };
        parts.push(t("signature-kind-pades", &[("level", level)]));
    } else {
        parts.push(t("signature-kind-pkcs7", &[]));
    }
    if let Ok(r) = &c.result {
        parts.push(katna_pdf_sign::key_text(&r.signer));
    }
    parts.join(" · ")
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
    /// Its signing certificates, likeliest first, as read without the
    /// PIN; empty when the token keeps them private.
    certificates: Vec<Arc<Certificate>>,
    /// The one to sign with.
    chosen: usize,
    /// Whether each of them chains to a root trusted here.
    trusted: Vec<bool>,
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
    /// Why no token shows, a line each with whether that step is fine,
    /// once looked into.
    diagnosis: Option<Vec<(bool, String)>>,
    password: Entity<TextInput>,
    reason: Entity<TextInput>,
    location: Entity<TextInput>,
    /// Whether to timestamp the signature, and the server to ask.
    timestamp: bool,
    /// Whether to keep the validation data in the file (PAdES B-LT).
    long_term: bool,
    /// A picture to show in the box, such as a scanned handwritten
    /// signature.
    picture: Option<PathBuf>,
    server: Entity<TextInput>,
    /// Other files to sign at the same time, with the key unlocked once.
    others: Vec<PathBuf>,
    /// Certify the file, allowing these changes after.
    certify: Option<Allowed>,
    /// Lock the form's fields: their values may not change after.
    lock: bool,
    error: Option<String>,
    /// The token refused the PIN: asked again in a card of its own.
    pin: Option<PinTrouble>,
    busy: bool,
    /// Put the keys in the password field on the next frame.
    focus: bool,
    /// Making a digital ID of one's own, when the form is open.
    new_id: Option<newid::IdForm>,
    /// Put the keys in the form's first field on the next frame.
    focus_id: bool,
    _inputs: Vec<Subscription>,
}

/// What a file's signatures come to, as the library lists it.
pub(crate) enum Verdict {
    Unsigned,
    /// This many signatures, intact and vouched for.
    Good(usize),
    /// Intact, but who signed is not vouched for: why.
    Unverified(String),
    /// A signature is broken, or the file changed after it: what.
    Bad(String),
    /// The file could not be read: why.
    Unreadable(String),
}

/// A folder as people read it, the home folder as ~.
fn home_short(folder: &Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if folder.starts_with(&home) && !home.as_os_str().is_empty() => {
            let rest = folder.strip_prefix(&home).unwrap_or(folder);
            if rest.as_os_str().is_empty() {
                "~".into()
            } else {
                format!("~/{}", rest.display())
            }
        }
        _ => folder.display().to_string(),
    }
}

/// The signatures of the file at `path`, checked offline.
pub(crate) fn verdict(path: &Path) -> Verdict {
    let doc = match std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| katna_pdf_cos::Document::load(b).map_err(|e| e.to_string()))
    {
        Ok(doc) => doc,
        Err(e) => return Verdict::Unreadable(e),
    };
    let rows = check(&doc);
    if rows.is_empty() {
        return Verdict::Unsigned;
    }
    if let Some(bad) = rows.iter().find(|r| !r.intact || r.problem.is_some()) {
        return Verdict::Bad(
            bad.problem
                .clone()
                .unwrap_or_else(|| t("library-check-broken", &[("name", &bad.name)])),
        );
    }
    match rows.iter().find_map(|r| match &r.vouched {
        Some(Vouched::Not { why, .. }) => Some(why.clone()),
        _ => None,
    }) {
        Some(why) => Verdict::Unverified(why),
        None => Verdict::Good(rows.len()),
    }
}

impl Viewer {
    /// Starts signing this file, `others` to be signed with it, now or
    /// once the file is open.
    pub fn sign_with(&mut self, others: Vec<PathBuf>, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Ready(_)) {
            self.sign_after = Some(others);
            return;
        }
        if self.signing.is_none() {
            self.toggle_signing(cx);
        }
        let current = self.path.clone();
        if let Some(signing) = self.signing.as_mut() {
            signing.others = others
                .into_iter()
                .filter(|p| Some(p) != current.as_ref())
                .collect();
        }
        cx.notify();
    }

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
        self.leave_text_edit(cx);
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
            diagnosis: None,
            password,
            reason,
            location,
            timestamp,
            long_term: read_note("long-term").as_deref().map(str::trim) != Some("off"),
            picture: remembered_picture(),
            server,
            others: Vec::new(),
            certify: None,
            lock: false,
            error: None,
            pin: None,
            busy: false,
            focus: false,
            new_id: None,
            focus_id: false,
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
                                let certificates: Vec<Arc<Certificate>> = module
                                    .certificates(&token)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(Arc::new)
                                    .collect();
                                let roots = roots();
                                let now = std::time::SystemTime::now();
                                let trusted = certificates
                                    .iter()
                                    .map(|c| {
                                        katna_pdf_sign::trust::chain(c, &[], &roots, now).trusted()
                                    })
                                    .collect();
                                found.push(Found {
                                    module: path.clone(),
                                    token,
                                    certificates,
                                    chosen: 0,
                                    trusted,
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

    /// Asks for a picture to show in the box.
    fn choose_picture(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("sign-picture-choose", &[]).into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| {
                    if let Some(signing) = this.signing.as_mut() {
                        signing.picture = Some(file);
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
        // A password-protected file is signed with the password it was
        // opened with, and stays protected.
        let key = self.key.clone();
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
        let long_term = signing.long_term;
        let lock = signing.lock && self.signable.has_form;
        // A picture only shows in a box.
        let picture_file = signing
            .picture
            .clone()
            .filter(|_| !matches!(placed, Placed::Hidden));
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
                            signing.pin = None;
                            cx.notify();
                        }
                    })
                    .ok();
                };
            let id = match opened {
                Ok(id) => id,
                Err(Refused::Other(error)) => return stop(this, Some(error), cx),
                Err(refused) => {
                    this.update(cx, |this, cx| {
                        if let Some(signing) = this.signing.as_mut() {
                            signing.busy = false;
                            signing.error = None;
                            signing.focus = true;
                            signing.pin = Some(match refused {
                                Refused::Pin(words) => PinTrouble::Wrong(words),
                                _ => PinTrouble::Locked,
                            });
                            signing
                                .password
                                .update(cx, |input, cx| input.set_text("", cx));
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
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
                    let picture = match &picture_file {
                        None => None,
                        Some(file) => Some(
                            std::fs::read(file)
                                .map_err(|e| e.to_string())
                                .and_then(|b| katna_pdf_sign::picture(&b))
                                .map_err(|e| format!("{}: {e}", file_name(file)))?,
                        ),
                    };
                    let what = What {
                        placed,
                        reason,
                        location,
                        tsa: tsa.clone(),
                        certify,
                        long_term,
                        picture,
                        lock,
                    };
                    // This file: into the file chosen, or into the folder.
                    let first = if others.is_empty() {
                        target.clone()
                    } else {
                        free_name(&target, &path)
                    };
                    sign_one(bytes, key.as_deref(), &what, id.as_ref(), &first)?;
                    let mut done = 1;
                    let mut failed = Vec::new();
                    for other in &others {
                        let signed =
                            std::fs::read(other)
                                .map_err(|e| e.to_string())
                                .and_then(|bytes| {
                                    sign_one(
                                        bytes,
                                        None,
                                        &what,
                                        id.as_ref(),
                                        &free_name(&target, other),
                                    )
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
                    write_note("long-term", if long_term { "on" } else { "off" });
                    if let Some(file) = &picture_file {
                        write_note("picture", &file.to_string_lossy());
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
                        Some(super::said(
                            "sign-batch-done",
                            &[
                                ("count", &done.to_string()),
                                ("folder", &target.display().to_string()),
                            ],
                        ))
                    } else {
                        Some(super::said(
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
        self.focus_id_form(window, cx);
        if let Some(signing) = self.signing.as_mut()
            && std::mem::take(&mut signing.focus)
        {
            let input = signing.password.clone();
            window.focus(&input.focus_handle(cx), cx);
        }
    }

    /// What to do while placing the signature, over the pages' top
    /// right corner, so the pages do not move as it comes and goes.
    /// The Sign mode's tray (G01): signing, signing without a box where
    /// the file allows, and the signatures already there; while the
    /// signature has no place yet, a hint over it says how to give one.
    pub(super) fn sign_tray(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let signing = self.signing.as_ref()?;
        let th = self.theme;
        let fields = !self.signable.fields.is_empty();
        let (say, free) = match self.signable.certified {
            Some(Allowed::Nothing) => ("sign-certified-locked", false),
            Some(_) if fields => ("sign-place-field", false),
            Some(_) => ("sign-certified-full", false),
            None if fields => ("sign-place-or-field", true),
            None => ("sign-place", true),
        };
        let placing = signing.placed.is_none();
        let tray = float::tray("sign-tray", &th)
            .child(
                float::dk(
                    "sign-place",
                    "sign",
                    Some(t("mode-sign", &[]).into()),
                    t(say, &[]),
                    float::Pressed::Tool,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    // Back to choosing where it goes.
                    if let Some(s) = this.signing.as_mut() {
                        s.placed = None;
                    }
                    cx.notify();
                })),
            )
            .when(free && placing, |d| {
                d.child(
                    float::dk(
                        "sign-hidden",
                        "eyeoff",
                        None,
                        t("sign-hidden", &[]),
                        float::Pressed::No,
                        &th,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.place(Placed::Hidden, cx))),
                )
            })
            .child(float::vsep(&th))
            .child(
                float::dk(
                    "signatures-show",
                    "shield",
                    None,
                    t("signatures-show", &[]),
                    if self.pane && self.tab == PaneTab::Signatures {
                        float::Pressed::Tool
                    } else {
                        float::Pressed::No
                    },
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.pane = true;
                    this.tab = PaneTab::Signatures;
                    cx.notify();
                })),
            )
            .child(
                float::dk(
                    "sign-cancel",
                    "x",
                    None,
                    t("sign-cancel", &[]),
                    float::Pressed::No,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle_signing(cx))),
            );
        let hint = placing.then(|| {
            div()
                .max_w(px(560.0))
                .px(px(space::S3))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(th.inverse)
                .text_color(th.on_inverse)
                .text_size(px(text::CAPTION))
                .child(t(say, &[]))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(space::S3))
                .children(hint)
                .child(tray)
                .into_any_element(),
        )
    }

    /// The card over the pages while signing: where to put the
    /// signature, then the certificate and its password.
    /// The certificate file to sign with, and how to choose another or
    /// make one's own.
    fn certificate_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(signing) = self.signing.as_ref() else {
            return div().into_any_element();
        };
        let th = self.theme;
        let certificate = signing
            .certificate
            .as_deref()
            .map_or_else(|| t("sign-certificate-none", &[]), file_name);
        let folder = signing
            .certificate
            .as_deref()
            .and_then(Path::parent)
            .map(home_short);
        let row = div()
            .flex()
            .items_center()
            .gap(px(space::S3))
            .px(px(space::S3))
            .py(px(space::S3))
            .rounded(px(radius::SM))
            .border_1()
            .border_color(th.line)
            .child(float::icon("file", 18.0, th.text2))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(text::BODY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(if signing.certificate.is_some() {
                                th.text
                            } else {
                                th.dim
                            })
                            .child(certificate),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(folder.unwrap_or_else(|| t("sign-certificate", &[]))),
                    ),
            )
            .child(
                float::button(
                    "sign-choose",
                    t(
                        if signing.certificate.is_some() {
                            "sign-change"
                        } else {
                            "sign-choose"
                        },
                        &[],
                    ),
                    false,
                    &th,
                )
                .ftip(t("sign-choose-tip", &[]), &th)
                .on_click(cx.listener(|this, _, _, cx| this.choose_certificate(cx))),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .child(row)
            .child(
                div()
                    .id("newid-open")
                    .self_start()
                    .text_size(px(text::SMALL))
                    .text_color(th.accent)
                    .cursor_pointer()
                    .hover(|d| d.underline())
                    .child(t("newid-open", &[]))
                    .on_click(cx.listener(|this, _, _, cx| this.open_id_form(cx))),
            )
            .into_any_element()
    }

    pub(super) fn signing_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let signing = self.signing.as_ref()?;
        signing.placed.as_ref()?;
        let th = self.theme;
        if let Some(trouble) = signing.pin.clone() {
            return Some(self.pin_card(trouble, cx));
        }
        let field = |input: &Entity<TextInput>| {
            div()
                .h(px(34.0))
                .flex()
                .items_center()
                .px(px(space::S3))
                .rounded(px(6.0))
                .border_1()
                .border_color(th.line)
                .bg(th.card)
                .text_size(px(text::BODY))
                .text_color(th.text)
                .child(input.clone())
        };
        let label = |id: &str| {
            div()
                .text_size(px(text::CAPTION))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let labelled = |id: &str, input: &Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(label(id))
                .child(field(input))
        };
        // A switch on the right of its words, as Settings' rows.
        let switch_row = |id: &'static str, words: &str, on: bool| {
            div()
                .id(id)
                .min_h(px(36.0))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(space::S3))
                .cursor_pointer()
                .child(
                    div()
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(t(words, &[])),
                )
                .child(float::switch((id, 0usize), on, &th))
        };
        let secret_label = if signing.source == Source::Token {
            "sign-pin-label"
        } else {
            "sign-password-label"
        };
        let busy = signing.busy;
        let card = div()
            .id("signing-card")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(space::S5))
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .text_size(px(text::SUBTITLE))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(th.text)
                    .child(t("sign-title", &[])),
            )
            .child(self.source_switch(cx))
            .when(signing.source == Source::File, |d| match &signing.new_id {
                Some(form) => d.child(self.id_form(form, cx)),
                None => d.child(self.certificate_row(cx)),
            })
            .when(signing.source == Source::Token, |d| {
                d.child(self.token_list(cx))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label(secret_label))
                    .child(
                        field(&signing.password)
                            .gap(px(space::S2))
                            .border_color(th.accent)
                            .child(float::icon("key", 15.0, th.dim))
                            .flex_row_reverse()
                            .justify_end(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S3))
                    .child(labelled("sign-reason-label", &signing.reason))
                    .child(labelled("sign-place-label", &signing.location)),
            )
            .when(!matches!(signing.placed, Some(Placed::Hidden)), |d| {
                d.child(label("sign-shown-label"))
                    .child(self.picture_row(cx))
            })
            .child(
                div()
                    .min_h(px(36.0))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("sign-timestamp", &[])),
                    )
                    .when(signing.timestamp, |d| {
                        d.child(field(&signing.server).w(px(220.0)).h(px(30.0)))
                    })
                    .child(
                        float::switch(("sign-timestamp", 0usize), signing.timestamp, &th).on_click(
                            cx.listener(|this, _, _, cx| {
                                if let Some(signing) = this.signing.as_mut() {
                                    signing.timestamp = !signing.timestamp;
                                    cx.notify();
                                }
                            }),
                        ),
                    ),
            )
            .child(
                switch_row("sign-long-term", "sign-long-term", signing.long_term)
                    .ftip(t("sign-long-term-tip", &[]), &th)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(signing) = this.signing.as_mut() {
                            signing.long_term = !signing.long_term;
                            cx.notify();
                        }
                    })),
            )
            .when(self.signable.has_form, |d| {
                d.child(
                    switch_row("sign-lock", "sign-lock", signing.lock)
                        .ftip(t("sign-lock-tip", &[]), &th)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(signing) = this.signing.as_mut() {
                                signing.lock = !signing.lock;
                                cx.notify();
                            }
                        })),
                )
            })
            .when(!self.signable.signed && signing.certify.is_some(), |d| {
                d.child(self.certify_choice(cx))
            })
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
                            float::button(
                                "sign-others-clear",
                                t("sign-others-clear", &[]),
                                false,
                                &th,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(signing) = this.signing.as_mut() {
                                    signing.others.clear();
                                    cx.notify();
                                }
                            })),
                        )
                    })
                    .child(
                        float::button(
                            "sign-others-choose",
                            t("sign-others-choose", &[]),
                            false,
                            &th,
                        )
                        .ftip(t("sign-others-tip", &[]), &th)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_others(cx))),
                    ),
            )
            .children(signing.error.clone().map(|error| {
                div()
                    .p(px(space::S3))
                    .rounded(px(7.0))
                    .bg(th.error_soft)
                    .text_size(px(text::SMALL))
                    .text_color(th.error)
                    .child(error)
            }));
        let certifying = signing.certify.is_some();
        let footer = div()
            .px(px(space::S5))
            .py(px(space::S4))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .border_t_1()
            .border_color(th.line_soft)
            .when(!self.signable.signed, |d| {
                d.child(
                    float::button(
                        "sign-certify-instead",
                        t(
                            if certifying {
                                "sign-plain-instead"
                            } else {
                                "sign-certify-instead"
                            },
                            &[],
                        ),
                        false,
                        &th,
                    )
                    .border_color(gpui::transparent_black())
                    .bg(gpui::transparent_black())
                    .ftip(t("sign-certify", &[]), &th)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(signing) = this.signing.as_mut() {
                            signing.certify = if certifying {
                                None
                            } else {
                                Some(Allowed::FormFilling)
                            };
                            cx.notify();
                        }
                    })),
                )
            })
            .child(div().flex_1())
            .child(
                float::button("sign-back", t("sign-cancel", &[]), false, &th)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_signing(cx))),
            )
            .child(
                float::button(
                    "sign-go",
                    t(
                        match (busy, signing.others.len()) {
                            (true, _) => "sign-busy",
                            (false, 0) => "sign-go",
                            (false, _) => "sign-go-all",
                        },
                        &[("count", &(signing.others.len() + 1).to_string())],
                    ),
                    true,
                    &th,
                )
                .child(float::icon("sign", 14.0, th.on_accent))
                .flex_row_reverse()
                .gap(px(space::S2))
                .when(busy, |b| b.opacity(0.6))
                .on_click(cx.listener(|this, _, _, cx| this.sign_now(cx))),
            );
        // Over the page, the page dimmed behind it (G02).
        Some(
            div()
                .id("signing-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(if th.dark { 0.45 } else { 0.25 }))
                .occlude()
                .child(
                    div()
                        .id("signing-dialog")
                        .w(px(540.0))
                        .max_h(px(640.0))
                        .flex()
                        .flex_col()
                        .rounded(px(12.0))
                        .bg(th.card)
                        .shadow(th.lifted())
                        .when(th.dark, |d| d.border_1().border_color(th.line))
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(card)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    /// The token refused the PIN (G05): asked again in a small card, the
    /// tries left told under it; or, once the token is locked, what to
    /// do about it.
    fn pin_card(&self, trouble: PinTrouble, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let busy = self.signing.as_ref().is_some_and(|s| s.busy);
        let back = cx.listener(|this, _, _, cx| {
            if let Some(signing) = this.signing.as_mut() {
                signing.pin = None;
                signing.focus = true;
                cx.notify();
            }
        });
        let (body, footer) = match trouble {
            PinTrouble::Wrong(words) => {
                let input = self.signing.as_ref().map(|s| s.password.clone());
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .text_size(px(text::SUBTITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("sign-pin-label", &[])),
                    )
                    .child(
                        div()
                            .h(px(34.0))
                            .flex()
                            .items_center()
                            .gap(px(space::S3))
                            .px(px(space::S3))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(th.error)
                            .bg(th.card)
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(float::icon("key", 16.0, th.dim))
                            .child(div().flex_1().min_w_0().children(input)),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.error)
                            .child(words),
                    );
                let footer = div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        float::button("pin-cancel", t("sign-cancel", &[]), false, &th)
                            .on_click(back),
                    )
                    .child(
                        float::button(
                            "pin-sign",
                            t(if busy { "sign-busy" } else { "sign-pin-again" }, &[]),
                            true,
                            &th,
                        )
                        .when(busy, |b| b.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.sign_now(cx))),
                    );
                (body, footer)
            }
            PinTrouble::Locked => {
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S3))
                            .child(float::icon("lock", 18.0, th.error))
                            .child(
                                div()
                                    .text_size(px(text::SUBTITLE))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(t("sign-locked-title", &[])),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text2)
                            .child(t("sign-locked-detail", &[])),
                    );
                let footer = div().flex().justify_end().child(
                    float::button("pin-close", t("signature-close", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_signing(cx))),
                );
                (body, footer)
            }
        };
        div()
            .id("signing-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(if th.dark { 0.45 } else { 0.25 }))
            .occlude()
            .child(
                div()
                    .id("pin-dialog")
                    .w(px(400.0))
                    .flex()
                    .flex_col()
                    .rounded(px(12.0))
                    .bg(th.card)
                    .shadow(th.lifted())
                    .when(th.dark, |d| d.border_1().border_color(th.line))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().p(px(space::S5)).child(body))
                    .child(
                        div()
                            .px(px(space::S5))
                            .py(px(space::S4))
                            .border_t_1()
                            .border_color(th.line_soft)
                            .child(footer),
                    ),
            )
            .into_any_element()
    }

    /// How the box will look (G02): the picture, if any, beside the
    /// words the signature writes, and the button to pick a picture.
    fn picture_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(signing) = self.signing.as_ref() else {
            return div().into_any_element();
        };
        let picture = signing.picture.clone();
        // Who signs, when the token tells; a file tells once unlocked.
        let who = match &signing.tokens {
            Tokens::Found(found) if signing.source == Source::Token => found
                .get(signing.token)
                .and_then(|f| f.certificates.get(f.chosen))
                .map(|c| common_name(&c.tbs_certificate.subject)),
            _ => None,
        };
        let by = match who {
            Some(name) => t("sign-preview-by", &[("name", &name)]),
            None => t("sign-preview-by-you", &[]),
        };
        let place = signing.location.read(cx).text().trim().to_owned();
        let mut when = jiff::Zoned::now().strftime("%d.%m.%Y %H:%M %Z").to_string();
        if !place.is_empty() {
            when = format!("{when} · {place}");
        }
        let mark = match &picture {
            Some(file) => div()
                .w(px(72.0))
                .h(px(40.0))
                .flex_none()
                .child(
                    gpui::img(file.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            None => div()
                .w(px(72.0))
                .h(px(40.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(float::icon("sign", 30.0, th.text2))
                .into_any_element(),
        };
        div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .rounded(px(radius::SM))
                    .border_1()
                    .border_color(th.line)
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(text::SMALL))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(by),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(when),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(
                        float::button(
                            "sign-picture-choose",
                            t("sign-picture-button", &[]),
                            false,
                            &th,
                        )
                        .tip(t("sign-picture-tip", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.choose_picture(cx))),
                    )
                    .when(picture.is_some(), |d| {
                        d.child(
                            float::button(
                                "sign-picture-clear",
                                t("sign-picture-clear", &[]),
                                false,
                                &th,
                            )
                            .border_color(gpui::transparent_black())
                            .bg(gpui::transparent_black())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(signing) = this.signing.as_mut() {
                                    signing.picture = None;
                                    write_note("picture", "");
                                    cx.notify();
                                }
                            })),
                        )
                    }),
            )
            .into_any_element()
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
                float::button(id, t(id, &[]), true, &th)
            } else {
                float::button(id, t(id, &[]), false, &th)
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
            float::chip(id, label, on == Some(source), &th)
                .on_click(cx.listener(move |this, _, _, cx| this.set_source(source, cx)))
        };
        div()
            .flex()
            .gap(px(space::S1))
            .child(button(
                "sign-from-token",
                Source::Token,
                t("sign-from-token", &[]),
            ))
            .child(button(
                "sign-from-file",
                Source::File,
                t("sign-from-file", &[]),
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
            Tokens::Failed(error) => {
                list = list
                    .child(note(error.clone(), th.error))
                    .child(self.diagnosis(cx));
            }
            Tokens::Found(found) if found.is_empty() => {
                list = list
                    .child(note(t("sign-no-tokens", &[]), th.dim))
                    .child(self.diagnosis(cx));
            }
            Tokens::Found(found) => {
                // A card for each certificate a token holds (G02): the
                // token and who, then the issuer and until when.
                for (i, f) in found.iter().enumerate() {
                    let what = [f.token.manufacturer.as_str(), f.token.model.as_str()]
                        .iter()
                        .filter(|s| !s.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let cards: Vec<(Option<usize>, String, String, bool)> = if f
                        .certificates
                        .is_empty()
                        || f.token.locked
                    {
                        vec![(
                            None,
                            f.token.label.clone(),
                            if f.token.locked {
                                t("sign-pin-locked", &[])
                            } else {
                                what
                            },
                            false,
                        )]
                    } else {
                        f.certificates
                            .iter()
                            .enumerate()
                            .map(|(c, cert)| {
                                let tbs = &cert.tbs_certificate;
                                // The day is enough.
                                let until = katna_pdf_sign::shown_date(
                                    tbs.validity.not_after.to_system_time(),
                                )
                                .split(' ')
                                .next()
                                .unwrap_or_default()
                                .to_owned();
                                (
                                    Some(c),
                                    t(
                                        "sign-token-who",
                                        &[
                                            ("token", &f.token.label),
                                            ("name", &common_name(&tbs.subject)),
                                        ],
                                    ),
                                    t(
                                        "sign-token-issuer",
                                        &[("issuer", &common_name(&tbs.issuer)), ("until", &until)],
                                    ),
                                    f.trusted.get(c).copied().unwrap_or(false),
                                )
                            })
                            .collect()
                    };
                    for (c, title, detail, trusted) in cards {
                        let picked = i == signing.token && c.is_none_or(|c| c == f.chosen);
                        let locked = f.token.locked;
                        let valid: gpui::Hsla = gpui::rgba(th.valid).into();
                        list = list.child(
                            div()
                                .id(("token", i * 64 + c.unwrap_or(0)))
                                .flex()
                                .items_center()
                                .gap(px(space::S4))
                                .px(px(space::S4))
                                .py(px(space::S3))
                                .rounded(px(radius::SM))
                                .border_1()
                                .border_color(if picked { th.accent } else { th.line })
                                .cursor_pointer()
                                .hover(move |s| s.bg(th.hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(signing) = this.signing.as_mut() {
                                        signing.token = i;
                                        if let (Some(c), Tokens::Found(found)) =
                                            (c, &mut signing.tokens)
                                            && let Some(f) = found.get_mut(i)
                                        {
                                            f.chosen = c;
                                        }
                                        signing.focus = true;
                                        cx.notify();
                                    }
                                }))
                                .child(float::icon("usb", 18.0, th.text2))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(text::BODY))
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(th.text)
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(text::CAPTION))
                                                .text_color(if locked { th.error } else { th.dim })
                                                .child(detail),
                                        ),
                                )
                                .when(trusted, |d| {
                                    d.child(
                                        div()
                                            .px(px(space::S3))
                                            .py(px(2.0))
                                            .rounded(px(radius::XS))
                                            .bg(valid.opacity(0.14))
                                            .text_size(px(text::CAPTION))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(valid)
                                            .child(t("sign-token-trusted", &[])),
                                    )
                                }),
                        );
                    }
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
                        signing.diagnosis = None;
                    }
                    this.look_for_tokens(cx);
                }),
            ))
            .into_any_element()
    }

    /// Why no token shows: a button to look into it, then what was
    /// found, step by step.
    fn diagnosis(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(lines) = self.signing.as_ref().and_then(|s| s.diagnosis.clone()) else {
            return div()
                .flex()
                .child(
                    float::button("sign-diagnose", t("sign-diagnose", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.diagnose(cx))),
                )
                .into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .children(lines.into_iter().map(|(ok, line)| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .child(widgets::icon(
                        if ok { "check" } else { "close" },
                        if ok { th.valid } else { th.katna.error },
                        14.0,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(line),
                    )
            }))
            .into_any_element()
    }

    /// Looks into why no token shows, off the UI thread.
    fn diagnose(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let findings = cx
                .background_executor()
                .spawn(async move { pkcs11::diagnose() })
                .await;
            let lines = findings
                .into_iter()
                .flat_map(|f| match f {
                    pkcs11::Finding::Device { name, id } => {
                        vec![(true, t("diagnose-device", &[("name", &name), ("id", &id)]))]
                    }
                    pkcs11::Finding::NoDevice => vec![(false, t("diagnose-no-device", &[]))],
                    pkcs11::Finding::Service { running: true } => {
                        vec![(true, t("diagnose-service", &[]))]
                    }
                    pkcs11::Finding::Service { running: false } => {
                        vec![(false, t("diagnose-no-service", &[]))]
                    }
                    pkcs11::Finding::NoModule => vec![(false, t("sign-no-driver", &[]))],
                    pkcs11::Finding::Module { path, tokens } => {
                        let driver = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        match tokens {
                            Ok(t_) if t_.is_empty() => {
                                vec![(false, t("diagnose-driver-empty", &[("driver", &driver)]))]
                            }
                            Ok(t_) => t_
                                .into_iter()
                                .map(|(label, certs)| {
                                    (
                                        certs > 0,
                                        t(
                                            if certs == 1 {
                                                "diagnose-driver-token-one"
                                            } else {
                                                "diagnose-driver-token"
                                            },
                                            &[
                                                ("driver", &driver),
                                                ("token", &label),
                                                ("count", &certs.to_string()),
                                            ],
                                        ),
                                    )
                                })
                                .collect(),
                            Err(e) => vec![(
                                false,
                                t(
                                    "diagnose-driver-failed",
                                    &[("driver", &driver), ("error", &e)],
                                ),
                            )],
                        }
                    }
                })
                .collect();
            this.update(cx, |this, cx| {
                if let Some(signing) = this.signing.as_mut() {
                    signing.diagnosis = Some(lines);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Viewer {
    /// Opens the file as it was when it was signed by `signer`, its
    /// first `end` bytes, in a tab of its own.
    pub(super) fn show_signed_version(&mut self, end: usize, signer: &str, cx: &mut Context<Self>) {
        match self.signed_version(end, signer) {
            Some(Ok(file)) => cx.emit(super::ViewerEvent::Dropped(vec![file])),
            Some(Err(e)) => {
                self.notice = Some(super::Notice::error(e.to_string()));
                cx.notify();
            }
            None => {}
        }
    }

    /// Compares the file with what `signer` signed, its first `end`
    /// bytes.
    pub(super) fn compare_signed_version(
        &mut self,
        end: usize,
        signer: &str,
        cx: &mut Context<Self>,
    ) {
        match self.signed_version(end, signer) {
            Some(Ok(file)) => self.compare_with(file, cx),
            Some(Err(e)) => {
                self.notice = Some(super::Notice::error(e.to_string()));
                cx.notify();
            }
            None => {}
        }
    }

    /// The file as `signer` signed it, its first `end` bytes, written to
    /// a file of its own.
    fn signed_version(&self, end: usize, signer: &str) -> Option<std::io::Result<PathBuf>> {
        let path = self.path.clone()?;
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = t(
            "signature-signed-version-file",
            &[("name", &stem), ("signer", signer)],
        );
        let shown = std::fs::read(&path).and_then(|bytes| {
            let bytes = bytes
                .get(..end)
                .ok_or_else(|| std::io::Error::other("the file is shorter than it was"))?;
            let folder = std::env::temp_dir().join("katna-pdf");
            std::fs::create_dir_all(&folder)?;
            let file = folder.join(name.replace(['/', '\\'], "-"));
            std::fs::write(&file, bytes)?;
            Ok(file)
        });
        Some(shown)
    }

    /// Asks where, then writes what was found checking the signatures
    /// as a PDF, to keep or send on.
    pub(super) fn save_report(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(rows)) = (self.path.clone(), self.signatures.clone()) else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let suggested = t("signatures-report-file", &[("name", &stem)]);
        let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
        let lines = report_lines(&super::file_name(&path), &rows);
        let title = t("signatures-report-title", &[]);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let pdf = katna_pdf_cos::text_pdf(&title, &lines);
                    std::fs::write(&target, pdf).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => super::said(
                        "signatures-report-saved",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => super::said("signatures-report-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Trusts `root` from now on, and so every signer it vouches for.
    pub(super) fn trust(&mut self, root: &Certificate, cx: &mut Context<Self>) {
        if let Err(e) = trust_root(root) {
            self.notice = Some(super::said(
                "signer-trust-failed",
                &[("error", &e.to_string())],
            ));
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

/// What a signature's state is, in a line.
pub(super) fn state(row: &SigRow) -> String {
    match &row.problem {
        Some(problem) => t("signature-bad", &[("problem", problem)]),
        None if row.whole => t("signature-intact", &[]),
        None => t("signature-intact-changed", &[]),
    }
}

/// Who vouches for a signer, if the signature holds.
fn identity(row: &SigRow) -> Option<String> {
    match (&row.vouched, row.problem.is_none()) {
        (Some(Vouched::By(root)), true) => Some(t("signer-vouched", &[("root", root)])),
        (Some(Vouched::Not { why, .. }), true) => Some(why.clone()),
        _ => None,
    }
}

/// The rest of what is known of a signature, a line each.
fn details(row: &SigRow) -> Vec<String> {
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
    if let Some(locks) = &row.locks {
        details.push(locks.clone());
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
    details
}

/// The signatures of file `name`, as lines for a report: each signer
/// and state, then what is known of it, indented.
fn report_lines(name: &str, rows: &[SigRow]) -> Vec<String> {
    let mut lines = vec![
        t("report-file", &[("name", name)]),
        format!(
            "  {}",
            t(
                "report-checked",
                &[(
                    "time",
                    &katna_pdf_sign::shown_date(std::time::SystemTime::now())
                )]
            )
        ),
        String::new(),
    ];
    for row in rows {
        let signer = row.signer.clone().unwrap_or_else(|| row.name.clone());
        lines.push(format!("{signer}: {}", state(row)));
        lines.extend(
            identity(row)
                .into_iter()
                .chain(details(row))
                .map(|l| format!("  {l}")),
        );
        lines.push(format!("  {}", t("report-field", &[("name", &row.name)])));
    }
    lines
}

/// The key to sign with, unlocked: from certificate file `file`, or on
/// token `found`, with `secret`, its password or PIN.
/// Why a key was not unlocked.
#[derive(Debug, Clone)]
enum Refused {
    /// The token's PIN is wrong: what it says of the tries left.
    Pin(String),
    /// The token is locked after too many wrong PINs.
    Locked,
    Other(String),
}

/// What the token said of the PIN just tried (G05).
#[derive(Debug, Clone)]
enum PinTrouble {
    Wrong(String),
    Locked,
}

fn unlock(file: Option<PathBuf>, found: Option<Found>, secret: &str) -> Result<Key, Refused> {
    use katna_pdf_sign::Signer as _;
    if let Some(found) = found {
        let module =
            pkcs11::Module::load(&found.module).map_err(|e| Refused::Other(e.to_string()))?;
        let token = module
            .tokens()
            .map_err(|e| Refused::Other(e.to_string()))?
            .into_iter()
            .find(|t| t.serial == found.token.serial && t.label == found.token.label)
            .ok_or_else(|| Refused::Other(t("sign-token-gone", &[])))?;
        let ids = module.open(&token, secret).map_err(|e| match e {
            pkcs11::TokenError::Pin => match module.pin_tries(&token) {
                pkcs11::PinTries::Last => Refused::Pin(t("sign-wrong-pin-last", &[])),
                pkcs11::PinTries::Few => Refused::Pin(t("sign-wrong-pin-few", &[])),
                pkcs11::PinTries::Locked => Refused::Locked,
                pkcs11::PinTries::Unknown => Refused::Pin(t("sign-wrong-pin", &[])),
            },
            pkcs11::TokenError::Locked => Refused::Locked,
            pkcs11::TokenError::Gone => Refused::Other(t("sign-token-gone", &[])),
            e => Refused::Other(e.to_string()),
        })?;
        // The certificate chosen, else the likeliest.
        let wanted = found.certificates.get(found.chosen);
        let mut ids = ids;
        let at = ids
            .iter()
            .position(|id| wanted.is_some_and(|w| **w == *id.certificate()))
            .unwrap_or(0);
        if ids.is_empty() {
            return Err(Refused::Other(t("sign-no-key", &[])));
        }
        let id = ids.swap_remove(at);
        return Ok(Box::new(id));
    }
    let file = file.ok_or_else(|| Refused::Other(t("sign-no-certificate", &[])))?;
    let p12 =
        std::fs::read(&file).map_err(|e| Refused::Other(format!("{}: {e}", file_name(&file))))?;
    let id = katna_pdf_sign::p12::open(&p12, secret).map_err(|e| {
        Refused::Other(match e {
            katna_pdf_sign::P12Error::Password => t("sign-wrong-password", &[]),
            e => e.to_string(),
        })
    })?;
    Ok(Box::new(id))
}

/// A box ticked or not.
pub(super) fn check_box(on: bool, th: super::Theme) -> gpui::Div {
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
    /// Keep the validation data in the file, fetched now.
    long_term: bool,
    picture: Option<katna_pdf_cos::Picture>,
    /// Lock every form field.
    lock: bool,
}

/// Signs PDF `bytes` with `id` as `what` says, and writes the signed
/// copy to `target`. A box goes on the same page, or the last one when
/// the file is shorter.
fn sign_one(
    bytes: Vec<u8>,
    key: Option<&str>,
    what: &What,
    id: &dyn katna_pdf_sign::Signer,
    target: &Path,
) -> Result<(), String> {
    let doc = super::cos(bytes, key)?;
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
            contact: crate::profile::Profile::load().contact(),
        },
    );
    request.time = time;
    request.certify = what.certify;
    request.picture = what.picture.clone();
    if what.lock {
        request.lock = Some(katna_pdf_cos::Lock::All);
    }
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
    // The chain and OCSP answers or CRLs, fetched now; a signature whose
    // data could not all be had is kept as it is with what was.
    let signed = if what.long_term {
        let doc = super::cos(signed, key)?;
        katna_pdf_sign::add_validation_data(
            &doc,
            &roots(),
            &katna_pdf_sign::revocation::Http::default(),
        )
        .map_err(|e| e.to_string())?
        .0
    } else {
        signed
    };
    // With a timestamp server too, the file and its validation data are
    // timestamped after it (PAdES B-LTA).
    let signed = match server.as_ref().filter(|_| what.long_term) {
        Some(server) => {
            let doc = super::cos(signed, key)?;
            katna_pdf_sign::timestamp_document(&doc, server, katna_pdf_sign::Digest::Sha256)
                .map_err(|e| e.to_string())?
        }
        None => signed,
    };
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

/// The picture last shown in a signature's box, if it is still there.
pub(super) fn remembered_picture() -> Option<PathBuf> {
    let path = PathBuf::from(read_note("picture")?.trim_end_matches('\n'));
    path.is_file().then_some(path)
}

fn remembered_certificate() -> Option<PathBuf> {
    let path = PathBuf::from(read_note("certificate")?.trim_end_matches('\n'));
    path.is_file().then_some(path)
}

/// Whether to timestamp, and the server: `on` or `off`, then the URL.
/// What signing starts with, as Settings > Signing shows and changes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningDefaults {
    pub timestamp: bool,
    pub server: String,
    pub long_term: bool,
}

pub(super) fn signing_defaults() -> SigningDefaults {
    let (timestamp, server) = remembered_timestamp();
    SigningDefaults {
        timestamp,
        server,
        long_term: read_note("long-term").as_deref().map(str::trim) != Some("off"),
    }
}

pub(super) fn set_signing_defaults(defaults: &SigningDefaults) {
    let server = defaults.server.trim();
    let server = if server.is_empty() { TSA } else { server };
    let on = if defaults.timestamp { "on" } else { "off" };
    write_note("timestamp", &format!("{on}\n{server}\n"));
    write_note("long-term", if defaults.long_term { "on" } else { "off" });
}

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
            long_term: false,
            picture: None,
            lock: false,
        };
        let file = Path::new("/somewhere/report.pdf");
        let first = free_name(&folder, file);
        sign_one(pdf.clone(), None, &what, &id, &first).unwrap();
        let second = free_name(&folder, file);
        assert_ne!(first, second);
        sign_one(pdf.clone(), None, &what, &id, &second).unwrap();
        for target in [first, second] {
            let doc = katna_pdf_cos::Document::load(std::fs::read(&target).unwrap()).unwrap();
            let checks = katna_pdf_sign::check(&doc, doc.bytes(), &Roots::new());
            assert!(checks[0].intact());
            // Page 6 asked for; the file has 2.
            assert_eq!(checks[0].signature.page, Some(1));
        }
        assert!(sign_one(b"nope".to_vec(), None, &what, &id, &folder.join("x.pdf")).is_err());

        // A password-protected file: signed with its password, it stays
        // protected, and the signature checks once it is opened.
        let locked = katna_pdf_cos::Document::load(pdf.clone())
            .unwrap()
            .lock("secret", "", katna_pdf_cos::Permissions::ALL)
            .unwrap();
        let target = folder.join("locked.pdf");
        assert!(sign_one(locked.clone(), None, &what, &id, &target).is_err());
        sign_one(locked, Some("secret"), &what, &id, &target).unwrap();
        let mut doc = katna_pdf_cos::Document::load(std::fs::read(&target).unwrap()).unwrap();
        assert!(doc.is_locked());
        doc.unlock("secret").unwrap();
        let checks = katna_pdf_sign::check(&doc, doc.bytes(), &Roots::new());
        assert!(checks[0].intact(), "{:?}", checks[0].coverage);

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
        sign_one(form, None, &what, &id, &target).unwrap();
        let doc = katna_pdf_cos::Document::load(std::fs::read(&target).unwrap()).unwrap();
        let after = signable(&doc);
        assert!(after.fields.is_empty() && after.signed);
        assert_eq!(after.certified, Some(Allowed::FormFilling));
        assert_eq!(check(&doc)[0].certifies, Some(Allowed::FormFilling));

        // The report says the same as the pane.
        let lines = report_lines("certified.pdf", &check(&doc));
        assert_eq!(lines[0], "File: certified.pdf");
        assert!(
            lines.iter().any(|l| l.starts_with("Asha Verma: ")),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"  Reason: Approved".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"  field: Author".to_owned()), "{lines:?}");
        let _ = std::fs::remove_dir_all(folder);
    }
}
