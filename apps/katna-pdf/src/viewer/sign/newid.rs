// SPDX-License-Identifier: GPL-3.0-or-later

//! Making a digital ID of one's own from the signing card, for those
//! with no certificate from an authority: a name, an email, an
//! organisation and a password make a new key and a certificate it
//! signs itself (`katna_pdf_sign::selfsigned`). It is kept in Katna
//! PDF's data folder (`pdf/ids/NAME.pfx`), its certificate beside it
//! (`NAME.cer`) to give to whoever should trust the signatures, and is
//! trusted here at once, so one's own signatures check as one's own.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, Context, Entity, Focusable as _, Subscription, div, prelude::*};
use katna_pdf_sign::selfsigned::{IdError, IdName, new_id};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};

use super::super::Viewer;
use crate::float;
use crate::text::t;

/// The new-ID form, open in the signing card.
pub(in crate::viewer) struct IdForm {
    name: Entity<TextInput>,
    email: Entity<TextInput>,
    organisation: Entity<TextInput>,
    password: Entity<TextInput>,
    again: Entity<TextInput>,
    /// Why the ID was not made, as last tried.
    error: Option<String>,
    /// Being made (a new RSA key takes a moment).
    making: bool,
    _inputs: Vec<Subscription>,
}

/// The folder kept IDs go in.
fn ids_folder() -> Option<PathBuf> {
    katna_core::paths::Paths::from_env()
        .ok()
        .map(|paths| paths.data_dir().join("pdf").join("ids"))
}

/// A file name in `folder` for an ID named `name`, not taken yet.
fn free_id_name(folder: &Path, name: &str) -> PathBuf {
    let stem: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim_matches(['-', ' ']);
    let stem = if stem.is_empty() { "ID" } else { stem };
    let mut target = folder.join(format!("{stem}.pfx"));
    let mut n = 2;
    while target.exists() {
        target = folder.join(format!("{stem} {n}.pfx"));
        n += 1;
    }
    target
}

impl Viewer {
    /// Opens the form that makes a new digital ID.
    pub(in crate::viewer) fn open_id_form(&mut self, cx: &mut Context<Self>) {
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let field = |placeholder: &str, masked: bool, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(t(placeholder, &[]), cx);
                input.set_masked(masked, cx);
                input.set_accent(accent);
                input
            })
        };
        let name = field("newid-name-hint", false, cx);
        let email = field("newid-email-hint", false, cx);
        let organisation = field("newid-organisation-hint", false, cx);
        let password = field("newid-password-hint", true, cx);
        let again = field("newid-again-hint", true, cx);
        let mut subs = Vec::new();
        for input in [&name, &email, &organisation, &password, &again] {
            subs.push(cx.subscribe(input, |this, _, event, cx| match event {
                InputEvent::Submit => this.make_id(cx),
                InputEvent::Cancel => this.close_id_form(cx),
                InputEvent::Changed => {}
            }));
        }
        // The name the system knows the person by, to start with.
        let known = super::super::annots::author();
        if !known.is_empty() {
            name.update(cx, |input, cx| input.set_text(known, cx));
        }
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        signing.new_id = Some(IdForm {
            name,
            email,
            organisation,
            password,
            again,
            error: None,
            making: false,
            _inputs: subs,
        });
        signing.focus_id = true;
        cx.notify();
    }

    fn close_id_form(&mut self, cx: &mut Context<Self>) {
        if let Some(signing) = self.signing.as_mut()
            && signing.new_id.as_ref().is_some_and(|f| !f.making)
        {
            signing.new_id = None;
            signing.focus = true;
            cx.notify();
        }
    }

    /// Focuses the form's first field, once, after it opens.
    pub(in crate::viewer) fn focus_id_form(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(signing) = self.signing.as_mut() else {
            return;
        };
        if !std::mem::take(&mut signing.focus_id) {
            return;
        }
        if let Some(form) = &signing.new_id {
            let name = form.name.clone();
            window.focus(&name.focus_handle(cx), cx);
        }
    }

    /// Makes the ID the form describes, keeps it, and signs with it.
    fn make_id(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.signing.as_mut().and_then(|s| s.new_id.as_mut()) else {
            return;
        };
        if form.making {
            return;
        }
        let read = |input: &Entity<TextInput>| input.read(cx).text().to_owned();
        let who = IdName {
            name: read(&form.name),
            email: read(&form.email),
            organisation: read(&form.organisation),
            ..IdName::default()
        };
        let password = read(&form.password);
        if password != read(&form.again) {
            form.error = Some(t("newid-not-same", &[]));
            cx.notify();
            return;
        }
        let Some(folder) = ids_folder() else {
            form.error = Some(t("newid-no-folder", &[]));
            cx.notify();
            return;
        };
        form.error = None;
        form.making = true;
        cx.notify();
        let secret = password.clone();
        cx.spawn(async move |this, cx| {
            let made = cx
                .background_executor()
                .spawn(async move {
                    let id = new_id(&who, &secret).map_err(|e| match e {
                        IdError::NoName => t("newid-no-name", &[]),
                        IdError::NoPassword => t("newid-no-password", &[]),
                        e => e.to_string(),
                    })?;
                    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
                    let file = free_id_name(&folder, &who.name);
                    std::fs::write(&file, &id.file).map_err(|e| e.to_string())?;
                    // The public half, to give to whoever should trust it.
                    let shared = file.with_extension("cer");
                    let pem = katna_pdf_sign::certview::pem(&id.certificate);
                    std::fs::write(&shared, pem).map_err(|e| e.to_string())?;
                    Ok::<_, String>((file, shared, id.certificate))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(signing) = this.signing.as_mut() else {
                    return;
                };
                match made {
                    Ok((file, shared, certificate)) => {
                        signing.new_id = None;
                        signing.certificate = Some(file.clone());
                        signing.error = None;
                        signing.focus = true;
                        signing
                            .password
                            .update(cx, |input, cx| input.set_text(password, cx));
                        super::write_note("certificate", &file.to_string_lossy());
                        // One's own ID is trusted here: one's own
                        // signatures show as one's own.
                        let _ = super::trust_root(&certificate);
                        this.notice = Some(super::super::said(
                            "newid-made",
                            &[("file", &shared.to_string_lossy())],
                        ));
                    }
                    Err(why) => {
                        if let Some(form) = signing.new_id.as_mut() {
                            form.making = false;
                            form.error = Some(why);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The form, in place of the certificate file row.
    pub(in crate::viewer) fn id_form(&self, form: &IdForm, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let field = |id: &'static str, input: &Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(
                    div()
                        .text_size(px(text::CAPTION))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(th.dim)
                        .child(t(id, &[])),
                )
                .child(
                    div()
                        .h(px(32.0))
                        .flex()
                        .items_center()
                        .px(px(space::S3))
                        .rounded(px(radius::SM))
                        .border_1()
                        .border_color(th.line)
                        .bg(th.card)
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(input.clone()),
                )
        };
        let row = || div().flex().gap(px(space::S3));
        div()
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .p(px(space::S3))
            .rounded(px(radius::SM))
            .border_1()
            .border_color(th.accent)
            .child(
                div()
                    .text_size(px(text::BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(t("newid-title", &[])),
            )
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("newid-about", &[])),
            )
            .child(field("newid-name", &form.name))
            .child(
                row()
                    .child(field("newid-email", &form.email))
                    .child(field("newid-organisation", &form.organisation)),
            )
            .child(
                row()
                    .child(field("newid-password", &form.password))
                    .child(field("newid-again", &form.again)),
            )
            .children(form.error.clone().map(|why| {
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.error)
                    .child(why)
            }))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(space::S2))
                    .child(
                        float::button("newid-cancel", t("newid-cancel", &[]), false, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.close_id_form(cx))),
                    )
                    .child(
                        float::button(
                            "newid-make",
                            t(
                                if form.making {
                                    "newid-making"
                                } else {
                                    "newid-make"
                                },
                                &[],
                            ),
                            true,
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.make_id(cx))),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::free_id_name;

    #[test]
    fn id_files_are_named_from_the_name() {
        let dir = std::env::temp_dir().join(format!("katna-ids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = free_id_name(&dir, "Asha Verma/Office");
        assert_eq!(first.file_name().unwrap(), "Asha Verma-Office.pfx");
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(
            free_id_name(&dir, "Asha Verma/Office").file_name().unwrap(),
            "Asha Verma-Office 2.pfx"
        );
        assert_eq!(free_id_name(&dir, "  ").file_name().unwrap(), "ID.pfx");
        let _ = std::fs::remove_dir_all(dir);
    }
}
