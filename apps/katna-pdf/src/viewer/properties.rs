// SPDX-License-Identifier: GPL-3.0-or-later

//! The document's properties (docs/PLAN.md §4.3, "metadata editor"):
//! a card, opened from the file's name in the toolbar or with Ctrl+D,
//! to change its title, author, subject and keywords, showing what made
//! it and when (`katna_pdf_cos::meta`), to put a password on it or
//! take one off (`katna_pdf_cos::lock`, AES-256), and to make it smaller
//! (`katna_pdf_cos::optimize`), and to save a copy as PDF/A
//! (`katna_pdf_cos::pdfa`).

use std::path::Path;
use std::sync::Arc;

use gpui::{AnyElement, Context, Entity, Subscription, div, prelude::*};

use katna_pdf_cos::{
    ActionPlace, DocAction, DocEvent, FieldEvent, Metadata, Optimize, PdfaIssue, Permissions,
};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::Tip;

use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// The card's fields: title, author, subject, keywords.
pub(super) struct Properties {
    /// As the file says, once read.
    read: Option<Metadata>,
    /// Whether the file has a password, and whether it was opened with
    /// its owner's (so it may be taken off), once read.
    locked: Option<(bool, bool)>,
    /// How large the file is, in bytes, once read.
    size: Option<u64>,
    /// The scripts and actions the file carries, once read.
    actions: Option<Vec<DocAction>>,
    /// What keeps the file from being PDF/A, once read (nothing: it is).
    pdfa: Option<Vec<PdfaIssue>>,
    /// The fonts the pages draw with, once read.
    fonts: Option<Vec<katna_pdf_cos::FontUse>>,
    /// What was wrong with the file, when it had to be repaired.
    repairs: Vec<katna_pdf_cos::Repair>,
    fields: [Entity<TextInput>; 4],
    /// A new password to put on: the one that opens the file, and the
    /// owner's, which opens it fully.
    password: Entity<TextInput>,
    owner: Entity<TextInput>,
    /// What someone opening it without the owner's password may do.
    allow: Permissions,
    /// Leave the title and author readable without the password.
    plain_meta: bool,
    /// The part shown.
    part: Part,
    /// How to make it smaller: the same look, pictures to 150 dpi, or
    /// to 96 dpi.
    shrink: usize,
    _fields: Vec<Subscription>,
}

/// Reaches one of the allowed actions.
type Allowed = fn(&mut Permissions) -> &mut bool;

/// The allowed actions the card offers, in order.
const ALLOWED: [(&str, Allowed); 6] = [
    ("properties-allow-print", |p| &mut p.print),
    ("properties-allow-change", |p| &mut p.change),
    ("properties-allow-copy", |p| &mut p.copy),
    ("properties-allow-comment", |p| &mut p.comment),
    ("properties-allow-fill", |p| &mut p.fill_forms),
    ("properties-allow-assemble", |p| &mut p.assemble),
];

const LABELS: [&str; 4] = [
    "properties-title",
    "properties-author",
    "properties-subject",
    "properties-keywords",
];

impl Viewer {
    /// Opens the properties card, or closes it.
    pub(super) fn toggle_properties(&mut self, cx: &mut Context<Self>) {
        if self.properties.take().is_some() {
            cx.notify();
            return;
        }
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        self.marking = None;
        self.redacting = None;
        self.linking = None;
        self.numbering = None;
        self.designing = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let fields = LABELS.map(|_| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx);
                input.set_accent(accent);
                input
            })
        });
        let password = cx.new(|cx| {
            let mut input = TextInput::new(t("properties-password-new", &[]), cx);
            input.set_accent(accent);
            input.set_masked(true, cx);
            input
        });
        let owner = cx.new(|cx| {
            let mut input = TextInput::new(t("properties-password-owner-new", &[]), cx);
            input.set_accent(accent);
            input.set_masked(true, cx);
            input
        });
        let subs = fields
            .iter()
            .map(|f| {
                cx.subscribe(f, |this, _, event, cx| {
                    if let InputEvent::Submit = event {
                        this.keep_properties(cx);
                    }
                })
            })
            .collect();
        self.properties = Some(Properties {
            read: None,
            locked: None,
            size: None,
            actions: None,
            pdfa: None,
            fonts: None,
            repairs: Vec::new(),
            fields,
            password,
            owner,
            allow: Permissions::ALL,
            plain_meta: false,
            part: Part::Description,
            shrink: 0,
            _fields: subs,
        });
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let meta = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let size = bytes.len() as u64;
                    super::cos(bytes, key.as_deref()).ok().map(|d| {
                        (
                            d.metadata(),
                            (d.is_encrypted(), d.opened_as_owner()),
                            size,
                            d.document_actions(),
                            d.pdfa_issues(),
                            d.fonts(),
                            d.repairs().to_vec(),
                        )
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(properties) = this.properties.as_mut() else {
                    return;
                };
                let (meta, locked, size, actions, pdfa, fonts, repairs) = meta.unwrap_or_default();
                properties.fonts = Some(fonts);
                properties.repairs = repairs;
                properties.locked = Some(locked);
                properties.size = Some(size);
                properties.actions = Some(actions);
                properties.pdfa = Some(pdfa);
                let values = [&meta.title, &meta.author, &meta.subject, &meta.keywords];
                for (field, value) in properties.fields.iter().zip(values) {
                    let value = value.clone().unwrap_or_default();
                    field.update(cx, |f, cx| f.set_text(value, cx));
                }
                properties.read = Some(meta);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Writes the card's fields into the file, when they changed.
    fn keep_properties(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.take() else {
            return;
        };
        cx.notify();
        let Some(read) = properties.read else {
            return;
        };
        let [title, author, subject, keywords] = properties
            .fields
            .clone()
            .map(|f| Some(f.read(cx).text().trim().to_owned()).filter(|v| !v.is_empty()));
        let meta = Metadata {
            title,
            author,
            subject,
            keywords,
            ..read.clone()
        };
        if meta == read {
            return;
        }
        self.change_document(
            Vec::new(),
            "properties-failed",
            move |doc| doc.set_metadata(&meta).map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Puts the password typed on the file, its owner's too, so the same
    /// password opens it fully.
    fn lock_file(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.as_ref() else {
            return;
        };
        let password = properties.password.read(cx).text().to_owned();
        let owner = properties.owner.read(cx).text().to_owned();
        let allow = properties.allow;
        let plain_meta = properties.plain_meta;
        // One password typed serves both ways; the owner's alone leaves
        // the file open to anyone, with only what is allowed.
        let (user, owner) = match (password.is_empty(), owner.is_empty()) {
            (true, true) => return,
            (false, true) => (password.clone(), password),
            (true, false) => (String::new(), owner),
            (false, false) => (password, owner),
        };
        self.properties = None;
        let pages = (0..self.page_count()).collect();
        let after: Arc<str> = Arc::from(owner.as_str());
        self.change_document_keyed(
            pages,
            "properties-lock-failed",
            Some(after),
            move |doc| {
                doc.lock_with(&user, &owner, allow, plain_meta)
                    .map_err(|e| e.to_string())
            },
            cx,
        );
    }

    /// Takes the file's password off.
    fn unlock_file(&mut self, cx: &mut Context<Self>) {
        self.properties = None;
        let pages = (0..self.page_count()).collect();
        self.change_document_keyed(
            pages,
            "properties-lock-failed",
            None,
            |doc| doc.without_lock().map_err(|e| e.to_string()),
            cx,
        );
    }

    /// Takes the lock off a file opened without its owner's password,
    /// with the owner's password typed in the card.
    fn unlock_file_with(&mut self, cx: &mut Context<Self>) {
        let Some(properties) = self.properties.as_ref() else {
            return;
        };
        let owner = properties.owner.read(cx).text().to_owned();
        if owner.is_empty() {
            return;
        }
        // The change opens the file with the owner's password; the
        // result has none.
        let key = self.key.replace(Arc::from(owner.as_str()));
        self.unlock_file(cx);
        self.key = key;
    }

    /// Writes the file again as small as `how` allows, and says how much
    /// smaller it came out; it is not saved until it is.
    fn shrink(&mut self, how: Optimize, cx: &mut Context<Self>) {
        self.properties = None;
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        let current = self.unsaved();
        let key = self.key.clone();
        self.notice = Some(super::said("properties-shrinking", &[]));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let done = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let before = bytes.len() as u64;
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let done = doc.optimize(&how).map_err(|e| e.to_string())?;
                    Ok::<_, String>((before, done.bytes))
                })
                .await;
            this.update(cx, |this, cx| {
                match done {
                    Ok((before, bytes)) if (bytes.len() as u64) < before => {
                        let after = bytes.len() as u64;
                        let pages = (0..this.page_count()).collect();
                        this.change_document(
                            pages,
                            "properties-shrink-failed",
                            move |_| Ok(bytes),
                            cx,
                        );
                        this.notice = Some(super::said(
                            "properties-shrunk",
                            &[
                                ("before", &crate::library::size_text(before)),
                                ("after", &crate::library::size_text(after)),
                            ],
                        ));
                    }
                    Ok(_) => this.notice = Some(super::said("properties-shrink-none", &[])),
                    Err(error) => {
                        this.notice = Some(super::said(
                            "properties-shrink-failed",
                            &[("error", &error)],
                        ))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks where to save the file as PDF/A, then writes it there: fonts
    /// a reader supplies embedded from the computer's own, an sRGB
    /// output intent and metadata put in, scripts and kept files taken
    /// out. The file open stays as it is.
    fn save_as_pdfa(&mut self, cx: &mut Context<Self>) {
        self.properties = None;
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = format!(
            "{}-pdfa.pdf",
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into())
        );
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        let current = self.unsaved();
        let key = self.key.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.notice = Some(super::said("pdfa-working", &[]));
                cx.notify();
            })
            .ok();
            let done = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).map_err(|e| e.to_string())?;
                    let fonts = katna_pdf_cos::font_files(&katna_pdf_cos::font_dirs());
                    let done = doc
                        .to_pdfa(&|want| katna_pdf_cos::find_font(&fonts, want))
                        .map_err(|e| e.to_string())?;
                    std::fs::write(&target, &done.bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>((super::file_name(&target), done.left))
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match done {
                    Ok((name, left)) if left.is_empty() => {
                        super::said("pdfa-saved", &[("name", &name)])
                    }
                    Ok((name, left)) => {
                        let left: Vec<String> = left.iter().map(ToString::to_string).collect();
                        super::said(
                            "pdfa-saved-left",
                            &[("name", &name), ("left", &left.join("; "))],
                        )
                    }
                    Err(error) => super::said("pdfa-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The properties card, while it is open (R14, R16): a heading
    /// with a close button, a tab for each part, the part chosen in a
    /// body that scrolls, and the buttons at the foot, always in view.
    pub(super) fn properties_card(
        &self,
        max_height: f32,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let properties = self.properties.as_ref()?;
        let th = self.theme;
        let fields = self.form.as_ref().map_or(0, |f| f.fields.len());
        let comments = self
            .annots
            .as_ref()
            .map_or(0, |a| a.iter().map(Vec::len).sum::<usize>());
        let tabs: Vec<Part> = Part::ALL
            .into_iter()
            .filter(|p| match p {
                Part::Form => fields > 0,
                Part::Comments => comments > 0,
                _ => true,
            })
            .collect();
        let part = if tabs.contains(&properties.part) {
            properties.part
        } else {
            Part::Description
        };
        let tab_row = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S2))
            .children(tabs.into_iter().map(|p| {
                float::chip(
                    ("properties-tab", p as usize),
                    t(p.label(), &[]),
                    p == part,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(properties) = this.properties.as_mut() {
                        properties.part = p;
                    }
                    cx.notify();
                }))
            }));
        let header = div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .px(px(space::S5))
            .pt(px(space::S4))
            .pb(px(space::S3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::TITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("properties-heading", &[])),
                    )
                    .child(
                        float::ib(
                            "properties-close",
                            "x",
                            t("properties-close", &[]),
                            false,
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.properties = None;
                            cx.notify();
                        })),
                    ),
            )
            .child(tab_row);
        let body = match part {
            Part::Description => self.description_part(properties, window, cx),
            Part::Password => self.password_part(properties, window, cx),
            Part::Size => self.size_part(properties, cx),
            Part::Pdfa => self.pdfa_part(properties, cx),
            Part::Fonts => self.fonts_part(properties),
            Part::Form => self.form_part(fields, cx),
            Part::Comments => self.comments_part(comments, cx),
            Part::Scripts => self.scripts_part(properties, cx),
        };
        let foot = div()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S5))
            .py(px(space::S3))
            .border_t_1()
            .border_color(th.line)
            .child(
                div()
                    .flex_1()
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(t("properties-foot", &[])),
            )
            .when(part == Part::Description, |d| {
                d.child(
                    float::button("properties-cancel", t("properties-cancel", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.properties = None;
                            cx.notify();
                        })),
                )
                .child(
                    float::button("properties-keep", t("properties-keep", &[]), true, &th)
                        .on_click(cx.listener(|this, _, _, cx| this.keep_properties(cx))),
                )
            })
            .when(part != Part::Description, |d| {
                d.child(
                    float::button("properties-done", t("properties-close", &[]), false, &th)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.properties = None;
                            cx.notify();
                        })),
                )
            });
        let height = max_height.clamp(240.0, 560.0);
        let card = div()
            .id("properties-card")
            .w(px(680.0))
            .max_w(px(680.0))
            .h(px(height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(12.0))
            .bg(th.card)
            .shadow(th.lifted())
            .when(th.dark, |d| d.border_1().border_color(th.line))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(
                div()
                    .id("properties-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(space::S5))
                    .pb(px(space::S4))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(body),
            )
            .child(foot);
        Some(card.into_any_element())
    }

    /// Title, author, subject and keywords, and what the file says of
    /// itself (R14).
    fn description_part(
        &self,
        properties: &Properties,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let field = |i: usize| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(space::S1))
                .child(label(LABELS[i], th))
                .child(float::input_box(&properties.fields[i], window, cx, &th))
        };
        let mut facts: Vec<(&str, String)> = Vec::new();
        if let Some(path) = &self.path {
            facts.push(("properties-file", path.display().to_string()));
        }
        if let Some(size) = properties.size {
            facts.push((
                "properties-size",
                t(
                    "properties-size-pages",
                    &[
                        ("size", &crate::library::size_text(size)),
                        ("pages", &self.page_count().to_string()),
                    ],
                ),
            ));
        }
        if let Some(read) = &properties.read {
            let rows = [
                ("properties-creator", read.creator.clone()),
                ("properties-producer", read.producer.clone()),
                (
                    "properties-created",
                    read.created.as_deref().and_then(shown_date),
                ),
                (
                    "properties-modified",
                    read.modified.as_deref().and_then(shown_date),
                ),
            ];
            facts.extend(rows.into_iter().filter_map(|(id, v)| v.map(|v| (id, v))));
        }
        if !properties.repairs.is_empty() {
            facts.push((
                "properties-repaired",
                super::repairs_text(&properties.repairs),
            ));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(field(0))
            .child(
                div()
                    .flex()
                    .gap(px(space::S3))
                    .child(field(1))
                    .child(field(2)),
            )
            .child(field(3))
            .child(section(t("properties-about", &[]), th))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .children(facts.into_iter().map(|(id, value)| {
                        div()
                            .flex()
                            .gap(px(space::S3))
                            .text_size(px(text::SMALL))
                            .child(
                                div()
                                    .w(px(140.0))
                                    .flex_none()
                                    .text_color(th.dim)
                                    .child(t(id, &[])),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    // A path stays on one line; what was
                                    // repaired is read whole.
                                    .when(id != "properties-repaired", |d| {
                                        d.text_ellipsis().whitespace_nowrap()
                                    })
                                    .text_color(th.text)
                                    .child(value),
                            )
                    })),
            )
            .into_any_element()
    }

    /// Putting a password on, or taking it off.
    fn password_part(
        &self,
        properties: &Properties,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Some((locked, owner)) = properties.locked else {
            return div().into_any_element();
        };
        let field = |input: &Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .child(float::input_box(input, window, cx, &th))
        };
        let mut allowed = div()
            .flex()
            .flex_wrap()
            .gap_x(px(space::S4))
            .gap_y(px(space::S2));
        for (i, (id, get)) in ALLOWED.into_iter().enumerate() {
            let mut allow = properties.allow;
            let on = *get(&mut allow);
            allowed = allowed.child(
                div()
                    .id(("properties-allow", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(p) = this.properties.as_mut() {
                            let flag = get(&mut p.allow);
                            *flag = !*flag;
                        }
                        cx.notify();
                    }))
                    .child(super::sign::check_box(on, th))
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(t(id, &[])),
                    ),
            );
        }
        let plain_meta = div()
            .id("properties-plain-meta")
            .flex()
            .items_center()
            .gap(px(space::S2))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(p) = this.properties.as_mut() {
                    p.plain_meta = !p.plain_meta;
                }
                cx.notify();
            }))
            .child(super::sign::check_box(properties.plain_meta, th))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(text::SMALL))
                    .text_color(th.text)
                    .child(t("properties-plain-meta", &[])),
            );
        let part = div().flex().flex_col().gap(px(space::S3));
        match (locked, owner) {
            (false, _) => part
                .child(note(t("properties-password-none", &[]), th))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S3))
                        .child(field(&properties.password))
                        .child(field(&properties.owner)),
                )
                .child(section(t("properties-allow", &[]), th))
                .child(allowed)
                .child(plain_meta)
                .child(
                    div().flex().justify_end().child(
                        float::button("properties-lock", t("properties-lock", &[]), true, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.lock_file(cx))),
                    ),
                ),
            (true, true) => part
                .child(note(t("properties-password-on", &[]), th))
                .child(
                    div().flex().child(
                        float::button("properties-unlock", t("properties-unlock", &[]), true, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.unlock_file(cx))),
                    ),
                ),
            (true, false) => part
                .child(note(t("properties-password-owner", &[]), th))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(space::S3))
                        .child(field(&properties.owner))
                        .child(
                            float::button(
                                "properties-unlock",
                                t("properties-unlock", &[]),
                                true,
                                &th,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.unlock_file_with(cx))),
                        ),
                ),
        }
        .into_any_element()
    }

    /// How large the file is, and making it smaller (R16).
    fn size_part(&self, properties: &Properties, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(size) = properties.size else {
            return div().into_any_element();
        };
        let ways = [
            ("properties-shrink-same", 0),
            ("properties-shrink-150", 1),
            ("properties-shrink-96", 2),
        ];
        let mut seg = float::seg(&th);
        for (id, i) in ways {
            seg = seg.child(
                float::seg_item(
                    ("properties-shrink-way", i),
                    None,
                    Some(t(id, &[]).into()),
                    properties.shrink == i,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = this.properties.as_mut() {
                        p.shrink = i;
                    }
                    cx.notify();
                })),
            );
        }
        let how = properties.shrink;
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .text_size(px(text::TITLE))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(crate::library::size_text(size)),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(t("properties-size-label", &[])),
                    ),
            )
            .child(note(t("properties-size-how", &[]), th))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(seg)
                    .child(div().flex_1())
                    .child(
                        float::button("properties-shrink", t("properties-shrink", &[]), true, &th)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let way = match how {
                                    0 => Optimize::LOSSLESS,
                                    1 => Optimize::MEDIUM,
                                    _ => Optimize::STRONG,
                                };
                                this.shrink(way, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    /// Whether the file is PDF/A, what stands in the way, and a copy
    /// made PDF/A (R16).
    fn pdfa_part(&self, properties: &Properties, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(issues) = &properties.pdfa else {
            return div().into_any_element();
        };
        if issues.is_empty() {
            return note(t("pdfa-is", &[]), th).into_any_element();
        }
        let count = issues.len().to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .p(px(space::S3))
                    .rounded(px(radius::SM))
                    .bg(th.error_soft)
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("pdfa-not-yet", &[("count", &count)])),
                    )
                    .children(issues.iter().map(|issue| {
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(format!("• {issue}"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(note(t("pdfa-can-fix", &[]), th)),
                    )
                    .child(
                        float::button("pdfa-save", t("pdfa-save", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.save_as_pdfa(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The form: its fields tinted, reset, or drawn in for good.
    fn form_part(&self, fields: usize, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let count = fields.to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(note(t("properties-form-fields", &[("count", &count)]), th))
            .child(
                div()
                    .id("form-highlight")
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.highlight_fields = !this.highlight_fields;
                        cx.notify();
                    }))
                    .child(super::sign::check_box(self.highlight_fields, th))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(t("form-highlight", &[])),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S2))
                    .child(
                        float::button("form-reset", t("form-reset", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.reset_form(cx))),
                    )
                    .child(
                        float::button("form-flatten", t("form-flatten", &[]), false, &th)
                            .tip(t("form-flatten-tip", &[]), &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.flatten(
                                    katna_pdf_cos::Flatten {
                                        forms: true,
                                        comments: false,
                                    },
                                    cx,
                                )
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The comments: drawn in for good.
    fn comments_part(&self, comments: usize, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let count = comments.to_string();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(note(
                t("properties-comments-count", &[("count", &count)]),
                th,
            ))
            .child(
                div().flex().child(
                    float::button("comments-flatten", t("comments-flatten", &[]), false, &th)
                        .tip(t("comments-flatten-tip", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.flatten(
                                katna_pdf_cos::Flatten {
                                    forms: false,
                                    comments: true,
                                },
                                cx,
                            )
                        })),
                ),
            )
            .into_any_element()
    }

    /// The file's scripts and actions, none of them run (R16).
    /// The fonts the pages draw with: each one's type, whether it is in
    /// the file, and the pages that use it.
    fn fonts_part(&self, properties: &Properties) -> AnyElement {
        let th = self.theme;
        let Some(fonts) = properties.fonts.as_ref() else {
            return div().into_any_element();
        };
        if fonts.is_empty() {
            return note(t("properties-fonts-none", &[]), th).into_any_element();
        }
        let missing = fonts.iter().filter(|f| !f.embedded).count();
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(note(
                if missing == 0 {
                    t("properties-fonts-all-in", &[])
                } else {
                    t(
                        "properties-fonts-missing",
                        &[("count", &missing.to_string())],
                    )
                },
                th,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(radius::SM))
                    .border_1()
                    .border_color(th.line)
                    .overflow_hidden()
                    .children(fonts.iter().enumerate().map(|(i, font)| {
                        let kept = t(
                            match (font.embedded, font.subset) {
                                (true, true) => "properties-font-subset",
                                (true, false) => "properties-font-embedded",
                                (false, _) => "properties-font-not-embedded",
                            },
                            &[],
                        );
                        div()
                            .flex()
                            .flex_col()
                            .px(px(space::S3))
                            .py(px(space::S2))
                            .when(i > 0, |d| d.border_t_1().border_color(th.line))
                            .child(
                                div()
                                    .text_size(px(text::BODY))
                                    .text_color(th.text)
                                    .child(font.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(t(
                                        "properties-font-line",
                                        &[
                                            ("kind", &font.kind),
                                            ("kept", &kept),
                                            ("pages", &katna_pdf_cos::ranges::format(&font.pages)),
                                        ],
                                    )),
                            )
                    })),
            )
            .into_any_element()
    }

    fn scripts_part(&self, properties: &Properties, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(actions) = properties.actions.as_ref() else {
            return div().into_any_element();
        };
        if actions.is_empty() {
            return note(t("properties-actions-none", &[]), th).into_any_element();
        }
        let list = div()
            .flex()
            .flex_col()
            .rounded(px(radius::SM))
            .border_1()
            .border_color(th.line)
            .overflow_hidden()
            .children(actions.iter().enumerate().map(|(i, a)| {
                let detail = a.action.detail();
                let first = detail.lines().next().unwrap_or_default();
                let shown: String = if first.chars().count() > 70 {
                    first.chars().take(68).chain("…".chars()).collect()
                } else {
                    first.to_owned()
                };
                let whole = detail.clone();
                div()
                    .id(("properties-action", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .when(i > 0, |d| d.border_t_1().border_color(th.line))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .tip(t("properties-action-copy", &[]), &th.katna)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(whole.clone()));
                        this.notice = Some(super::said("properties-action-copied", &[]));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_none()
                            .px(px(space::S2))
                            .py(px(2.0))
                            .rounded(px(radius::XS))
                            .bg(th.sunk)
                            .text_size(px(text::CAPTION))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(format!(
                                "{} · {}",
                                place_label(&a.place),
                                t(&format!("actions-kind-{}", a.action.kind()), &[])
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family("monospace")
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(shown),
                    )
            }));
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(list)
            .child(note(t("properties-actions-tip", &[]), th))
            .into_any_element()
    }
}

/// The card's parts, one a tab.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Part {
    #[default]
    Description,
    Password,
    Size,
    Pdfa,
    Fonts,
    Form,
    Comments,
    Scripts,
}

impl Part {
    const ALL: [Part; 8] = [
        Part::Description,
        Part::Password,
        Part::Size,
        Part::Pdfa,
        Part::Fonts,
        Part::Form,
        Part::Comments,
        Part::Scripts,
    ];

    fn label(self) -> &'static str {
        match self {
            Part::Description => "properties-tab-description",
            Part::Password => "properties-password",
            Part::Size => "properties-size",
            Part::Pdfa => "pdfa-heading",
            Part::Fonts => "properties-fonts",
            Part::Form => "properties-form",
            Part::Comments => "properties-comments",
            Part::Scripts => "properties-tab-scripts",
        }
    }
}

/// A field's name over it.
fn label(id: &str, th: crate::theme::Theme) -> gpui::Div {
    div()
        .text_size(px(text::SMALL))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(th.dim)
        .child(t(id, &[]))
}

/// A small heading in capitals between groups.
fn section(words: String, th: crate::theme::Theme) -> gpui::Div {
    div()
        .pt(px(space::S2))
        .text_size(px(text::MICRO))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(th.dim)
        .child(words.to_uppercase())
}

/// Words that explain, in the dimmer colour.
fn note(words: String, th: crate::theme::Theme) -> gpui::Div {
    div()
        .text_size(px(text::SMALL))
        .text_color(th.dim)
        .child(words)
}

/// A PDF date (`D:20261019103005+05'30'`) as people read it:
/// `2026-10-19 10:30`, with its zone when it has one.
fn shown_date(pdf: &str) -> Option<String> {
    let d = pdf.trim().trim_start_matches("D:");
    let part = |from: usize| {
        d.get(from..from + 2)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
    };
    let year = d
        .get(0..4)
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))?;
    let mut out = format!(
        "{year}-{}-{}",
        part(4).unwrap_or("01"),
        part(6).unwrap_or("01")
    );
    if let (Some(h), Some(m)) = (part(8), part(10)) {
        out.push_str(&format!(" {h}:{m}"));
        match d.get(14..15) {
            Some("Z") => out.push_str(" UTC"),
            Some(sign @ ("+" | "-")) => {
                if let (Some(zh), Some(zm)) = (d.get(15..17), d.get(18..20)) {
                    out.push_str(&format!(" {sign}{zh}:{zm}"));
                }
            }
            _ => {}
        }
    }
    Some(out)
}

/// Where an action is attached, as people read it.
fn place_label(place: &ActionPlace) -> String {
    match place {
        ActionPlace::Document(name) => t("actions-place-document", &[("name", name)]),
        ActionPlace::Open => t("actions-place-open", &[]),
        ActionPlace::DocumentEvent(e) => t(
            match e {
                DocEvent::WillClose => "actions-place-will-close",
                DocEvent::WillSave => "actions-place-will-save",
                DocEvent::DidSave => "actions-place-did-save",
                DocEvent::WillPrint => "actions-place-will-print",
                DocEvent::DidPrint => "actions-place-did-print",
            },
            &[],
        ),
        ActionPlace::Page { page, opened } => t(
            if *opened {
                "actions-place-page-open"
            } else {
                "actions-place-page-close"
            },
            &[("page", &(page + 1).to_string())],
        ),
        ActionPlace::Field { name, event } => {
            let event = t(
                match event {
                    FieldEvent::Click => "actions-event-click",
                    FieldEvent::Keystroke => "actions-event-keystroke",
                    FieldEvent::Format => "actions-event-format",
                    FieldEvent::Validate => "actions-event-validate",
                    FieldEvent::Calculate => "actions-event-calculate",
                    FieldEvent::Focus => "actions-event-focus",
                    FieldEvent::Blur => "actions-event-blur",
                    FieldEvent::MouseEnter => "actions-event-mouse-enter",
                    FieldEvent::MouseExit => "actions-event-mouse-exit",
                    FieldEvent::MouseDown => "actions-event-mouse-down",
                    FieldEvent::MouseUp => "actions-event-mouse-up",
                },
                &[],
            );
            t("actions-place-field", &[("name", name), ("event", &event)])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shown_date;

    #[test]
    fn dates_read_as_people_write_them() {
        assert_eq!(
            shown_date("D:20261019103005+05'30'").as_deref(),
            Some("2026-10-19 10:30 +05:30")
        );
        assert_eq!(
            shown_date("D:20261019103005Z").as_deref(),
            Some("2026-10-19 10:30 UTC")
        );
        assert_eq!(shown_date("D:20261019").as_deref(), Some("2026-10-19"));
        assert_eq!(shown_date("soon"), None);
    }
}
