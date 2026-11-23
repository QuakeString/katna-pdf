// SPDX-License-Identifier: GPL-3.0-or-later

//! Right-clicking in the side pane, as other readers allow: a page's
//! thumbnail offers going there, turning, moving, taking out and copying
//! its page, and the pages picked with it; a bookmark offers going
//! there, renaming, setting it to the page shown, moving and deleting.

use gpui::{AnyElement, Context, Pixels, Point, prelude::*};

use super::Viewer;
use crate::contents::{Change, Path};
use crate::float;
use crate::organize::PageOp;
use crate::text::t;

/// What was right-clicked in the pane.
pub(super) enum SideMenu {
    Thumb { at: Point<Pixels>, page: usize },
    Bookmark { at: Point<Pixels>, path: Path },
}

impl Viewer {
    /// A thumbnail right-clicked: its page is picked unless it already
    /// is, with others, and the menu opens.
    pub(super) fn thumb_menu(&mut self, at: Point<Pixels>, page: usize, cx: &mut Context<Self>) {
        if !self.picked.contains(&page) {
            self.picked = [page].into();
            self.anchor = Some(page);
        }
        self.side_menu = Some(SideMenu::Thumb { at, page });
        cx.notify();
    }

    /// A bookmark right-clicked: it is picked and the menu opens.
    pub(super) fn bookmark_menu(&mut self, at: Point<Pixels>, path: Path, cx: &mut Context<Self>) {
        if let Some(contents) = self.contents.as_mut() {
            contents.chosen = Some(path.clone());
        }
        self.side_menu = Some(SideMenu::Bookmark { at, path });
        cx.notify();
    }

    /// The pane's menu, at the pointer, while open.
    pub(super) fn side_menu_element(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let th = self.theme;
        let item = |id: &'static str, glyph: &str, words: String| {
            float::menu_item(id, Some(glyph), words, None, &th)
        };
        let mut body = float::menu("side-menu", 240.0, &th).on_mouse_down_out(cx.listener(
            |this, _, _, cx| {
                this.side_menu = None;
                cx.notify();
            },
        ));
        let at = match self.side_menu.as_ref()? {
            SideMenu::Thumb { at, page } => {
                let page = *page;
                let many = self.picked.len() > 1;
                let pages = |one: &str, more: &str| {
                    if many {
                        t(more, &[("count", &self.picked.len().to_string())])
                    } else {
                        t(one, &[])
                    }
                };
                let op = |op: PageOp| {
                    cx.listener(move |this: &mut Viewer, _: &gpui::ClickEvent, _, cx| {
                        this.side_menu = None;
                        this.page_op(op, cx);
                    })
                };
                let last = page + 1 == self.page_count();
                body = body
                    .child(
                        item("thumb-menu-go", "page", t("thumb-menu-go", &[])).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.side_menu = None;
                                this.go_to_page(page, cx);
                            }),
                        ),
                    )
                    .child(
                        item("thumb-menu-copy", "img", t("page-menu-copy-page", &[])).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.side_menu = None;
                                this.copy_page_picture(page, cx);
                            }),
                        ),
                    )
                    .child(float::menu_sep(&th))
                    .child(
                        item(
                            "thumb-menu-right",
                            "rot",
                            pages("thumb-menu-turn-right", "thumb-menu-turn-right-many"),
                        )
                        .on_click(op(PageOp::Turn(1))),
                    )
                    .child(
                        item(
                            "thumb-menu-left",
                            "rotl",
                            pages("thumb-menu-turn-left", "thumb-menu-turn-left-many"),
                        )
                        .on_click(op(PageOp::Turn(-1))),
                    )
                    .when(page > 0, |d| {
                        d.child(
                            item("thumb-menu-earlier", "chevu", t("thumb-menu-earlier", &[]))
                                .on_click(op(PageOp::Earlier)),
                        )
                    })
                    .when(!last, |d| {
                        d.child(
                            item("thumb-menu-later", "down", t("thumb-menu-later", &[]))
                                .on_click(op(PageOp::Later)),
                        )
                    })
                    .child(float::menu_sep(&th))
                    .child(
                        item("thumb-menu-insert", "plus", t("thumb-menu-insert", &[])).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.side_menu = None;
                                this.choose_insert(cx);
                            }),
                        ),
                    )
                    .child(
                        item(
                            "thumb-menu-extract",
                            "download",
                            pages("thumb-menu-extract", "thumb-menu-extract-many"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.side_menu = None;
                            this.choose_extract(cx);
                        })),
                    )
                    .when(self.picked.len() < self.page_count(), |d| {
                        d.child(
                            item(
                                "thumb-menu-delete",
                                "trash",
                                pages("thumb-menu-delete", "thumb-menu-delete-many"),
                            )
                            .on_click(op(PageOp::Delete)),
                        )
                    });
                *at
            }
            SideMenu::Bookmark { at, path } => {
                let dest = self
                    .contents
                    .as_ref()
                    .and_then(|c| c.rows().into_iter().find(|r| &r.path == path))
                    .and_then(|r| r.dest);
                let change = |change: Change| {
                    cx.listener(move |this: &mut Viewer, _: &gpui::ClickEvent, _, cx| {
                        this.side_menu = None;
                        this.change_outline(change.clone(), cx);
                    })
                };
                if let Some(dest) = dest {
                    body = body.child(
                        item("bookmark-menu-go", "page", t("bookmark-menu-go", &[])).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.side_menu = None;
                                this.go_to(dest, cx);
                            }),
                        ),
                    );
                }
                body = body
                    .child(
                        item(
                            "bookmark-menu-rename",
                            "edit",
                            t("bookmark-menu-rename", &[]),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.side_menu = None;
                            this.start_renaming(window, cx);
                        })),
                    )
                    .child(
                        item("bookmark-menu-place", "fwd", t("bookmark-menu-place", &[])).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.side_menu = None;
                                let here = this.here();
                                this.change_outline(Change::Place(here), cx);
                            }),
                        ),
                    )
                    .child(
                        item("bookmark-menu-add", "plus", t("bookmark-menu-add", &[])).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.side_menu = None;
                                this.add_bookmark(window, cx);
                            }),
                        ),
                    )
                    .child(float::menu_sep(&th))
                    .child(
                        item("bookmark-menu-up", "chevu", t("bookmark-up", &[]))
                            .on_click(change(Change::Up)),
                    )
                    .child(
                        item("bookmark-menu-down", "down", t("bookmark-down", &[]))
                            .on_click(change(Change::Down)),
                    )
                    .child(
                        item(
                            "bookmark-menu-outdent",
                            "chevl",
                            t("bookmark-menu-outdent", &[]),
                        )
                        .on_click(change(Change::Outdent)),
                    )
                    .child(
                        item(
                            "bookmark-menu-indent",
                            "chevr",
                            t("bookmark-menu-indent", &[]),
                        )
                        .on_click(change(Change::Indent)),
                    )
                    .child(float::menu_sep(&th))
                    .child(
                        item(
                            "bookmark-menu-delete",
                            "trash",
                            t("bookmark-menu-delete", &[]),
                        )
                        .on_click(change(Change::Delete)),
                    );
                *at
            }
        };
        Some(
            gpui::deferred(
                katna_ui::anchored()
                    .position(at)
                    .snap_to_window()
                    .child(float::appear("side-menu-in", body, -4.0)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}
