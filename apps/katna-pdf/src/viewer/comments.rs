// SPDX-License-Identifier: GPL-3.0-or-later

//! The comments tab: the list of annotations with their replies under
//! them, each one's review status, lock, hidden and printed flags, a
//! filter by kind, author and status, deleting all on a page or in the
//! file, and the comments saved as text to read.

use std::path::Path;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, REVIEW_STATES, Ref, annotation_status};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};

use super::{PaneTab, Viewer};
use crate::annotate;
use crate::text::t;

/// A reply being typed to annotation `to`.
pub(super) struct Reply {
    pub to: Ref,
    pub input: Entity<TextInput>,
    pub _input: Subscription,
}

/// Which comments the list shows.
#[derive(Default)]
pub(super) struct CommentFilter {
    /// Whether the filter rows are shown.
    pub shown: bool,
    pub kind: Option<KindGroup>,
    pub author: Option<String>,
    pub status: Option<StatusPick>,
}

/// Kinds of annotation, as the filter groups them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum KindGroup {
    Notes,
    Text,
    Markup,
    Drawings,
    Stamps,
}

impl KindGroup {
    const ALL: [KindGroup; 5] = [
        KindGroup::Notes,
        KindGroup::Text,
        KindGroup::Markup,
        KindGroup::Drawings,
        KindGroup::Stamps,
    ];

    fn of(kind: &AnnotKind) -> Option<Self> {
        Some(match kind {
            AnnotKind::Note | AnnotKind::FileAttachment { .. } => KindGroup::Notes,
            AnnotKind::FreeText { .. } => KindGroup::Text,
            AnnotKind::Markup(..) => KindGroup::Markup,
            AnnotKind::Ink(_)
            | AnnotKind::Square { .. }
            | AnnotKind::Circle { .. }
            | AnnotKind::Line { .. }
            | AnnotKind::Polygon(_)
            | AnnotKind::PolyLine(_) => KindGroup::Drawings,
            AnnotKind::Stamp(_) => KindGroup::Stamps,
            AnnotKind::Other(_) => return None,
        })
    }

    fn message(self) -> &'static str {
        match self {
            KindGroup::Notes => "comments-filter-notes",
            KindGroup::Text => "comments-filter-text",
            KindGroup::Markup => "comments-filter-markup",
            KindGroup::Drawings => "comments-filter-drawings",
            KindGroup::Stamps => "comments-filter-stamps",
        }
    }
}

/// A status to show: none set yet, or one of the review states.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StatusPick {
    Open,
    Is(&'static str),
}

/// The message naming a review state.
fn state_message(state: &str) -> &'static str {
    match state {
        "Accepted" => "comments-status-accepted",
        "Rejected" => "comments-status-rejected",
        "Cancelled" => "comments-status-cancelled",
        _ => "comments-status-completed",
    }
}

impl Viewer {
    /// Whether annotation `a` passes the filter.
    fn passes(&self, a: &Annotation, all: &[Vec<Annotation>]) -> bool {
        let f = &self.comment_filter;
        if let Some(kind) = f.kind
            && KindGroup::of(&a.kind) != Some(kind)
        {
            return false;
        }
        if let Some(author) = &f.author
            && a.author != *author
        {
            return false;
        }
        if let Some(status) = f.status {
            let now = a.id.and_then(|id| annotation_status(all, id));
            match status {
                StatusPick::Open if now.is_some() => return false,
                StatusPick::Is(state) if now != Some(state) => return false,
                _ => {}
            }
        }
        true
    }

    pub(super) fn toggle_comment_filter(&mut self, cx: &mut Context<Self>) {
        self.comment_filter.shown = !self.comment_filter.shown;
        if !self.comment_filter.shown {
            self.comment_filter = Default::default();
        }
        cx.notify();
    }

    /// Starts typing a reply to the annotation chosen.
    fn start_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(to) = self.chosen else {
            return;
        };
        if self.reply.as_ref().is_some_and(|r| r.to == to) {
            return;
        }
        self.finish_comment(true, cx);
        self.finish_reply(true, cx);
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("comment-reply-placeholder", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_reply(true, cx),
            InputEvent::Cancel => this.finish_reply(false, cx),
            InputEvent::Changed => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        self.pane = true;
        self.tab = PaneTab::Comments;
        self.reply = Some(Reply {
            to,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing a reply, adding it or not. A reply is a note on what
    /// it answers, with no icon of its own.
    pub(super) fn finish_reply(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(reply) = self.reply.take() else {
            return;
        };
        cx.notify();
        let text = reply.input.read(cx).text().trim().to_owned();
        if !keep || text.is_empty() {
            return;
        }
        let Some(parent) = self
            .annots
            .as_ref()
            .and_then(|all| all.iter().flatten().find(|a| a.id == Some(reply.to)))
            .cloned()
        else {
            return;
        };
        let mut a = Annotation::new(parent.page, AnnotKind::Note);
        a.rect = parent.rect;
        a.colour = parent.colour;
        a.reply_to = parent.id;
        a.contents = text;
        a.author = super::annots::author();
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// Sets the chosen annotation's review status (`None` clears it): a
    /// hidden reply that carries the state, as Acrobat writes it.
    fn set_status(&mut self, state: &str, cx: &mut Context<Self>) {
        let Some(parent) = self.chosen_annotation().cloned() else {
            return;
        };
        let author = super::annots::author();
        let mut a = Annotation::new(parent.page, AnnotKind::Note);
        a.rect = parent.rect;
        a.reply_to = parent.id;
        a.state = Some(state.to_owned());
        a.contents = format!("{state} set by {author}");
        a.author = author;
        a.flags.hidden = true;
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// Changes the chosen annotation's flags with `change`.
    fn change_flags(
        &mut self,
        change: impl FnOnce(&mut katna_pdf_cos::AnnotFlags),
        cx: &mut Context<Self>,
    ) {
        let Some(mut a) = self.chosen_annotation().cloned() else {
            return;
        };
        change(&mut a.flags);
        if a.flags.locked {
            // Its comment may not be changed now.
            self.comment = None;
            self.reply = None;
        }
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// Deletes every annotation that is not locked, on `page` or on
    /// every page.
    fn delete_all(&mut self, page: Option<usize>, cx: &mut Context<Self>) {
        let Some(all) = &self.annots else {
            return;
        };
        let remove: Vec<Ref> = all
            .iter()
            .enumerate()
            .filter(|(p, _)| page.is_none_or(|page| page == *p))
            .flat_map(|(_, annots)| annots.iter())
            .filter(|a| !a.flags.locked)
            .filter_map(|a| a.id)
            .collect();
        if remove.is_empty() {
            return;
        }
        self.comment = None;
        self.reply = None;
        self.chosen = None;
        self.change_annotations(
            AnnotChanges {
                put: Vec::new(),
                remove,
            },
            cx,
        );
    }

    /// Asks where to save the comments as text, and saves them there.
    fn export_text(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(annots)) = (self.path.clone(), self.annots.clone()) else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let chosen = cx.prompt_for_new_path(&folder, Some(&format!("{stem}-comments.txt")));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(&target, katna_pdf_cos::comments_text(&annots))
                        .map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t(
                        "comments-text-saved",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => t("comments-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The buttons above the comments list.
    pub(super) fn comment_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let page = self.current_page();
        let button = |id: &'static str, icon: &'static str, tip: &str| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .size(px(32.0))
                .tip(t(tip, &[]), &th.katna)
        };
        let filtering = self.comment_filter.shown;
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                widgets::icon_button_colored(
                    "comments-filter",
                    "filter",
                    18.0,
                    if filtering {
                        th.katna.accent
                    } else {
                        th.katna.text_dim
                    },
                    &th.katna,
                )
                .size(px(32.0))
                .tip(t("comments-filter", &[]), &th.katna)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_comment_filter(cx))),
            )
            .child(
                button("comments-text", "document", "comments-text")
                    .on_click(cx.listener(|this, _, _, cx| this.export_text(cx))),
            )
            .child(
                button("comments-import", "upload", "comments-import")
                    .on_click(cx.listener(|this, _, _, cx| this.import_xfdf(cx))),
            )
            .child(
                button("comments-export", "download", "comments-export")
                    .on_click(cx.listener(|this, _, _, cx| this.export_xfdf(cx))),
            )
            .child(
                button("comments-delete-page", "trash", "comments-delete-page")
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_all(Some(page), cx))),
            )
            .child(
                button("comments-delete-all", "close", "comments-delete-all")
                    .on_click(cx.listener(|this, _, _, cx| this.delete_all(None, cx))),
            )
            .into_any_element()
    }

    /// A small chip, filled when `on`.
    fn chip(
        &self,
        id: impl Into<gpui::ElementId>,
        label: String,
        on: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let th = &self.theme.katna;
        let chip = if on {
            widgets::filled_button(id, label, th)
        } else {
            widgets::outlined_button(id, label, th)
        };
        chip.h(px(26.0))
            .px(px(space::S3))
            .text_size(px(text::SMALL))
    }

    /// The rows of filter chips: kinds, authors (when there are several)
    /// and statuses.
    fn filter_rows(&self, all: &[Vec<Annotation>], cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let row = || {
            div()
                .flex()
                .flex_wrap()
                .gap(px(space::S1))
                .px(px(space::S2))
                .py(px(space::S1))
        };
        let f = &self.comment_filter;
        let mut kinds = row().child(
            self.chip(
                "filter-kind-all",
                t("comments-filter-kind-all", &[]),
                f.kind.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.comment_filter.kind = None;
                cx.notify();
            })),
        );
        for kind in KindGroup::ALL {
            kinds = kinds.child(
                self.chip(kind.message(), t(kind.message(), &[]), f.kind == Some(kind))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.comment_filter.kind = Some(kind);
                        cx.notify();
                    })),
            );
        }
        let mut authors: Vec<&str> = all
            .iter()
            .flatten()
            .filter(|a| a.state.is_none() && !a.author.is_empty())
            .map(|a| a.author.as_str())
            .collect();
        authors.sort_unstable();
        authors.dedup();
        let mut by = row().child(
            self.chip(
                "filter-author-all",
                t("comments-filter-author-all", &[]),
                f.author.is_none(),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.comment_filter.author = None;
                cx.notify();
            })),
        );
        for (i, author) in authors.iter().enumerate() {
            let name = (*author).to_owned();
            let on = f.author.as_deref() == Some(author);
            by = by.child(
                self.chip(("filter-author", i), name.clone(), on)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.comment_filter.author = Some(name.clone());
                        cx.notify();
                    })),
            );
        }
        let mut statuses = row()
            .child(
                self.chip(
                    "filter-status-all",
                    t("comments-filter-status-all", &[]),
                    f.status.is_none(),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.comment_filter.status = None;
                    cx.notify();
                })),
            )
            .child(
                self.chip(
                    "filter-status-open",
                    t("comments-filter-open", &[]),
                    f.status == Some(StatusPick::Open),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.comment_filter.status = Some(StatusPick::Open);
                    cx.notify();
                })),
            );
        for (i, state) in REVIEW_STATES.into_iter().enumerate() {
            statuses = statuses.child(
                self.chip(
                    ("filter-status", i),
                    t(state_message(state), &[]),
                    f.status == Some(StatusPick::Is(state)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.comment_filter.status = Some(StatusPick::Is(state));
                    cx.notify();
                })),
            );
        }
        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .py(px(space::S1))
            .border_b_1()
            .border_color(th.line)
            .child(kinds)
            .when(authors.len() > 1, |d| d.child(by))
            .child(statuses)
            .into_any_element()
    }

    /// The comments list: each annotation with its replies under it.
    pub(super) fn comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(all) = self.annots.clone() else {
            return div().into_any_element();
        };
        let mut list = div()
            .id("comments-list")
            .size_full()
            .overflow_y_scroll()
            .py(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1));
        let mut any = false;
        let mut hidden_by_filter = 0;
        for (page, annots) in all.iter().enumerate() {
            for a in annots {
                let Some(id) = a.id else {
                    continue;
                };
                if a.reply_to.is_some() || a.state.is_some() {
                    continue;
                }
                any = true;
                if !self.passes(a, &all) {
                    hidden_by_filter += 1;
                    continue;
                }
                let status = annotation_status(&all, id);
                list = list.child(self.comment_row(a, page, status, false, window, cx));
                // Replies, and replies to them, in the file's order.
                let mut parents = vec![id];
                let mut shown = Vec::new();
                while let Some(parent) = parents.pop() {
                    for reply in annots
                        .iter()
                        .filter(|r| r.state.is_none() && r.reply_to == Some(parent))
                    {
                        if let Some(rid) = reply.id
                            && !shown.contains(&rid)
                        {
                            shown.push(rid);
                            parents.push(rid);
                            list =
                                list.child(self.comment_row(reply, page, None, true, window, cx));
                        }
                    }
                }
            }
        }
        if !any {
            list = list.child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S4))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("comments-none", &[])),
            );
        } else if hidden_by_filter > 0 {
            list = list.child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S2))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t(
                        "comments-filter-hides",
                        &[("n", &hidden_by_filter.to_string())],
                    )),
            );
        }
        let filters = self
            .comment_filter
            .shown
            .then(|| self.filter_rows(&all, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(filters)
            .child(list)
            .into_any_element()
    }

    /// One comment in the list; `reply` ones sit indented under what
    /// they answer. The chosen one shows its comment being typed, its
    /// status chips and its buttons.
    #[allow(clippy::too_many_arguments)]
    fn comment_row(
        &self,
        a: &Annotation,
        page: usize,
        status: Option<&str>,
        reply: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Some(id) = a.id else {
            return div().into_any_element();
        };
        let chosen = self.chosen == Some(id);
        let page_label = (page + 1).to_string();
        let what = if !a.contents.is_empty() {
            a.contents.clone()
        } else {
            match &a.kind {
                AnnotKind::Stamp(name) => katna_pdf_cos::stamp_label(name),
                AnnotKind::FileAttachment { name, .. } => name.clone(),
                kind => t(annotate::kind_message(kind), &[]),
            }
        };
        let mut meta = t("comment-page", &[("page", &page_label)]);
        if !a.author.is_empty() {
            meta = format!("{meta} · {}", a.author);
        }
        if let Some(status) = status {
            meta = format!("{meta} · {}", t(state_message(status), &[]));
        }
        let [r, g, b] = a.colour;
        let swatch = gpui::rgb(u32::from_be_bytes([
            0,
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
        ]));
        let mark = |name: &'static str| {
            div()
                .flex_none()
                .child(widgets::icon(name, th.katna.text_dim, 14.0))
        };
        let mut row = div()
            .id(("comment", id.num as usize))
            .ml(px(if reply { space::S6 } else { space::S2 }))
            .mr(px(space::S2))
            .px(px(space::S3))
            .py(px(space::S2))
            .rounded(px(radius::SM))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .cursor_pointer()
            .when(chosen, |d| d.bg(th.field))
            .hover(move |s| s.bg(th.hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose(Some(id), cx);
                this.edit_comment(window, cx);
                this.go_to(katna_pdf_cos::Destination { page, top: None }, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .when(!reply, |d| {
                        d.child(div().size(px(10.0)).flex_none().rounded_full().bg(swatch))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(what),
                    )
                    .when(a.flags.locked, |d| d.child(mark("lock")))
                    .when(a.flags.hidden, |d| d.child(mark("eye-off")))
                    .when(!a.flags.print, |d| d.child(mark("print").opacity(0.4))),
            )
            .child(
                div()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(meta),
            );
        if !chosen {
            return row.into_any_element();
        }
        let typing = |input: &Entity<TextInput>| {
            div()
                .px(px(space::S2))
                .py(px(space::S1))
                .rounded(px(radius::XS))
                .bg(th.card)
                .border_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(input.clone())
        };
        if let Some(comment) = self.comment.as_ref().filter(|c| c.id == id) {
            row = row.child(typing(&comment.input));
        }
        if let Some(reply) = self.reply.as_ref().filter(|r| r.to == id) {
            row = row.child(typing(&reply.input));
        }
        if !reply {
            let mut chips = div().flex().flex_wrap().gap(px(space::S1));
            for (i, state) in REVIEW_STATES.into_iter().enumerate() {
                let on = status == Some(state);
                chips = chips.child(
                    self.chip(("status", i), t(state_message(state), &[]), on)
                        .tip(t("comments-status-tip", &[]), &th.katna)
                        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_status(if on { "None" } else { state }, cx);
                        })),
                );
            }
            row = row.child(chips);
        }
        let locked = a.flags.locked;
        let button = |id: &'static str, icon: &'static str, tip: &str| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .tip(t(tip, &[]), &th.katna)
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };
        let attachment = matches!(a.kind, AnnotKind::FileAttachment { .. });
        row = row.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(space::S1))
                .when(attachment, |d| {
                    d.child(
                        button("comment-save-file", "download", "comment-save-file")
                            .on_click(cx.listener(|this, _, _, cx| this.save_attachment(cx))),
                    )
                })
                .child(
                    button("comment-reply", "reply", "comment-reply")
                        .on_click(cx.listener(|this, _, window, cx| this.start_reply(window, cx))),
                )
                .child(
                    button(
                        "comment-lock",
                        "lock",
                        if locked {
                            "comment-unlock"
                        } else {
                            "comment-lock"
                        },
                    )
                    .when(!locked, |b| b.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.change_flags(|f| f.locked = !locked, cx)
                    })),
                )
                .child(
                    button(
                        "comment-hide",
                        if a.flags.hidden { "eye-off" } else { "eye" },
                        if a.flags.hidden {
                            "comment-show"
                        } else {
                            "comment-hide"
                        },
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.change_flags(|f| f.hidden = !f.hidden, cx)
                        }),
                    ),
                )
                .child(
                    button(
                        "comment-print",
                        "print",
                        if a.flags.print {
                            "comment-print"
                        } else {
                            "comment-no-print"
                        },
                    )
                    .when(!a.flags.print, |b| b.opacity(0.5))
                    .on_click(
                        cx.listener(|this, _, _, cx| this.change_flags(|f| f.print = !f.print, cx)),
                    ),
                )
                .child(
                    button("comment-delete", "trash", "comment-delete")
                        .when(locked, |b| b.opacity(0.4))
                        .on_click(cx.listener(|this, _, _, cx| this.delete_chosen(cx))),
                ),
        );
        row.into_any_element()
    }
}
