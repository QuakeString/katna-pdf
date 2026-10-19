// SPDX-License-Identifier: GPL-3.0-or-later

//! Making and changing links (docs/PLAN.md §4.3, Phase 4): with the link
//! tool from the edit row, a box dragged over the page becomes a link to
//! a web or mail address or to a page, typed in the card; a click on a
//! link already there picks it, to send it elsewhere or delete it. The
//! links are annotations (`katna_pdf_cos::Document::change_links`).

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{Destination, Link, LinkTarget};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::annots::Draft;
use super::{Content, Viewer};
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// The link tool, while in hand.
pub(super) struct Linking {
    /// Where the link goes, typed.
    target: Entity<TextInput>,
    /// A box being dragged out on the page shown.
    drawing: Option<Draft>,
    /// A new link's page and box, waiting for where it goes.
    new: Option<(usize, [f32; 4])>,
    /// A link already there, picked: its page and number on it.
    picked: Option<(usize, usize)>,
    /// Put the keys in the field on the next frame.
    focus: bool,
    _target: Subscription,
}

/// What was typed as a place to go: a page number, or an address. A bare
/// domain is taken as a web address, and a bare email address as a mail
/// one.
pub(super) fn parse_target(typed: &str, pages: usize) -> Option<LinkTarget> {
    let typed = typed.trim();
    if typed.is_empty() || typed.contains(char::is_whitespace) {
        return None;
    }
    let page = typed
        .strip_prefix(['p', 'P'])
        .unwrap_or(typed)
        .parse::<usize>()
        .ok();
    if let Some(page) = page {
        return (1..=pages).contains(&page).then(|| {
            LinkTarget::Page(Destination {
                page: page - 1,
                top: None,
            })
        });
    }
    let lower = typed.to_ascii_lowercase();
    let uri = if lower.contains("://") || lower.starts_with("mailto:") {
        typed.to_owned()
    } else if typed.contains('@') {
        format!("mailto:{typed}")
    } else if typed.contains('.') {
        format!("https://{typed}")
    } else {
        return None;
    };
    Some(LinkTarget::Uri(uri))
}

/// A link's place to go, as typed into the card.
fn shown_target(target: &LinkTarget) -> String {
    match target {
        LinkTarget::Page(d) => (d.page + 1).to_string(),
        LinkTarget::Uri(u) => u.strip_prefix("mailto:").unwrap_or(u).to_owned(),
    }
}

impl Viewer {
    /// Takes the link tool in hand, or puts it down.
    pub(super) fn toggle_linking(&mut self, cx: &mut Context<Self>) {
        if self.linking.take().is_some() {
            cx.notify();
            return;
        }
        self.pick_adding(None, cx);
        self.marking = None;
        self.properties = None;
        self.redacting = None;
        self.designing = None;
        self.numbering = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let target = cx.new(|cx| {
            let mut input = TextInput::new(t("link-target", &[]), cx);
            input.set_accent(accent);
            input
        });
        let sub = cx.subscribe(&target, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.apply_link(cx);
            }
        });
        self.linking = Some(Linking {
            target,
            drawing: None,
            new: None,
            picked: None,
            focus: false,
            _target: sub,
        });
        cx.notify();
    }

    /// A press on the pages with the link tool in hand: a click on a link
    /// picks it; otherwise a box starts being dragged out. Returns
    /// whether the tool is in hand.
    pub(super) fn link_press(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.linking.is_none() {
            return false;
        }
        let Some((page, point)) = self.page_point(position) else {
            return true;
        };
        let hit = self.inverse(page).and_then(|inv| {
            let [x, y] = inv.map(point);
            let links = self.links.as_ref()?.get(page)?;
            links.iter().rposition(|l| {
                let r = l.rect;
                x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
            })
        });
        let shown = hit.and_then(|i| {
            let link = self.links.as_ref()?.get(page)?.get(i)?;
            Some(shown_target(&link.target))
        });
        let Some(linking) = self.linking.as_mut() else {
            return true;
        };
        match (hit, shown) {
            (Some(i), Some(shown)) => {
                linking.picked = Some((page, i));
                linking.new = None;
                linking.focus = true;
                linking.target.update(cx, |input, cx| {
                    input.set_text(shown, cx);
                    input.select_all_text(cx);
                });
            }
            _ => {
                linking.drawing = Some(Draft {
                    page,
                    path: vec![point],
                });
            }
        }
        cx.notify();
        true
    }

    pub(super) fn link_drag(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self
            .linking
            .as_ref()
            .and_then(|l| l.drawing.as_ref())
            .map(|d| d.page)
        else {
            return false;
        };
        let shown = desktop_px(self.shown);
        let Some(b) = self.page_bounds.borrow().get(&page).copied() else {
            return true;
        };
        let point = [
            ((position.x - b.left()) / shown).clamp(0.0, (b.size.width / shown).max(0.0)),
            ((position.y - b.top()) / shown).clamp(0.0, (b.size.height / shown).max(0.0)),
        ];
        if let Some(draft) = self.linking.as_mut().and_then(|l| l.drawing.as_mut()) {
            draft.path.truncate(1);
            draft.path.push(point);
        }
        cx.notify();
        true
    }

    /// The pointer let go: the box dragged out waits for where it goes.
    pub(super) fn link_release(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(draft) = self.linking.as_mut().and_then(|l| l.drawing.take()) else {
            return false;
        };
        cx.notify();
        let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last()) else {
            return true;
        };
        if (to[0] - from[0]).abs() < CLICK || (to[1] - from[1]).abs() < CLICK {
            // A click away from the links: nothing picked.
            if let Some(linking) = self.linking.as_mut() {
                linking.picked = None;
                linking.new = None;
            }
            return true;
        }
        let Some(inv) = self.inverse(draft.page) else {
            return true;
        };
        let rect = inv.rect([
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[0].max(to[0]),
            from[1].max(to[1]),
        ]);
        if let Some(linking) = self.linking.as_mut() {
            linking.new = Some((draft.page, rect));
            linking.picked = None;
            linking.focus = true;
            linking
                .target
                .update(cx, |input, cx| input.set_text("", cx));
        }
        true
    }

    /// The new link made, or the one picked sent where the card says.
    fn apply_link(&mut self, cx: &mut Context<Self>) {
        let count = self.page_count();
        let Some(linking) = self.linking.as_mut() else {
            return;
        };
        let typed = linking.target.read(cx).text().to_owned();
        let Some(target) = parse_target(&typed, count) else {
            self.notice = Some(t("link-bad", &[("pages", &count.to_string())]));
            cx.notify();
            return;
        };
        let (page, remove, rect) = match (linking.new.take(), linking.picked.take()) {
            (Some((page, rect)), _) => (page, Vec::new(), rect),
            (None, Some((page, i))) => {
                let Some(link) = self.links.as_ref().and_then(|l| l.get(page)?.get(i)) else {
                    return;
                };
                (page, vec![i], link.rect)
            }
            (None, None) => return,
        };
        let link = Link { rect, target };
        self.change_document(
            vec![page],
            "link-failed",
            move |doc| {
                doc.change_links(page, &remove, &[link])
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        cx.notify();
    }

    /// Takes the link picked out.
    fn delete_link(&mut self, cx: &mut Context<Self>) {
        let Some((page, i)) = self.linking.as_mut().and_then(|l| l.picked.take()) else {
            return;
        };
        self.change_document(
            vec![page],
            "link-failed",
            move |doc| doc.change_links(page, &[i], &[]).map_err(|e| e.to_string()),
            cx,
        );
        cx.notify();
    }

    /// The links on page `page` while the tool is in hand, the one picked
    /// marked, and the box being dragged out.
    pub(super) fn link_overlay(&self, page: usize) -> Vec<AnyElement> {
        let Some(linking) = self.linking.as_ref() else {
            return Vec::new();
        };
        let shown = self.shown;
        let accent = self.theme.katna.accent & 0xffff_ff00;
        let area = |b: [f32; 4], strong: bool| {
            div()
                .absolute()
                .left(desktop_px(b[0] * shown))
                .top(desktop_px(b[1] * shown))
                .w(desktop_px((b[2] - b[0]) * shown))
                .h(desktop_px((b[3] - b[1]) * shown))
                .border_2()
                .border_color(gpui::rgba(accent | if strong { 0xff } else { 0x99 }))
                .bg(gpui::rgba(accent | if strong { 0x33 } else { 0x14 }))
        };
        let mut out = Vec::new();
        let transform = match &self.content {
            Content::Ready(opened) => opened.doc.page_transform(page),
            _ => None,
        };
        if let Some(transform) = transform {
            if let Some(links) = self.links.as_ref().and_then(|l| l.get(page)) {
                for (i, link) in links.iter().enumerate() {
                    let b = katna_pdf_render::map_rect(transform, link.rect);
                    let picked = linking.picked == Some((page, i));
                    out.push(area(b, picked).into_any_element());
                }
            }
            if let Some((p, rect)) = linking.new
                && p == page
            {
                let b = katna_pdf_render::map_rect(transform, rect);
                out.push(area(b, true).border_dashed().into_any_element());
            }
        }
        if let Some(draft) = linking.drawing.as_ref().filter(|d| d.page == page)
            && let (Some(&from), Some(&to)) = (draft.path.first(), draft.path.last())
        {
            let b = [
                from[0].min(to[0]),
                from[1].min(to[1]),
                from[0].max(to[0]),
                from[1].max(to[1]),
            ];
            out.push(area(b, false).border_dashed().into_any_element());
        }
        out
    }

    /// The link card, while the tool is in hand.
    pub(super) fn link_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let linking = self.linking.as_mut()?;
        let handle = linking.target.focus_handle(cx);
        if std::mem::take(&mut linking.focus) {
            window.focus(&handle, cx);
        } else if linking.new.is_none() && linking.picked.is_none() && handle.is_focused(window) {
            // The field went with the link it was for: the keys go back
            // to the pages, for Ctrl+S and the rest.
            window.focus(&self.focus, cx);
        }
        let linking = self.linking.as_ref()?;
        let th = self.theme;
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t(id, &[]))
        };
        let ready = linking.new.is_some() || linking.picked.is_some();
        let picked = linking.picked.is_some();
        Some(
            div()
                .id("link-card")
                .absolute()
                .top(px(space::S3))
                .right(px(space::S5))
                .w(px(340.0))
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
                        .child(t("link-title", &[])),
                )
                .child(label(if ready { "link-to" } else { "link-hint" }))
                .when(ready, |card| {
                    card.child(
                        div()
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .px(px(space::S3))
                            .rounded(px(radius::SM))
                            .bg(th.field)
                            .text_size(px(text::BODY))
                            .text_color(th.text)
                            .child(linking.target.clone()),
                    )
                    .child(
                        div()
                            .pt(px(space::S2))
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(space::S2))
                            .when(picked, |row| {
                                row.child(
                                    widgets::outlined_button(
                                        "link-delete",
                                        t("link-delete", &[]),
                                        &th.katna,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| this.delete_link(cx))),
                                )
                            })
                            .child(
                                widgets::filled_button(
                                    "link-apply",
                                    t(if picked { "link-change" } else { "link-make" }, &[]),
                                    &th.katna,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.apply_link(cx))),
                            ),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_typed_says_where_to_go() {
        let page = |p| Some(LinkTarget::Page(Destination { page: p, top: None }));
        let uri = |u: &str| Some(LinkTarget::Uri(u.into()));
        assert_eq!(parse_target("3", 5), page(2));
        assert_eq!(parse_target(" p5 ", 5), page(4));
        assert_eq!(parse_target("6", 5), None);
        assert_eq!(parse_target("0", 5), None);
        assert_eq!(parse_target("example.com", 5), uri("https://example.com"));
        assert_eq!(parse_target("http://a.b/c?d", 5), uri("http://a.b/c?d"));
        assert_eq!(parse_target("me@a.in", 5), uri("mailto:me@a.in"));
        assert_eq!(parse_target("mailto:me@a.in", 5), uri("mailto:me@a.in"));
        assert_eq!(parse_target("hello", 5), None);
        assert_eq!(parse_target("two words.com", 5), None);
        assert_eq!(parse_target("", 5), None);
    }
}
