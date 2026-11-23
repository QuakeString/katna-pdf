// SPDX-License-Identifier: GPL-3.0-or-later

//! Making and changing links (docs/PLAN.md §4.3, Phase 4): with the link
//! tool from the edit row, a box dragged over the page becomes a link to
//! a web or mail address or to a page, typed in the card; a click on a
//! link already there picks it, to send it elsewhere or delete it. The
//! links are annotations (`katna_pdf_cos::Document::change_links`).

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{Destination, Link, LinkFit, LinkStyle, LinkTarget};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{space, text};

use super::annots::Draft;
use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// A drag shorter than this, in points, is a click.
const CLICK: f32 = 3.0;

/// The link tool, while in hand.
pub(super) struct Linking {
    /// The page a link goes to, typed.
    page_no: Entity<TextInput>,
    /// The web or email address, typed.
    address: Entity<TextInput>,
    /// The named place, typed or picked.
    name: Entity<TextInput>,
    /// Which of those it goes to.
    kind: LinkKind,
    /// How the page is shown.
    fit: LinkFit,
    /// Whether a box is drawn round it.
    boxed: bool,
    /// The file's named places, read when the tool is taken up.
    names: Vec<String>,
    /// A box being dragged out on the page shown.
    drawing: Option<Draft>,
    /// A new link's page and box, waiting for where it goes.
    new: Option<(usize, [f32; 4])>,
    /// A link already there, picked: its page and number on it.
    picked: Option<(usize, usize)>,
    /// Put the keys in the field on the next frame.
    focus: bool,
    _subs: Vec<Subscription>,
}

impl Linking {
    /// Whether its card is open, for a new link or one picked.
    pub(super) fn open(&self) -> bool {
        self.new.is_some() || self.picked.is_some()
    }
}

/// Where a link goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkKind {
    Page,
    Web,
    Named,
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

impl Viewer {
    /// Takes the link tool in hand, or puts it down.
    pub(super) fn toggle_linking(&mut self, cx: &mut Context<Self>) {
        if self.linking.take().is_some() {
            cx.notify();
            return;
        }
        if !self.allows(super::Allow::Change, cx) {
            return;
        }
        self.pick_adding(None, cx);
        self.marking = None;
        self.properties = None;
        self.redacting = None;
        self.designing = None;
        self.numbering = None;
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let field = |hint: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new(hint, cx);
                input.set_accent(accent);
                input
            })
        };
        let page_no = field(t("link-page-hint", &[]), cx);
        let address = field(t("link-address-hint", &[]), cx);
        let name = field(t("link-name-hint", &[]), cx);
        let subs = [&page_no, &address, &name]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event, cx| match event {
                    InputEvent::Submit => this.apply_link(cx),
                    InputEvent::Changed => cx.notify(),
                    _ => {}
                })
            })
            .collect();
        self.linking = Some(Linking {
            page_no,
            address,
            name,
            kind: LinkKind::Page,
            fit: LinkFit::Page,
            boxed: false,
            names: Vec::new(),
            drawing: None,
            new: None,
            picked: None,
            focus: false,
            _subs: subs,
        });
        // The file's named places, for links to one of them.
        self.read_cos(
            |doc| {
                doc.named_destinations()
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
            },
            |this, names, cx| {
                if let Some(linking) = this.linking.as_mut() {
                    linking.names = names;
                    cx.notify();
                }
            },
            cx,
        );
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
        let found = hit.and_then(|i| {
            let link = self.links.as_ref()?.get(page)?.get(i)?;
            Some(link.target.clone())
        });
        let Some(linking) = self.linking.as_mut() else {
            return true;
        };
        match (hit, found) {
            (Some(i), Some(target)) => {
                linking.picked = Some((page, i));
                linking.new = None;
                linking.focus = true;
                linking.boxed = false;
                linking.fit = LinkFit::Page;
                match &target {
                    LinkTarget::Uri(u) => {
                        linking.kind = LinkKind::Web;
                        let shown = u.strip_prefix("mailto:").unwrap_or(u).to_owned();
                        linking
                            .address
                            .update(cx, |input, cx| input.set_text(shown, cx));
                    }
                    LinkTarget::Page(d) => {
                        linking.kind = LinkKind::Page;
                        let shown = (d.page + 1).to_string();
                        linking
                            .page_no
                            .update(cx, |input, cx| input.set_text(shown, cx));
                    }
                }
                // How it shows the page and looks, read from the file.
                self.read_cos(
                    move |doc| doc.link_styles(page).into_iter().nth(i),
                    move |this, style, cx| {
                        let Some(style) = style else {
                            return;
                        };
                        let Some(linking) = this.linking.as_mut() else {
                            return;
                        };
                        if linking.picked != Some((page, i)) {
                            return;
                        }
                        linking.fit = style.fit;
                        linking.boxed = style.boxed;
                        if let Some(name) = style.named {
                            linking.kind = LinkKind::Named;
                            linking
                                .name
                                .update(cx, |input, cx| input.set_text(name, cx));
                        }
                        cx.notify();
                    },
                    cx,
                );
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
            linking.boxed = false;
            for input in [&linking.page_no, &linking.address, &linking.name] {
                input.update(cx, |input, cx| input.set_text("", cx));
            }
        }
        true
    }

    /// The new link made, or the one picked sent where the card says.
    fn apply_link(&mut self, cx: &mut Context<Self>) {
        let count = self.page_count();
        let Some(linking) = self.linking.as_ref() else {
            return;
        };
        let style = LinkStyle {
            fit: linking.fit,
            boxed: linking.boxed,
            named: None,
        };
        let made = match linking.kind {
            LinkKind::Page => {
                let typed = linking.page_no.read(cx).text().trim().to_owned();
                match typed.parse::<usize>() {
                    Ok(page) if (1..=count).contains(&page) => Ok((
                        LinkTarget::Page(Destination {
                            page: page - 1,
                            top: None,
                        }),
                        style,
                    )),
                    _ => Err(t("link-bad-page", &[("pages", &count.to_string())])),
                }
            }
            LinkKind::Web => {
                let typed = linking.address.read(cx).text().to_owned();
                match parse_target(&typed, 0) {
                    Some(target @ LinkTarget::Uri(_)) => Ok((target, style)),
                    _ => Err(t("link-bad-address", &[])),
                }
            }
            LinkKind::Named => {
                let typed = linking.name.read(cx).text().trim().to_owned();
                if linking.names.contains(&typed) {
                    // The page is looked up again by name when read; any
                    // page will do here.
                    let target = LinkTarget::Page(Destination { page: 0, top: None });
                    Ok((
                        target,
                        LinkStyle {
                            named: Some(typed),
                            ..style
                        },
                    ))
                } else {
                    Err(t("link-bad-name", &[]))
                }
            }
        };
        let (target, style) = match made {
            Ok(made) => made,
            Err(words) => {
                self.notice = Some(super::Notice::warning(words, None));
                cx.notify();
                return;
            }
        };
        let Some(linking) = self.linking.as_mut() else {
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
                doc.change_links_styled(page, &remove, &[(link, style)])
                    .map_err(|e| e.to_string())
            },
            cx,
        );
        cx.notify();
    }

    /// The link card closed with nothing changed.
    fn cancel_link(&mut self, cx: &mut Context<Self>) {
        if let Some(linking) = self.linking.as_mut() {
            linking.new = None;
            linking.picked = None;
        }
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
    /// marked, and the box being dragged out; under the new or picked
    /// one, its card (E09).
    pub(super) fn link_overlay(&self, page: usize, cx: &Context<Self>) -> Vec<AnyElement> {
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
                .border_1()
                .border_color(gpui::rgba(accent | if strong { 0xff } else { 0x99 }))
                .bg(gpui::rgba(accent | if strong { 0x1a } else { 0x0d }))
        };
        let mut out = Vec::new();
        let transform = match &self.content {
            Content::Ready(opened) => opened.doc.page_transform(page),
            _ => None,
        };
        let mut card_at = None;
        if let Some(transform) = transform {
            if let Some(links) = self.links.as_ref().and_then(|l| l.get(page)) {
                for (i, link) in links.iter().enumerate() {
                    let b = katna_pdf_render::map_rect(transform, link.rect);
                    let picked = linking.picked == Some((page, i));
                    if picked {
                        card_at = Some(b);
                        out.push(area(b, true).border_dashed().into_any_element());
                    } else {
                        out.push(area(b, false).into_any_element());
                    }
                }
            }
            if let Some((p, rect)) = linking.new
                && p == page
            {
                let b = katna_pdf_render::map_rect(transform, rect);
                card_at = Some(b);
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
        if let Some(b) = card_at {
            out.push(
                div()
                    .absolute()
                    .left(desktop_px(b[0] * shown))
                    .top(desktop_px(b[3] * shown + 8.0))
                    .child(self.link_box(linking, cx))
                    .into_any_element(),
            );
        }
        out
    }

    /// The card under a link: where it goes, how the page is shown, a
    /// box round it.
    fn link_box(&self, linking: &Linking, cx: &Context<Self>) -> AnyElement {
        let th = self.theme;
        let picked = linking.picked.is_some();
        let label = |id: &str| {
            div()
                .text_size(px(text::SMALL))
                .text_color(th.text2)
                .child(t(id, &[]))
        };
        let field = |input: &Entity<TextInput>| {
            div()
                .h(px(32.0))
                .px(px(space::S3))
                .flex()
                .items_center()
                .rounded(px(7.0))
                .border_1()
                .border_color(th.line)
                .bg(th.card)
                .text_size(px(text::BODY))
                .text_color(th.text)
                .child(input.clone())
        };
        let kinds = float::seg(&th).children(
            [
                (LinkKind::Page, "link-kind-page"),
                (LinkKind::Web, "link-kind-web"),
                (LinkKind::Named, "link-kind-named"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (kind, words))| {
                float::seg_item(
                    ("link-kind", i),
                    None,
                    Some(t(words, &[]).into()),
                    linking.kind == kind,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(linking) = this.linking.as_mut() {
                        linking.kind = kind;
                        linking.focus = true;
                    }
                    cx.notify();
                }))
            }),
        );
        let fits = float::seg(&th).children(
            [
                (LinkFit::Page, "link-fit-page"),
                (LinkFit::Width, "link-fit-width"),
                (LinkFit::AsNow, "link-fit-now"),
            ]
            .into_iter()
            .enumerate()
            .map(|(i, (fit, words))| {
                float::seg_item(
                    ("link-fit", i),
                    None,
                    Some(t(words, &[]).into()),
                    linking.fit == fit,
                    &th,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(linking) = this.linking.as_mut() {
                        linking.fit = fit;
                    }
                    cx.notify();
                }))
            }),
        );
        let place = match linking.kind {
            LinkKind::Page => div()
                .flex()
                .gap(px(space::S3))
                .child(
                    div()
                        .w(px(80.0))
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("link-page"))
                        .child(field(&linking.page_no)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .child(label("link-show"))
                        .child(fits),
                ),
            LinkKind::Web => div()
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(label("link-address"))
                .child(field(&linking.address)),
            LinkKind::Named => {
                let typed = linking.name.read(cx).text().to_lowercase();
                let matching: Vec<String> = linking
                    .names
                    .iter()
                    .filter(|n| n.to_lowercase().contains(typed.trim()))
                    .take(6)
                    .cloned()
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(label("link-name"))
                    .child(field(&linking.name))
                    .when(linking.names.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(th.dim)
                                .child(t("link-no-names", &[])),
                        )
                    })
                    .child(div().flex().flex_wrap().gap(px(space::S1)).children(
                        matching.into_iter().enumerate().map(|(i, name)| {
                            let chosen = name.clone();
                            float::chip(("link-name-pick", i), name, false, &th).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if let Some(linking) = this.linking.as_mut() {
                                        let chosen = chosen.clone();
                                        linking
                                            .name
                                            .update(cx, |input, cx| input.set_text(chosen, cx));
                                    }
                                    cx.notify();
                                }),
                            )
                        }),
                    ))
            }
        };
        div()
            .id("link-card")
            .occlude()
            .w(px(320.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(th.card)
            .shadow(th.lifted())
            .border_1()
            .border_color(th.line)
            .font_family(gpui::SharedString::from(".SystemUIFont"))
            .cursor(gpui::CursorStyle::Arrow)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(space::S4))
                    .flex()
                    .flex_col()
                    .gap(px(space::S3))
                    .child(
                        div()
                            .text_size(px(text::BODY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(th.text)
                            .child(t("link-title", &[])),
                    )
                    .child(kinds)
                    .child(place)
                    .child(
                        div()
                            .id("link-boxed")
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .cursor_pointer()
                            .text_size(px(text::SMALL))
                            .text_color(th.text)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(linking) = this.linking.as_mut() {
                                    linking.boxed = !linking.boxed;
                                }
                                cx.notify();
                            }))
                            .child(super::sign::check_box(linking.boxed, th))
                            .child(t("link-boxed", &[])),
                    ),
            )
            .child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .border_t_1()
                    .border_color(th.line)
                    .when(picked, |row| {
                        row.child(
                            div()
                                .id("link-delete")
                                .px(px(space::S2))
                                .py(px(space::S1))
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .text_size(px(text::SMALL))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(th.error)
                                .hover(|s| s.bg(th.error_soft))
                                .on_click(cx.listener(|this, _, _, cx| this.delete_link(cx)))
                                .child(t("link-delete", &[])),
                        )
                    })
                    .child(div().flex_1())
                    .when(!picked, |row| {
                        row.child(
                            float::button("link-cancel", t("link-cancel", &[]), false, &th)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_link(cx))),
                        )
                    })
                    .child(
                        float::button("link-apply", t("link-done", &[]), true, &th)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_link(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Puts the keys in the link card's field when it opens, and gives
    /// them back to the pages when it closes. It draws nothing here: the
    /// card is under its link on the page.
    pub(super) fn link_card(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let linking = self.linking.as_mut()?;
        let input = match linking.kind {
            LinkKind::Page => &linking.page_no,
            LinkKind::Web => &linking.address,
            LinkKind::Named => &linking.name,
        };
        let handle = input.focus_handle(cx);
        let open = linking.new.is_some() || linking.picked.is_some();
        if std::mem::take(&mut linking.focus) {
            window.focus(&handle, cx);
        } else if !open {
            let any = [&linking.page_no, &linking.address, &linking.name]
                .iter()
                .any(|i| i.focus_handle(cx).is_focused(window));
            if any {
                // The field went with the link it was for: the keys go
                // back to the pages, for Ctrl+S and the rest.
                window.focus(&self.focus, cx);
            }
        }
        None
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
