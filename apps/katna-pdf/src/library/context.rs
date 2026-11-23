// SPDX-License-Identifier: GPL-3.0-or-later

//! The library's right-click menus (L12): on a document, a drive's file
//! or folder, and a PDF attached to mail.

use std::path::PathBuf;

use gpui::{AnyElement, Context, Pixels, Point, prelude::*};
use katna_dbus::CloudEntry;
use katna_ui::px;

use super::{LibraryEvent, LibraryView};
use crate::float::{self, menu, menu_item, menu_sep};
use crate::text::t;

/// What was right-clicked.
#[derive(Clone)]
pub(super) enum Clicked {
    Doc(i64),
    Drive(CloudEntry),
    Mail(usize),
}

/// What to do with a file once it is here (a drive's or a mail's is
/// fetched first).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Then {
    Open,
    Window,
    SaveCopy,
}

impl LibraryView {
    /// Opens the menu for `clicked` at `at`.
    pub(super) fn show_context(
        &mut self,
        clicked: Clicked,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.context = Some((clicked, at));
        cx.notify();
    }

    /// Does `then` with `path`, a file on this computer.
    pub(super) fn then_with(&mut self, path: PathBuf, then: Then, cx: &mut Context<Self>) {
        match then {
            Then::Open => cx.emit(LibraryEvent::Open(path, None)),
            Then::Window => cx.emit(LibraryEvent::OpenWindow(path)),
            Then::SaveCopy => {
                let folder = dirs_home();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let chosen = cx.prompt_for_new_path(&folder, Some(&name));
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(target))) = chosen.await else {
                        return;
                    };
                    let copied = std::fs::copy(&path, &target).map_err(|e| e.to_string());
                    this.update(cx, |this, cx| {
                        this.notice = Some(match copied {
                            Ok(_) => t(
                                "library-copy-saved",
                                &[("name", &target.display().to_string())],
                            ),
                            Err(e) => t("library-copy-failed", &[("error", &e)]),
                        });
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// The menu open, placed where the click was.
    pub(super) fn context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (clicked, at) = self.context.clone()?;
        let th = self.theme;
        let item = |id: &'static str, glyph: Option<&'static str>| {
            menu_item(id, glyph, t(id, &[]), None, &th).when(glyph.is_none(), |d| d.pl(px(36.0)))
        };
        let close = |this: &mut Self, cx: &mut Context<Self>| {
            this.context = None;
            cx.notify();
        };
        let mut list = menu("library-context", 230.0, &th)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| close(this, cx)));
        match clicked {
            Clicked::Doc(id) => {
                let doc = self.docs.iter().find(|d| d.id == id)?.clone();
                let (path, copy, folder) = (doc.path.clone(), doc.path.clone(), doc.path.clone());
                let starred = doc.rating > 0;
                list = list
                    .child(
                        item("library-context-open", Some("eye")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.open(id, None, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-window", Some("tabs")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.then_with(path.clone(), Then::Window, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-folder", Some("folder")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                cx.reveal_path(&folder);
                            },
                        )),
                    )
                    .child(menu_sep(&th))
                    .child(
                        item(
                            if starred {
                                "library-context-unstar"
                            } else {
                                "library-context-star"
                            },
                            Some("star"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            close(this, cx);
                            this.rate(id, if starred { 0 } else { 5 }, cx);
                        })),
                    )
                    .child(
                        item("library-context-tags", Some("tag")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.chosen = vec![id];
                                this.select(id, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-copy-path", Some("copy")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    copy.display().to_string(),
                                ));
                            },
                        )),
                    )
                    .child(menu_sep(&th))
                    .child(
                        item("library-context-copies", Some("copy")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.go(super::Place::Copies, cx);
                            },
                        )),
                    )
                    .child(
                        menu_item(
                            "library-context-forget",
                            Some("x"),
                            t("library-context-forget", &[]),
                            None,
                            &th,
                        )
                        .text_color(th.error)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            close(this, cx);
                            this.forget(id, cx);
                        })),
                    );
            }
            Clicked::Drive(entry) if entry.folder => {
                list = list.child(item("library-context-open", Some("folder")).on_click(
                    cx.listener(move |this, _, _, cx| {
                        close(this, cx);
                        this.enter_folder(&entry, cx);
                    }),
                ));
            }
            Clicked::Drive(entry) => {
                let (a, b, c) = (entry.clone(), entry.clone(), entry);
                list = list
                    .child(
                        item("library-context-open", Some("eye")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_entry(a.clone(), Then::Open, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-window", Some("tabs")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_entry(b.clone(), Then::Window, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-save", Some("download")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_entry(c.clone(), Then::SaveCopy, cx);
                            },
                        )),
                    );
            }
            Clicked::Mail(i) => {
                list = list
                    .child(
                        item("library-context-open", Some("eye")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_mail_pdf(i, Then::Open, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-window", Some("tabs")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_mail_pdf(i, Then::Window, cx);
                            },
                        )),
                    )
                    .child(
                        item("library-context-save", Some("download")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                close(this, cx);
                                this.fetch_mail_pdf(i, Then::SaveCopy, cx);
                            },
                        )),
                    );
            }
        }
        Some(
            gpui::deferred(
                katna_ui::anchored()
                    .position(at)
                    .snap_to_window()
                    .child(float::appear("library-context-in", list, -4.0)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// Where a copy is first offered to go.
fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}
