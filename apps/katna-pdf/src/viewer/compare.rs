// SPDX-License-Identifier: GPL-3.0-or-later

//! Comparing this file with another version of it (docs/PLAN.md §4.1,
//! "compare two documents, side by side and as a diff overlay"): the
//! older file's pages beside these, or drawn over them, with the
//! changes `katna_pdf_render::compare` finds listed in the side pane
//! under Changes. Words found changed are marked on both; pages whose
//! words agree are then drawn small and looked at for changed pictures
//! and drawings.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyElement, Context, FontWeight, RenderImage, ScrollHandle, Window, div, prelude::*};
use katna_pdf_render::Document;
use katna_pdf_render::compare::{self, Comparison, Kind};
use katna_ui::px;
use katna_ui::scale::desktop_px;
use katna_ui::scrollbar::ScrollBar;
use katna_ui::tokens::{radius, space, text};

use super::{Content, PaneTab, Viewer, file_name};
use crate::float;
use crate::text::t;

/// Only in the older file.
const RED: u32 = 0xd9_30_25;
/// Only in this one.
const BLUE: u32 = 0x1a_73_e8;
/// Drawn differently.
const AMBER: u32 = 0xe3_74_00;
/// Older pages kept drawn at most.
const KEEP: usize = 8;
/// Pages are looked at for changed drawings at this scale.
const LOOK: f32 = 1.0;
/// A change's text in the list is cut after this many characters.
const SHOWN_CHARS: usize = 120;

/// Comparing with another version.
pub(super) struct Comparing {
    /// Tells this comparison's late results from an earlier one's.
    id: u64,
    name: String,
    /// The older version, once open.
    doc: Option<Arc<dyn Document>>,
    result: Option<Comparison>,
    /// Pages looked at for changed drawings, of how many.
    looked: (usize, usize),
    failed: Option<String>,
    /// The change picked in the list.
    current: Option<usize>,
    /// The older page drawn over this one, rather than beside it.
    pub(super) overlay: bool,
    /// The older version's pages drawn: the scale and the picture.
    pages: HashMap<usize, (f32, Arc<RenderImage>)>,
    drawing: HashSet<usize>,
    scroll: ScrollHandle,
    bar: ScrollBar,
}

impl Viewer {
    /// Asks for the file to compare this one with.
    pub(super) fn choose_compare(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
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
                this.update(cx, |this, cx| this.compare_with(file, cx)).ok();
            }
        })
        .detach();
    }

    /// Compares this file with `older`, now or once the file is open.
    pub fn compare_with(&mut self, older: PathBuf, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            self.compare_after = Some(older);
            return;
        };
        let newer = opened.doc.clone();
        self.stop_comparing(cx);
        let id = self.generation * 1_000_000 + self.compare_count;
        self.compare_count += 1;
        self.comparing = Some(Comparing {
            id,
            name: file_name(&older),
            doc: None,
            result: None,
            looked: (0, 0),
            failed: None,
            current: None,
            overlay: false,
            pages: HashMap::new(),
            drawing: HashSet::new(),
            scroll: ScrollHandle::new(),
            bar: ScrollBar::default(),
        });
        self.pane = true;
        self.tab = PaneTab::Changes;
        let key = self.key.clone();
        let name = file_name(&older);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&older).map_err(|e| e.to_string())?;
                    let bytes = Arc::new(bytes);
                    let engine = crate::engine::engine();
                    // The same password as this file's, as versions
                    // of one file mostly share it.
                    let doc = engine
                        .open(bytes.clone(), "")
                        .or_else(|_| engine.open(bytes, key.as_deref().unwrap_or("")))
                        .map_err(|_| t("compare-unreadable", &[("name", &name)]))?;
                    let result = compare::compare(doc.as_ref(), newer.as_ref());
                    Ok::<_, String>((doc, result))
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(comparing) = this.comparing.as_mut().filter(|c| c.id == id) else {
                    return;
                };
                match found {
                    Ok((doc, result)) => {
                        let pairs = result.pages_to_look_at();
                        comparing.looked = (0, pairs.len());
                        comparing.doc = Some(doc);
                        comparing.result = Some(result);
                        this.list.remeasure();
                        this.look_at_drawings(id, pairs, cx);
                    }
                    Err(error) => comparing.failed = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Draws the pages whose words agree, one pair at a time, and adds
    /// the areas that differ to the changes.
    fn look_at_drawings(&mut self, id: u64, pairs: Vec<(usize, usize)>, cx: &mut Context<Self>) {
        let (Content::Ready(opened), Some(comparing)) = (&self.content, &self.comparing) else {
            return;
        };
        let (Some(older), newer) = (comparing.doc.clone(), opened.doc.clone()) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            for (page, old_page) in pairs {
                let (older, newer) = (older.clone(), newer.clone());
                let areas = cx
                    .background_executor()
                    .spawn(async move {
                        match (older.render(old_page, LOOK), newer.render(page, LOOK)) {
                            (Some(a), Some(b)) => compare::differences(&a, &b, LOOK),
                            _ => Vec::new(),
                        }
                    })
                    .await;
                let going = this
                    .update(cx, |this, cx| {
                        let Some(comparing) = this.comparing.as_mut().filter(|c| c.id == id) else {
                            return false;
                        };
                        comparing.looked.0 += 1;
                        if !areas.is_empty()
                            && let Some(result) = comparing.result.as_mut()
                        {
                            // The change picked stays picked.
                            let picked = comparing.current.map(|i| result.changes[i].clone());
                            result.add_drawing(page, areas);
                            comparing.current =
                                picked.and_then(|p| result.changes.iter().position(|c| *c == p));
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !going {
                    return;
                }
            }
        })
        .detach();
    }

    /// Stops comparing.
    pub(super) fn stop_comparing(&mut self, cx: &mut Context<Self>) {
        let Some(comparing) = self.comparing.take() else {
            return;
        };
        self.released
            .extend(comparing.pages.into_values().map(|(_, image)| image));
        if self.tab == PaneTab::Changes {
            self.tab = PaneTab::Pages;
        }
        self.list.remeasure();
        cx.notify();
    }

    /// Whether the older pages are shown beside these.
    pub(super) fn side_by_side(&self) -> bool {
        self.comparing
            .as_ref()
            .is_some_and(|c| !c.overlay && c.doc.is_some())
    }

    /// The older page to draw page `page` over, when overlaid.
    pub(super) fn overlaid(&self, page: usize) -> Option<(Arc<dyn Document>, usize)> {
        let comparing = self.comparing.as_ref().filter(|c| c.overlay)?;
        let old = comparing.result.as_ref()?.pages.get(page).copied()??;
        Some((comparing.doc.clone()?, old))
    }

    fn set_overlay(&mut self, overlay: bool, cx: &mut Context<Self>) {
        if let Some(comparing) = self.comparing.as_mut() {
            comparing.overlay = overlay;
            self.list.remeasure();
            cx.notify();
        }
    }

    /// Goes to change `index` in the list.
    fn pick_change(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(comparing) = self.comparing.as_mut() else {
            return;
        };
        let Some(result) = comparing.result.as_ref() else {
            return;
        };
        let Some(change) = result.changes.get(index) else {
            return;
        };
        comparing.current = Some(index);
        comparing.scroll.scroll_to_item(index);
        if let Some((page, _)) = result.new_place(change) {
            self.go_to_page(page, cx);
        }
        cx.notify();
    }

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(count) = self
            .comparing
            .as_ref()
            .and_then(|c| Some(c.result.as_ref()?.changes.len()))
            .filter(|n| *n > 0)
        else {
            return;
        };
        let current = self.comparing.as_ref().and_then(|c| c.current);
        let next = match (current, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(i), true) => (i + 1) % count,
            (Some(i), false) => (i + count - 1) % count,
        };
        self.pick_change(next, cx);
    }

    /// The marks on page `page` of this file.
    pub(super) fn compare_marks(&self, page: usize) -> Vec<AnyElement> {
        let Some((comparing, result)) = self
            .comparing
            .as_ref()
            .and_then(|c| Some((c, c.result.as_ref()?)))
        else {
            return Vec::new();
        };
        let current = comparing
            .current
            .and_then(|i| result.new_place(result.changes.get(i)?));
        let mut marks: Vec<AnyElement> = Vec::new();
        // Drawn over, the colours already say what changed.
        if !comparing.overlay {
            marks.extend(
                result
                    .new_marks(page)
                    .into_iter()
                    .map(|(rect, kind)| mark(rect, kind, false, self.shown)),
            );
        }
        if let Some((p, rect)) = current
            && p == page
        {
            marks.push(outline(rect, self.shown));
        }
        marks
    }

    /// The older page set beside page `page` of this file.
    pub(super) fn render_older(
        &mut self,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let shown = self.shown;
        let paper = self.paper();
        let device = shown * window.scale_factor();
        let colors = self.page_colors();
        let Some(comparing) = self.comparing.as_mut() else {
            return div().into_any_element();
        };
        let (Some(doc), Some(result)) = (comparing.doc.clone(), comparing.result.as_ref()) else {
            return div().into_any_element();
        };
        let Some(old) = result.pages.get(page).copied().flatten() else {
            // Nothing in the older file lines up with this page.
            let (w, h) = match &self.content {
                Content::Ready(opened) => opened.doc.page_size(page).unwrap_or((612.0, 792.0)),
                _ => (612.0, 792.0),
            };
            return div()
                .w(desktop_px(w * shown))
                .h(desktop_px(h * shown))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::S5))
                .rounded(px(radius::XS))
                .border_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(t("compare-no-page", &[]))
                .into_any_element();
        };
        let (w, h) = doc.page_size(old).unwrap_or((612.0, 792.0));
        let mut marks: Vec<AnyElement> = result
            .old_marks(old)
            .into_iter()
            .map(|(rect, kind)| mark(rect, kind, true, shown))
            .collect();
        if let Some(rect) = comparing
            .current
            .and_then(|i| result.old_place(result.changes.get(i)?))
            .filter(|(p, _)| *p == old)
            .map(|(_, rect)| rect)
        {
            marks.push(outline(rect, shown));
        }
        let image = comparing.pages.get(&old).map(|(_, image)| image.clone());
        let fresh = comparing
            .pages
            .get(&old)
            .is_some_and(|(scale, _)| (scale / device - 1.0).abs() < 0.01);
        if !fresh && comparing.drawing.insert(old) {
            let id = comparing.id;
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { super::draw(doc.as_ref(), old, device, colors) })
                    .await;
                this.update(cx, |this, cx| {
                    let Some(comparing) = this.comparing.as_mut().filter(|c| c.id == id) else {
                        this.released.extend(image);
                        return;
                    };
                    comparing.drawing.remove(&old);
                    if let Some(image) = image {
                        if let Some((_, gone)) = comparing.pages.insert(old, (device, image)) {
                            this.released.push(gone);
                        }
                        // The pages farthest from this one go first.
                        while comparing.pages.len() > KEEP {
                            let far = comparing
                                .pages
                                .keys()
                                .copied()
                                .max_by_key(|p| p.abs_diff(old));
                            if let Some((_, gone)) = far.and_then(|p| comparing.pages.remove(&p)) {
                                this.released.push(gone);
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        div()
            .w(desktop_px(w * shown))
            .h(desktop_px(h * shown))
            .flex_shrink_0()
            .bg(paper)
            .shadow_sm()
            .relative()
            .children(image.map(|image| gpui::img(gpui::ImageSource::Render(image)).size_full()))
            .children(marks)
            .into_any_element()
    }

    /// The head of the list of changes (R20): how many, and buttons to
    /// step through them and to stop.
    pub(super) fn compare_actions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let comparing = self.comparing.as_ref()?;
        let th = self.theme;
        let count = comparing.result.as_ref().map(|r| r.changes.len());
        let head = match count {
            Some(n) => t("compare-head", &[("n", &n.to_string())]),
            None => t("compare-head-working", &[]),
        };
        let ib = |id: &'static str, glyph: &'static str, tip: &'static str| {
            float::sized_ib(id, glyph, 28.0, 16.0, t(tip, &[]), false, &th)
        };
        Some(
            div()
                .h(px(40.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(2.0))
                .pl(px(space::S3))
                .pr(px(space::S2))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(text::MICRO))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(th.dim)
                        .child(head.to_uppercase()),
                )
                .child(
                    ib("compare-previous", "chevu", "compare-previous")
                        .on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx))),
                )
                .child(
                    ib("compare-next", "chev", "compare-next")
                        .on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx))),
                )
                .child(
                    ib("compare-close", "x", "compare-close")
                        .on_click(cx.listener(|this, _, _, cx| this.stop_comparing(cx))),
                )
                .into_any_element(),
        )
    }

    /// The banner over the pages while comparing (R20): which file is
    /// on which side, how many changes, and Overlay or Side by side.
    pub(super) fn compare_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let comparing = self.comparing.as_ref()?;
        let th = self.theme;
        let newer = self.path.as_deref().map(file_name).unwrap_or_default();
        let words = match (&comparing.failed, comparing.result.as_ref()) {
            (Some(error), _) => error.clone(),
            (None, None) => t("compare-working", &[]),
            (None, Some(result)) => {
                let n = result.changes.len();
                let changes = match n {
                    0 => t("compare-none-short", &[]),
                    1 => t("compare-one", &[]),
                    n => t("compare-many", &[("n", &n.to_string())]),
                };
                let key = if comparing.overlay {
                    "compare-banner-overlay"
                } else {
                    "compare-banner"
                };
                format!(
                    "{} · {changes}",
                    t(key, &[("older", &comparing.name), ("newer", &newer)])
                )
            }
        };
        let overlay = comparing.overlay;
        Some(
            div()
                .id("compare-banner")
                .occlude()
                .max_w(px(720.0))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .pl(px(space::S3))
                .pr(px(space::S1))
                .py(px(space::S1))
                .rounded(px(radius::MD))
                .bg(th.accent_soft)
                .text_size(px(text::SMALL))
                .text_color(th.accent_text)
                .child(float::icon("info", 15.0, th.accent_text))
                .child(div().flex_1().min_w_0().truncate().child(words))
                .when(comparing.doc.is_some(), |d| {
                    d.child(
                        float::button(
                            "compare-switch",
                            t(
                                if overlay {
                                    "compare-side"
                                } else {
                                    "compare-overlay"
                                },
                                &[],
                            ),
                            false,
                            &th,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.set_overlay(!overlay, cx)),
                        ),
                    )
                })
                .into_any_element(),
        )
    }

    /// The list of changes, for the side pane.
    pub(super) fn changes_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Some(comparing) = self.comparing.as_ref() else {
            return div().into_any_element();
        };
        let line = |s: String| {
            div()
                .px(px(space::S4))
                .py(px(space::S1))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(s)
        };
        let mut head = Vec::new();
        let Some(result) = comparing.result.as_ref() else {
            head.push(line(match &comparing.failed {
                Some(error) => error.clone(),
                None => t("compare-working", &[]),
            }));
            return div()
                .py(px(space::S2))
                .flex()
                .flex_col()
                .children(head)
                .into_any_element();
        };
        let (done, total) = comparing.looked;
        if result.changes.is_empty() && done == total {
            head.push(line(t("compare-none", &[])));
        }
        if done < total {
            head.push(line(t(
                "compare-looking",
                &[("done", &done.to_string()), ("total", &total.to_string())],
            )));
        }
        if comparing.overlay {
            head.push(line(t("compare-legend", &[])));
        }
        let rows = result.changes.iter().enumerate().map(|(i, change)| {
            let (was, now) = result.texts(change);
            let (was, now) = (cut(&was), cut(&now));
            let what = match change.kind {
                Kind::Added => t("compare-added", &[("text", &now)]),
                Kind::Removed => t("compare-removed", &[("text", &was)]),
                Kind::Changed => t("compare-changed", &[("was", &was), ("now", &now)]),
                Kind::Drawing => t("compare-drawing", &[]),
            };
            let page = result
                .new_place(change)
                .map(|(p, _)| t("compare-page", &[("page", &(p + 1).to_string())]))
                .unwrap_or_default();
            let picked = comparing.current == Some(i);
            let hover = th.hover;
            div()
                .id(("change", i))
                .mx(px(space::S2))
                .px(px(space::S2))
                .py(px(space::S2))
                .flex()
                .gap(px(space::S2))
                .rounded(px(radius::SM))
                .when(picked, |d| d.bg(th.accent_soft))
                .when(!picked, |d| d.hover(move |s| s.bg(hover)))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.pick_change(i, cx)))
                .child(
                    div()
                        .mt(px(5.0))
                        .size(px(8.0))
                        .flex_none()
                        .rounded(px(radius::FULL))
                        .bg(gpui::rgb(color(change.kind))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(text::CAPTION))
                                .text_color(th.dim)
                                .child(page),
                        )
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .when(picked, |d| d.font_weight(FontWeight::MEDIUM))
                                .child(what),
                        ),
                )
        });
        let thumb = th.katna.text_dim & 0xffff_ff00 | 0x99;
        let list = div()
            .id("changes-list")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&comparing.scroll)
            .py(px(space::S2))
            .flex()
            .flex_col()
            .children(head)
            .children(rows);
        comparing
            .bar
            .clone()
            .wrap("changes-scroll", &comparing.scroll, list, thumb, window, cx)
            .into_any_element()
    }
}

fn color(kind: Kind) -> u32 {
    match kind {
        Kind::Added | Kind::Changed => BLUE,
        Kind::Removed => RED,
        Kind::Drawing => AMBER,
    }
}

/// A change's mark on a page: words tinted, a removal a thin bar, a
/// drawing outlined. On the older page what went is red and the thin
/// bar where words came in blue.
fn mark(rect: [f32; 4], kind: Kind, older: bool, shown: f32) -> AnyElement {
    let base = div()
        .absolute()
        .left(desktop_px(rect[0] * shown))
        .top(desktop_px(rect[1] * shown))
        .w(desktop_px(((rect[2] - rect[0]) * shown).max(2.0)))
        .h(desktop_px((rect[3] - rect[1]) * shown));
    match kind {
        Kind::Drawing => base
            .border_1()
            .border_color(gpui::rgb(AMBER))
            .bg(gpui::rgba(AMBER << 8 | 0x18)),
        _ if rect[2] - rect[0] <= 2.5 => base.bg(gpui::rgb(if older { BLUE } else { RED })),
        _ if older => base.bg(gpui::rgba(RED << 8 | 0x40)),
        kind => base.bg(gpui::rgba(color(kind) << 8 | 0x40)),
    }
    .into_any_element()
}

/// The change picked, outlined.
fn outline(rect: [f32; 4], shown: f32) -> AnyElement {
    let pad = 3.0;
    div()
        .absolute()
        .left(desktop_px(rect[0] * shown - pad))
        .top(desktop_px(rect[1] * shown - pad))
        .w(desktop_px((rect[2] - rect[0]) * shown + 2.0 * pad))
        .h(desktop_px((rect[3] - rect[1]) * shown + 2.0 * pad))
        .rounded(px(radius::XS))
        .border_2()
        .border_color(gpui::rgb(AMBER))
        .into_any_element()
}

/// `s` cut to a length a list row shows.
fn cut(s: &str) -> String {
    match s.char_indices().nth(SHOWN_CHARS) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_owned(),
    }
}
