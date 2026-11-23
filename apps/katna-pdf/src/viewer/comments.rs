// SPDX-License-Identifier: GPL-3.0-or-later

//! The comments tab: the list of annotations with their replies under
//! them, each one's review status, lock, hidden and printed flags, a
//! filter by kind, author, status and words in them, deleting all on a page or in the
//! file, and the comments saved as text to read.

use std::path::Path;

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, REVIEW_STATES, Ref, annotation_status};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::{PaneTab, Viewer};
use crate::float::{self, FloatTip};
use crate::text::t;
use crate::theme::Theme;

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
    /// Words to look for in the comments, their replies and authors.
    pub words: Option<(Entity<TextInput>, Subscription)>,
    /// The order of the list; kept when the filter closes.
    pub sort: SortBy,
}

/// The order the comments list goes in.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SortBy {
    /// Page by page, in the file's order on each.
    #[default]
    Page,
    /// By author's name, then page.
    Author,
    /// Newest change first; those with no date last.
    Date,
    /// By kind (notes, text, marked text, drawings, stamps), then page.
    Kind,
    /// By colour, so each colour's comments come together, then page.
    Colour,
}

impl SortBy {
    const ALL: [SortBy; 5] = [
        SortBy::Page,
        SortBy::Author,
        SortBy::Date,
        SortBy::Kind,
        SortBy::Colour,
    ];

    fn message(self) -> &'static str {
        match self {
            SortBy::Page => "comments-sort-page",
            SortBy::Author => "comments-sort-author",
            SortBy::Date => "comments-sort-date",
            SortBy::Kind => "comments-sort-kind",
            SortBy::Colour => "comments-sort-colour",
        }
    }

    /// Puts `list` (page, comment) in this order; ties keep the file's.
    fn apply(self, list: &mut [(usize, &Annotation)]) {
        match self {
            SortBy::Page => {}
            SortBy::Author => list.sort_by_cached_key(|(_, a)| {
                (a.author.trim().is_empty(), a.author.trim().to_lowercase())
            }),
            SortBy::Date => list.sort_by_cached_key(|(_, a)| {
                std::cmp::Reverse(a.modified.as_deref().and_then(pdf_time))
            }),
            SortBy::Kind => list.sort_by_key(|(_, a)| {
                KindGroup::of(&a.kind)
                    .and_then(|k| KindGroup::ALL.iter().position(|g| *g == k))
                    .unwrap_or(KindGroup::ALL.len())
            }),
            SortBy::Colour => list.sort_by_key(|(_, a)| colour_key(a.colour)),
        }
    }
}

/// A colour as hue first, so reds, yellows, greens and blues each come
/// together; greys last, lightest first.
fn colour_key([r, g, b]: [f32; 3]) -> (u8, u16, u16) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let chroma = max - min;
    if chroma < 0.08 {
        return (1, 0, ((1.0 - max) * 1000.0) as u16);
    }
    let hue = if max == r {
        ((g - b) / chroma).rem_euclid(6.0)
    } else if max == g {
        (b - r) / chroma + 2.0
    } else {
        (r - g) / chroma + 4.0
    };
    (0, (hue * 60.0) as u16, ((1.0 - max) * 1000.0) as u16)
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

/// Whether `words` are in annotation `a` or its author, or in
/// any reply to it on its page, matched as the find bar matches (case and
/// accents ignored; "in quotes" for whole words).
fn thread_says(a: &Annotation, annots: &[Annotation], words: &str) -> bool {
    let says = |a: &Annotation| {
        let text = format!("{}\n{}", a.contents, a.author);
        let page = katna_pdf_render::text::PageText {
            boxes: vec![[0.0; 4]; text.chars().count()],
            text,
        };
        !page.find(words).is_empty()
    };
    let mut thread = vec![a];
    let mut i = 0;
    while let Some(&at) = thread.get(i) {
        if says(at) {
            return true;
        }
        if let Some(id) = at.id {
            thread.extend(annots.iter().filter(|r| r.reply_to == Some(id)));
        }
        i += 1;
    }
    false
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

    /// The words the comments are searched for, if any.
    fn comment_words(&self, cx: &Context<Self>) -> Option<String> {
        let (input, _) = self.comment_filter.words.as_ref()?;
        let words = input.read(cx).text().trim().to_owned();
        (!words.is_empty()).then_some(words)
    }

    pub(super) fn toggle_comment_filter(&mut self, cx: &mut Context<Self>) {
        self.comment_filter.shown = !self.comment_filter.shown;
        if !self.comment_filter.shown {
            self.comment_filter = CommentFilter {
                sort: self.comment_filter.sort,
                ..Default::default()
            };
        } else {
            let accent = gpui::rgba(self.theme.katna.accent).into();
            let input = cx.new(|cx| {
                let mut input = TextInput::new(t("comments-search", &[]), cx);
                input.set_accent(accent);
                input
            });
            let sub = cx.subscribe(&input, |this, _, event, cx| match event {
                InputEvent::Cancel => this.toggle_comment_filter(cx),
                InputEvent::Submit | InputEvent::Changed => cx.notify(),
            });
            self.comment_filter.words = Some((input, sub));
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
                    Ok(target) => super::said(
                        "comments-text-saved",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => super::said("comments-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The head of the comments tab (C03): how many there are, and a menu
    /// of what can be done with all of them.
    pub(super) fn comment_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let count = self
            .annots
            .iter()
            .flat_map(|all| all.iter().flatten())
            .filter(|a| a.id.is_some() && a.reply_to.is_none() && a.state.is_none())
            .count();
        div()
            .h(px(40.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(space::S3))
            .pr(px(space::S2))
            .child(
                div()
                    .text_size(px(text::MICRO))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(th.dim)
                    .child(t("comments-head", &[("n", &count.to_string())]).to_uppercase()),
            )
            .child(
                div()
                    .relative()
                    .child(
                        float::sized_ib(
                            "comments-more",
                            "more",
                            28.0,
                            16.0,
                            t("comments-more", &[]),
                            self.comments_menu,
                            &th,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.comments_menu = !this.comments_menu;
                            cx.notify();
                        })),
                    )
                    .when(self.comments_menu, |d| {
                        d.child(float::popover(self.comments_menu_card(cx), false, false))
                    }),
            )
            .into_any_element()
    }

    /// The menu of the comments tab: the filter, saving and bringing in,
    /// and deleting all.
    fn comments_menu_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let page = self.current_page();
        let item = |id: &'static str, glyph: &'static str| {
            float::menu_item(id, Some(glyph), t(id, &[]), None, &th)
        };
        let hover = th.hover;
        let danger = |id: &'static str| {
            div()
                .id(id)
                .h(px(32.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .rounded(px(5.0))
                .text_size(px(text::SMALL))
                .text_color(th.error)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(float::icon("trash", 16.0, th.error))
                .child(t(id, &[]))
        };
        let close = |this: &mut Viewer| this.comments_menu = false;
        float::menu("comments-menu", 280.0, &th)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.comments_menu = false;
                cx.notify();
            }))
            .child(float::heading(
                t("comments-menu-head", &[]).to_uppercase(),
                &th,
            ))
            .child(item("comments-menu-filter", "filter").on_click(cx.listener(
                move |this, _, _, cx| {
                    close(this);
                    this.toggle_comment_filter(cx);
                },
            )))
            .child(item("comments-menu-text", "file").on_click(cx.listener(
                move |this, _, _, cx| {
                    close(this);
                    this.export_text(cx);
                },
            )))
            .child(item("comments-menu-export", "upload").on_click(cx.listener(
                move |this, _, _, cx| {
                    close(this);
                    this.export_xfdf(cx);
                },
            )))
            .child(
                item("comments-menu-import", "download").on_click(cx.listener(
                    move |this, _, _, cx| {
                        close(this);
                        this.import_xfdf(cx);
                    },
                )),
            )
            .child(float::menu_sep(&th))
            .child(danger("comments-menu-delete-page").on_click(cx.listener(
                move |this, _, _, cx| {
                    close(this);
                    this.delete_all(Some(page), cx);
                },
            )))
            .child(danger("comments-menu-delete-all").on_click(cx.listener(
                move |this, _, _, cx| {
                    close(this);
                    this.delete_all(None, cx);
                },
            )))
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
        let mut order = row().child(
            div()
                .h(px(26.0))
                .flex()
                .items_center()
                .pr(px(space::S1))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("comments-sort", &[])),
        );
        for (i, sort) in SortBy::ALL.into_iter().enumerate() {
            order = order.child(
                self.chip(("comments-sort", i), t(sort.message(), &[]), f.sort == sort)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.comment_filter.sort = sort;
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
            .children(f.words.as_ref().map(|(input, _)| {
                div().px(px(space::S2)).py(px(space::S1)).child(
                    div()
                        .h(px(30.0))
                        .flex()
                        .items_center()
                        .px(px(space::S2))
                        .rounded(px(radius::SM))
                        .border_1()
                        .border_color(th.line)
                        .bg(th.card)
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(input.clone()),
                )
            }))
            .child(kinds)
            .when(authors.len() > 1, |d| d.child(by))
            .child(statuses)
            .child(order)
            .into_any_element()
    }

    /// The quick chips over the list: all, the ones with no status yet,
    /// and each author's.
    fn quick_chips(&self, all: &[Vec<Annotation>], cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let f = &self.comment_filter;
        let mut authors: Vec<String> = all
            .iter()
            .flatten()
            .filter(|a| a.state.is_none() && a.reply_to.is_none() && !a.author.is_empty())
            .map(|a| a.author.clone())
            .collect();
        authors.sort_unstable();
        authors.dedup();
        let none = f.kind.is_none() && f.author.is_none() && f.status.is_none();
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap(px(space::S1))
            .px(px(space::S3))
            .pb(px(space::S2))
            .child(
                float::chip("comments-all", t("comments-all", &[]), none, &th).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.comment_filter.kind = None;
                        this.comment_filter.author = None;
                        this.comment_filter.status = None;
                        cx.notify();
                    }),
                ),
            )
            .child(
                float::chip(
                    "comments-open",
                    t("comments-open", &[]),
                    f.status == Some(StatusPick::Open),
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    let f = &mut this.comment_filter;
                    f.status = (f.status != Some(StatusPick::Open)).then_some(StatusPick::Open);
                    cx.notify();
                })),
            );
        for (i, author) in authors.into_iter().enumerate() {
            let on = f.author.as_deref() == Some(author.as_str());
            row = row.child(
                float::chip(("comments-by", i), author.clone(), on, &th).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.comment_filter.author = (!on).then(|| author.clone());
                        cx.notify();
                    },
                )),
            );
        }
        row.into_any_element()
    }

    /// The comments list: a card for each, its replies counted under it
    /// and shown when it is chosen.
    pub(super) fn comments(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(all) = self.annots.clone() else {
            return div().into_any_element();
        };
        let mut list = div()
            .id("comments-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(space::S2))
            .pb(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S2));
        let mut any = false;
        let mut hidden_by_filter = 0;
        let words = self.comment_words(cx);
        let mut tops: Vec<(usize, &Annotation)> = all
            .iter()
            .enumerate()
            .flat_map(|(page, annots)| annots.iter().map(move |a| (page, a)))
            .filter(|(_, a)| a.id.is_some() && a.reply_to.is_none() && a.state.is_none())
            .collect();
        self.comment_filter.sort.apply(&mut tops);
        for (page, a) in tops {
            let annots = &all[page];
            let Some(id) = a.id else {
                continue;
            };
            any = true;
            if !self.passes(a, &all) || words.as_deref().is_some_and(|w| !thread_says(a, annots, w))
            {
                hidden_by_filter += 1;
                continue;
            }
            // Replies, and replies to them, in the file's order.
            let mut parents = vec![id];
            let mut replies: Vec<Annotation> = Vec::new();
            while let Some(parent) = parents.pop() {
                for reply in annots
                    .iter()
                    .filter(|r| r.state.is_none() && r.reply_to == Some(parent))
                {
                    if let Some(rid) = reply.id
                        && !replies.iter().any(|r| r.id == Some(rid))
                    {
                        parents.push(rid);
                        replies.push(reply.clone());
                    }
                }
            }
            let status = annotation_status(&all, id);
            list = list.child(self.comment_card(a, page, status, &replies, window, cx));
        }
        if !any {
            list = list.child(
                div()
                    .px(px(space::S2))
                    .py(px(space::S4))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("comments-none", &[])),
            );
        } else if hidden_by_filter > 0 {
            list = list.child(
                div()
                    .px(px(space::S2))
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
        let chips = (!self.comment_filter.shown && any).then(|| self.quick_chips(&all, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(filters)
            .children(chips)
            .child(list)
            .into_any_element()
    }

    /// One comment's card. The chosen one opens: its comment being
    /// typed, its replies, a reply box, its status and its buttons.
    fn comment_card(
        &self,
        a: &Annotation,
        page: usize,
        status: Option<&str>,
        replies: &[Annotation],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Some(id) = a.id else {
            return div().into_any_element();
        };
        let chosen = self.chosen == Some(id);
        let [r, g, b] = a.colour;
        let tone: gpui::Hsla = gpui::rgb(u32::from_be_bytes([
            0,
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
        ]))
        .into();
        // Notes are people talking: their name by a round mark. The rest
        // are marks on the page: their kind by its icon.
        let talk = matches!(
            a.kind,
            AnnotKind::Note | AnnotKind::FreeText { .. } | AnnotKind::FileAttachment { .. }
        );
        let who = if talk && !a.author.is_empty() {
            a.author.clone()
        } else if talk {
            t("comment-someone", &[])
        } else {
            t(kind_name(&a.kind), &[])
        };
        let mark = if talk {
            let initial = who.chars().next().unwrap_or('?').to_uppercase().to_string();
            div()
                .size(px(18.0))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(tone)
                .text_size(px(10.0))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(if tone.l > 0.6 {
                    gpui::black()
                } else {
                    gpui::white()
                })
                .child(initial)
        } else {
            div()
                .flex_none()
                .child(float::icon(kind_icon(&a.kind), 15.0, tone))
        };
        let mut meta = t("comment-page-short", &[("page", &(page + 1).to_string())]);
        if let Some(age) = a.modified.as_deref().and_then(age) {
            meta = format!("{meta} · {age}");
        }
        let what = if !a.contents.is_empty() {
            if talk {
                a.contents.clone()
            } else {
                format!("“{}”", a.contents)
            }
        } else {
            match &a.kind {
                AnnotKind::Stamp(name) => katna_pdf_cos::stamp_label(name),
                AnnotKind::FileAttachment { name, .. } => name.clone(),
                _ => String::new(),
            }
        };
        let flag = |name: &'static str| float::icon(name, 13.0, th.dim);
        let pill = status_pill(status, talk, &th);
        let reply_count = (!replies.is_empty()).then(|| {
            div()
                .text_size(px(text::CAPTION))
                .text_color(th.dim)
                .child(if replies.len() == 1 {
                    t("comment-one-reply", &[])
                } else {
                    t("comment-replies", &[("n", &replies.len().to_string())])
                })
        });
        let mut card = div()
            .id(("comment", id.num as usize))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(px(radius::SM))
            .border_1()
            .border_color(if chosen { th.accent } else { th.line })
            .bg(th.card)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, window, cx| {
                this.choose(Some(id), cx);
                this.edit_comment(window, cx);
                this.go_to(katna_pdf_cos::Destination { page, top: None }, cx);
            }))
            .child(
                div()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(mark)
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(120.0))
                                    .truncate()
                                    .text_size(px(text::SMALL))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(who),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(meta),
                            )
                            .when(a.flags.locked, |d| d.child(flag("lock")))
                            .when(a.flags.hidden, |d| d.child(flag("eyeoff")))
                            .when(!a.flags.print, |d| d.child(flag("print"))),
                    )
                    .when(!what.is_empty() && !chosen, |d| {
                        d.child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(if talk { th.text } else { th.text2 })
                                .child(what.clone()),
                        )
                    })
                    .when(pill.is_some() || reply_count.is_some(), |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .children(pill)
                                .children(reply_count),
                        )
                    }),
            );
        if !chosen {
            return card.into_any_element();
        }
        let field = |input: &Entity<TextInput>| {
            div()
                .flex_1()
                .min_w_0()
                .h(px(30.0))
                .flex()
                .items_center()
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .bg(th.card)
                .border_1()
                .border_color(th.accent)
                .text_size(px(text::SMALL))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(input.clone())
        };
        let mut open = div()
            .px(px(space::S3))
            .pb(px(space::S3))
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation());
        match self.comment.as_ref().filter(|c| c.id == id) {
            Some(comment) => open = open.child(div().flex().child(field(&comment.input))),
            None if !what.is_empty() => {
                open = open.child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.text)
                        .child(what),
                )
            }
            None => {}
        }
        for reply in replies {
            let mut head = reply.author.clone();
            if head.is_empty() {
                head = t("comment-someone", &[]);
            }
            open = open.child(
                div()
                    .ml(px(space::S2))
                    .pl(px(space::S3))
                    .border_l_2()
                    .border_color(th.line)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .text_size(px(text::CAPTION))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(th.text)
                                    .child(head),
                            )
                            .children(reply.modified.as_deref().and_then(age).map(|age| {
                                div()
                                    .text_size(px(text::CAPTION))
                                    .text_color(th.dim)
                                    .child(age)
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .child(reply.contents.clone()),
                    ),
            );
        }
        let replying = self.reply.as_ref().filter(|r| r.to == id);
        let hover = th.hover;
        let reply_box = match replying {
            Some(reply) => field(&reply.input).into_any_element(),
            None => div()
                .id("comment-reply-box")
                .flex_1()
                .h(px(30.0))
                .flex()
                .items_center()
                .px(px(space::S2))
                .rounded(px(radius::SM))
                .border_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .cursor_text()
                .hover(move |s| s.bg(hover))
                .child(t("comment-reply-placeholder", &[]))
                .on_click(cx.listener(|this, _, window, cx| this.start_reply(window, cx)))
                .into_any_element(),
        };
        let typing = replying.is_some();
        open = open.child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(reply_box)
                .child(
                    float::button("comment-reply", t("comment-reply", &[]), true, &th)
                        .h(px(30.0))
                        .px(px(space::S3))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if typing {
                                this.finish_reply(true, cx);
                            } else {
                                this.start_reply(window, cx);
                            }
                        })),
                ),
        );
        if talk || status.is_some() {
            let mut chips = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::S1))
                .child(
                    div()
                        .mr(px(space::S1))
                        .text_size(px(text::CAPTION))
                        .text_color(th.dim)
                        .child(t("comments-status", &[])),
                )
                .child(
                    float::chip(
                        "status-none",
                        t("comments-status-none", &[]),
                        status.is_none(),
                        &th,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.set_status("None", cx))),
                );
            for (i, state) in REVIEW_STATES.into_iter().enumerate() {
                let on = status == Some(state);
                chips = chips.child(
                    float::chip(("status", i), t(state_message(state), &[]), on, &th)
                        .ftip(t("comments-status-tip", &[]), &th)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_status(if on { "None" } else { state }, cx);
                        })),
                );
            }
            open = open.child(chips);
        }
        let locked = a.flags.locked;
        let button = |id: &'static str, icon: &'static str, tip: &str, on: bool| {
            float::sized_ib(id, icon, 28.0, 15.0, t(tip, &[]), on, &th)
        };
        let attachment = matches!(a.kind, AnnotKind::FileAttachment { .. });
        open =
            open.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .pt(px(space::S1))
                    .border_t_1()
                    .border_color(th.line_soft)
                    .when(attachment, |d| {
                        d.child(
                            button("comment-save-file", "download", "comment-save-file", false)
                                .on_click(cx.listener(|this, _, _, cx| this.save_attachment(cx))),
                        )
                    })
                    .when(!a.contents.trim().is_empty(), |d| {
                        let words = a.contents.clone();
                        d.child(
                            button("comment-copy", "copy", "comment-copy", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                        words.clone(),
                                    ));
                                    this.notice = Some(super::said("comment-copied", &[]));
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .child(
                        button(
                            "comment-lock",
                            if locked { "lock" } else { "unlock" },
                            if locked {
                                "comment-unlock"
                            } else {
                                "comment-lock"
                            },
                            locked,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_flags(|f| f.locked = !locked, cx)
                        })),
                    )
                    .child(
                        button(
                            "comment-hide",
                            if a.flags.hidden { "eyeoff" } else { "eye" },
                            if a.flags.hidden {
                                "comment-show"
                            } else {
                                "comment-hide"
                            },
                            a.flags.hidden,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.change_flags(|f| f.hidden = !f.hidden, cx)
                        })),
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
                            !a.flags.print,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.change_flags(|f| f.print = !f.print, cx)
                        })),
                    )
                    .when(super::looks::tool_of(a).is_some(), |d| {
                        let kept = self.look_is_kept(a);
                        d.child(
                            button(
                                "comment-look",
                                "star",
                                if kept {
                                    "comment-look-drop"
                                } else {
                                    "comment-look-keep"
                                },
                                kept,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_kept_look(cx))),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        button("comment-delete", "trash", "comment-delete", false)
                            .when(locked, |b| b.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| this.delete_chosen(cx))),
                    ),
            );
        card = card.child(open);
        card.into_any_element()
    }
}

/// A comment's status as a small coloured pill: none for marks on the
/// page with no status, "Open" for notes with none yet.
fn status_pill(status: Option<&str>, talk: bool, th: &Theme) -> Option<gpui::Div> {
    let (words, hue) = match status {
        Some(state @ ("Accepted" | "Completed")) => (t(state_message(state), &[]), 0.39),
        Some(state) => (t(state_message(state), &[]), 0.0),
        None if talk => (t("comments-open", &[]), 0.11),
        None => return None,
    };
    let (fg, bg) = if th.dark {
        (
            gpui::hsla(hue, 0.7, 0.7, 1.0),
            gpui::hsla(hue, 0.6, 0.5, 0.18),
        )
    } else {
        (
            gpui::hsla(hue, 0.75, 0.3, 1.0),
            gpui::hsla(hue, 0.8, 0.55, 0.18),
        )
    };
    Some(
        div()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(4.0))
            .bg(bg)
            .text_size(px(text::CAPTION))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(fg)
            .child(words),
    )
}

/// The icon of an annotation's kind, as its tool shows it.
fn kind_icon(kind: &AnnotKind) -> &'static str {
    use katna_pdf_cos::MarkupStyle;
    match kind {
        AnnotKind::Markup(MarkupStyle::Highlight, _) => "mark",
        AnnotKind::Markup(MarkupStyle::Underline, _) => "under",
        AnnotKind::Markup(MarkupStyle::StrikeOut, _) => "strike",
        AnnotKind::Markup(MarkupStyle::Squiggly, _) => "squig",
        AnnotKind::Note => "note",
        AnnotKind::FreeText { .. } => "text",
        AnnotKind::Ink(_) => "pen",
        AnnotKind::Square { .. } => "sq",
        AnnotKind::Circle { .. } => "circ",
        AnnotKind::Line { arrow: false, .. } => "line",
        AnnotKind::Line { arrow: true, .. } => "arrow",
        AnnotKind::Polygon(_) | AnnotKind::PolyLine(_) => "polygon",
        AnnotKind::Stamp(_) => "stamp",
        AnnotKind::FileAttachment { .. } => "clip",
        AnnotKind::Other(_) => "comment",
    }
}

/// The short name of an annotation's kind.
fn kind_name(kind: &AnnotKind) -> &'static str {
    use katna_pdf_cos::MarkupStyle;
    match kind {
        AnnotKind::Markup(MarkupStyle::Highlight, _) => "tool-highlight-name",
        AnnotKind::Markup(MarkupStyle::Underline, _) => "tool-underline-name",
        AnnotKind::Markup(MarkupStyle::StrikeOut, _) => "tool-strikeout-name",
        AnnotKind::Markup(MarkupStyle::Squiggly, _) => "tool-squiggly-name",
        AnnotKind::Note => "tool-note-name",
        AnnotKind::FreeText { .. } => "tool-free-text-name",
        AnnotKind::Ink(_) => "tool-ink-name",
        AnnotKind::Square { .. } => "tool-rectangle-name",
        AnnotKind::Circle { .. } => "tool-ellipse-name",
        AnnotKind::Line { arrow: false, .. } => "tool-line-name",
        AnnotKind::Line { arrow: true, .. } => "tool-arrow-name",
        AnnotKind::Polygon(_) | AnnotKind::PolyLine(_) => "tool-polygon-name",
        AnnotKind::Stamp(_) => "tool-stamp-name",
        AnnotKind::FileAttachment { .. } => "tool-attach-name",
        AnnotKind::Other(_) => "annot-other",
    }
}

/// How long ago a PDF date (`D:YYYYMMDDHHmmSS` and its offset) was:
/// "now", minutes, hours, "yesterday", else the day.
fn age(date: &str) -> Option<String> {
    let then = pdf_time(date)?;
    let secs = jiff::Timestamp::now().as_second() - then.as_second();
    Some(match secs {
        ..60 => t("comment-age-now", &[]),
        60..3600 => t("comment-age-min", &[("n", &(secs / 60).to_string())]),
        3600..86_400 => t("comment-age-h", &[("n", &(secs / 3600).to_string())]),
        86_400..172_800 => t("comment-age-yesterday", &[]),
        _ => then
            .to_zoned(jiff::tz::TimeZone::system())
            .strftime("%-d %b %Y")
            .to_string(),
    })
}

/// A PDF date as a moment: `D:YYYYMMDDHHmmSS` then `Z`, `+HH'mm'` or
/// `-HH'mm'` (none read as UTC).
fn pdf_time(date: &str) -> Option<jiff::Timestamp> {
    let d = date.strip_prefix("D:").unwrap_or(date);
    let digits: String = d.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() < 8 {
        return None;
    }
    let n = |from: usize, len: usize, or: i32| -> i32 {
        digits
            .get(from..from + len)
            .and_then(|s| s.parse().ok())
            .unwrap_or(or)
    };
    let civil = jiff::civil::DateTime::new(
        n(0, 4, 1970) as i16,
        n(4, 2, 1) as i8,
        n(6, 2, 1) as i8,
        n(8, 2, 0) as i8,
        n(10, 2, 0) as i8,
        n(12, 2, 0) as i8,
        0,
    )
    .ok()?;
    let rest = &d[digits.len()..];
    let offset = match rest.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let num: Vec<i32> = rest[1..]
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            let secs =
                num.first().copied().unwrap_or(0) * 3600 + num.get(1).copied().unwrap_or(0) * 60;
            if sign == '-' { -secs } else { secs }
        }
        _ => 0,
    };
    let offset = jiff::tz::Offset::from_seconds(offset).ok()?;
    offset.to_timestamp(civil).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_dates_read_with_their_offset() {
        let utc = pdf_time("D:20261123010000Z").unwrap();
        let ist = pdf_time("D:20261123063000+05'30'").unwrap();
        assert_eq!(utc, ist);
        assert!(pdf_time("D:2026").is_none());
        assert!(pdf_time("D:20261399000000").is_none());
    }

    #[test]
    fn newest_first_then_undated() {
        let at = |d: &str| std::cmp::Reverse(pdf_time(d));
        assert!(at("D:20261123070000Z") < at("D:20261123064500+05'30'"));
        assert!(at("D:20250101000000Z") < at("not a date"));
    }

    #[test]
    fn colours_come_together_by_hue_greys_last() {
        let red = colour_key([0.9, 0.1, 0.1]);
        let yellow = colour_key([1.0, 0.85, 0.0]);
        let blue = colour_key([0.1, 0.3, 0.9]);
        let black = colour_key([0.0, 0.0, 0.0]);
        assert!(red < yellow && yellow < blue && blue < black);
    }
}
