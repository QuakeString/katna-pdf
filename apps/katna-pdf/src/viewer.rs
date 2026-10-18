// SPDX-License-Identifier: GPL-3.0-or-later

//! The reading window: one document's pages in a scrolling column, drawn
//! as they come into view (docs/PLAN.md §2.4).
//!
//! Pages are drawn on background threads, newest request first, at most
//! [`WORKERS`] at a time. A page whose zoom changed keeps showing its old
//! bitmap, stretched, until the sharp one arrives, so no page is ever
//! blank once drawn. Bitmaps far from the screen are freed.
//!
//! Each tab holds one viewer (`tabs.rs`); opening files is the tabs' job,
//! so the viewer only says when it is asked to ([`ViewerEvent`]).

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, FontWeight, ImageSource, KeyBinding, ListAlignment, ListOffset, ListState,
    RenderImage, Subscription, Window, actions, div, img, list, prelude::*, svg,
};
use image::{Frame, RgbaImage};
use katna_pdf_render::{Bitmap, Document, Engine, Hayro, OpenError};
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};
use katna_ui::{ScrollBar, px, unpx};

use crate::contents::{self, Contents};
use crate::history::History;
use crate::search::{self, Found, Hit, Texts};
use crate::select::{self, Selection, Spot};
use crate::text::t;
use crate::theme::Theme;
use crate::thumbnails::{self, Thumbs};

actions!(
    katna_pdf,
    [
        Open,
        ZoomIn,
        ZoomOut,
        FitWidth,
        ActualSize,
        NextPage,
        PreviousPage,
        ScrollDown,
        ToggleContents,
        ToggleNight,
        Find,
        FindNext,
        FindPrevious,
        CopyText,
        ScrollUp,
        CloseTab,
        NextTab,
        PreviousTab,
        Quit
    ]
);

/// Pages drawn at once.
const WORKERS: usize = 3;
/// Requests waiting beyond this many are dropped, oldest first.
const QUEUE: usize = 8;
/// Pages kept drawn on each side of the ones on screen.
const KEEP: usize = 3;
/// Zoom steps, as on Okular's and Acrobat's zoom menus.
const STEPS: &[f32] = &[
    0.12, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0,
];
/// Room beside the widest page at fit width, each side.
const MARGIN: f32 = 24.0;
/// Room between pages.
const GAP: f32 = 12.0;

/// The viewer, when no text field in it has the keys.
const READING: &str = "Viewer && !TextInput";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-o", Open, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-1", FitWidth, None),
        KeyBinding::new("ctrl-0", ActualSize, None),
        // Keys that mean something else while typing in a field.
        KeyBinding::new("pagedown", NextPage, Some(READING)),
        KeyBinding::new("pageup", PreviousPage, Some(READING)),
        KeyBinding::new("space", NextPage, Some(READING)),
        KeyBinding::new("shift-space", PreviousPage, Some(READING)),
        KeyBinding::new("down", ScrollDown, Some(READING)),
        KeyBinding::new("up", ScrollUp, Some(READING)),
        KeyBinding::new("ctrl-c", CopyText, Some(READING)),
        KeyBinding::new("ctrl-f", Find, None),
        KeyBinding::new("f3", FindNext, None),
        KeyBinding::new("shift-f3", FindPrevious, None),
        KeyBinding::new("f7", ToggleContents, None),
        KeyBinding::new("ctrl-shift-n", ToggleNight, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-f4", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-pagedown", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        KeyBinding::new("ctrl-pageup", PreviousTab, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// How big pages are shown.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    /// The widest page fills the card's width, following the window.
    FitWidth,
    /// Points to logical pixels (1.0 is the page's printed size on a
    /// 96 dpi screen, as other readers count 100%).
    Fixed(f32),
}

/// The find bar: its field and what it has found.
struct SearchBar {
    input: Entity<TextInput>,
    found: Found,
    /// Bumped for each new search, so the last one stops.
    generation: Arc<AtomicU64>,
    _input: Subscription,
}

/// The side pane's tabs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PaneTab {
    Pages,
    Contents,
}

/// What a viewer asks of the tabs around it.
pub enum ViewerEvent {
    /// The open button was pressed: choose files to open.
    ChooseFiles,
    /// Files were dropped on the viewer.
    Dropped(Vec<PathBuf>),
}

/// Recent files on the start screen.
const RECENT: usize = 8;

/// Points to logical pixels at 100%: 96 pixels to the inch, 72 points.
const POINT: f32 = 96.0 / 72.0;

/// What the card shows.
enum Content {
    /// Nothing opened yet.
    Empty,
    Opening(String),
    Failed(String),
    Ready(Opened),
}

struct Opened {
    name: String,
    doc: Arc<dyn Document>,
    /// The widest page, in points, for fit width.
    widest: f32,
}

/// A drawn page: the scale (device pixels per point) it was drawn at,
/// and whether as a night page.
struct Drawn {
    scale: f32,
    night: bool,
    image: Arc<RenderImage>,
}

pub struct Viewer {
    focus: FocusHandle,
    /// The file shown or being opened.
    path: Option<PathBuf>,
    content: Content,
    zoom: Zoom,
    /// Points to logical pixels, as last laid out.
    shown: f32,
    list: ListState,
    /// The list's overlay scrollbar, as Katna Mail's lists have.
    bar: ScrollBar,
    pages: HashMap<usize, Drawn>,
    /// Pages being drawn, with the scale asked for.
    drawing: HashMap<usize, f32>,
    /// Pages waiting to be drawn, newest last.
    queue: Vec<usize>,
    /// The pages on screen, as the list last reported.
    visible: Range<usize>,
    /// Bitmaps to free on the next frame (they need the window).
    released: Vec<Arc<RenderImage>>,
    /// Bumped on each open, so a late page of the last file is dropped.
    generation: u64,
    /// The document's outline, once read, if it has one.
    contents: Option<Contents>,
    /// Each page's text, read on first need, for search.
    texts: Option<Texts>,
    /// The find bar, while open.
    search: Option<SearchBar>,
    /// The text selected with the pointer, and whether it is being
    /// dragged out now.
    selection: Option<Selection>,
    selecting: bool,
    /// Pages whose text is being read for the pointer.
    reading_text: std::collections::HashSet<usize>,
    /// Where each page shown was laid out last frame, in the window.
    page_bounds: Rc<RefCell<HashMap<usize, gpui::Bounds<gpui::Pixels>>>>,
    /// Whether the side pane is shown, and which of its tabs.
    pane: bool,
    tab: PaneTab,
    thumbs: Thumbs,
    theme: Theme,
    /// Whether the window is dark: pages are then shown as night pages
    /// unless [`Viewer::night`] says otherwise.
    dark: bool,
    /// Night pages turned on or off by hand, overriding the theme.
    night: Option<bool>,
    _appearance: Subscription,
}

impl Viewer {
    pub fn new(file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance());
            let was = this.night_pages();
            this.dark = is_dark(window.appearance());
            if this.night_pages() != was {
                this.redraw_thumbs();
            }
            cx.notify();
        });
        let list = ListState::new(0, ListAlignment::Top, desktop_px(600.0));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            let range = event.visible_range.clone();
            weak.update(cx, |this, cx| this.scrolled(range, cx)).ok();
        });
        let mut this = Self {
            focus,
            path: None,
            content: Content::Empty,
            zoom: Zoom::FitWidth,
            shown: 1.0,
            list,
            bar: ScrollBar::default(),
            pages: HashMap::new(),
            drawing: HashMap::new(),
            queue: Vec::new(),
            visible: 0..0,
            released: Vec::new(),
            generation: 0,
            contents: None,
            texts: None,
            search: None,
            selection: None,
            selecting: false,
            reading_text: Default::default(),
            page_bounds: Rc::default(),
            pane: false,
            tab: PaneTab::Pages,
            thumbs: Thumbs::new(),
            theme: Theme::for_appearance(window.appearance()),
            dark: is_dark(window.appearance()),
            night: None,
            _appearance: appearance,
        };
        if let Some(file) = file {
            this.open(file, cx);
        }
        this
    }

    /// The file shown or being opened.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The tab's and the window's title: the file's name.
    pub fn title(&self) -> Option<String> {
        self.path.as_deref().map(file_name)
    }

    /// Reads and opens `path` off the UI thread; the card says "Opening…"
    /// meanwhile.
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = file_name(&path);
        self.path = Some(path.clone());
        self.generation += 1;
        let generation = self.generation;
        self.content = Content::Opening(name.clone());
        self.contents = None;
        self.texts = None;
        self.selection = None;
        self.reading_text.clear();
        if let Some(bar) = self.search.as_mut() {
            bar.generation.fetch_add(1, Ordering::Relaxed);
            bar.found = Found::default();
        }
        self.forget_pages();
        cx.notify();
        self.read_outline(path.clone(), generation, cx);
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path).map_err(|err| {
                        t(
                            "error-read",
                            &[("name", &name), ("error", &err.to_string())],
                        )
                    })?;
                    let doc = Hayro.open(Arc::new(bytes), "").map_err(|err| match err {
                        OpenError::Locked => t("error-locked", &[("name", &name)]),
                        OpenError::Invalid => t("error-invalid", &[("name", &name)]),
                    })?;
                    let widest = (0..doc.page_count())
                        .filter_map(|p| doc.page_size(p))
                        .fold(1.0_f32, |w, (pw, _)| w.max(pw));
                    Ok::<_, String>(Opened { name, doc, widest })
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match opened {
                    Ok(opened) => {
                        this.list.reset(opened.doc.page_count());
                        // Where it was left last time.
                        let page = this
                            .path
                            .as_deref()
                            .and_then(|p| cx.global::<History>().page(p));
                        if let Some(page) = page.filter(|p| *p < opened.doc.page_count()) {
                            this.list.scroll_to(ListOffset {
                                item_ix: page,
                                offset_in_item: px(0.0),
                            });
                        }
                        this.texts = Some(search::texts(opened.doc.page_count()));
                        let old = this.thumbs.reset(opened.doc.page_count());
                        this.released.extend(old);
                        this.visible = 0..1;
                        this.content = Content::Ready(opened);
                    }
                    Err(message) => this.content = Content::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reads the outline of `path` off the UI thread, after the pages
    /// have started, with our own object layer (hayro does not read
    /// outlines). The file is read again so that two copies are not
    /// held while the first page is drawn. The pane opens by itself when
    /// the file asks for it (`/PageMode /UseOutlines`).
    fn read_outline(&mut self, path: PathBuf, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let (outline, show) = cx
                .background_executor()
                .spawn(async move {
                    let Some(doc) = std::fs::read(&path)
                        .ok()
                        .and_then(|bytes| katna_pdf_cos::Document::load(bytes).ok())
                    else {
                        return (Vec::new(), false);
                    };
                    let show = doc
                        .catalog()
                        .is_some_and(|c| c.name("PageMode") == Some(b"UseOutlines"));
                    (doc.outline(), show)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation && !outline.is_empty() {
                    this.contents = Some(Contents::new(outline));
                    if show {
                        this.pane = true;
                        this.tab = PaneTab::Contents;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows or hides the side pane, once a document is open.
    fn toggle_pane(&mut self, cx: &mut Context<Self>) {
        if matches!(self.content, Content::Ready(_)) {
            self.pane = !self.pane;
            cx.notify();
        }
    }

    /// Opens the find bar, or selects its text if it is open, and puts
    /// the keys in it.
    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.content, Content::Ready(_)) {
            return;
        }
        let input = match &self.search {
            Some(bar) => bar.input.clone(),
            None => {
                let input = cx.new(|cx| TextInput::new(t("search-placeholder", &[]), cx));
                let sub =
                    cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                        InputEvent::Changed => this.start_search(cx),
                        InputEvent::Submit => this.step_search(true, cx),
                        InputEvent::Cancel => this.close_search(window, cx),
                    });
                self.search = Some(SearchBar {
                    input: input.clone(),
                    found: Found::default(),
                    generation: Arc::new(AtomicU64::new(0)),
                    _input: sub,
                });
                input
            }
        };
        input.update(cx, |input, cx| {
            input.set_accent(gpui::rgba(self.theme.katna.accent).into());
            input.select_all_text(cx);
        });
        window.focus(&input.focus_handle(cx), cx);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bar) = self.search.take() {
            bar.generation.fetch_add(1, Ordering::Relaxed);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Searches for what the find bar holds, from the page being read.
    fn start_search(&mut self, cx: &mut Context<Self>) {
        let from = self.current_page();
        let (Some(bar), Some(texts), Content::Ready(opened)) =
            (self.search.as_mut(), self.texts.clone(), &self.content)
        else {
            return;
        };
        let mine = bar.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let query = bar.input.read(cx).text().trim().to_owned();
        bar.found = Found {
            pages: if query.is_empty() { 0 } else { texts.len() },
            ..Found::default()
        };
        cx.notify();
        if query.is_empty() {
            return;
        }
        let (doc, generation) = (opened.doc.clone(), bar.generation.clone());
        let (sender, receiver) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                search::run(
                    doc.as_ref(),
                    &texts,
                    &query,
                    from,
                    &generation,
                    mine,
                    |page, hits| sender.send_blocking((page, hits)).is_ok(),
                );
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Ok((page, hits)) = receiver.recv().await {
                let going = this.update(cx, |this, cx| {
                    let Some(bar) = this.search.as_mut() else {
                        return false;
                    };
                    if bar.generation.load(Ordering::Relaxed) != mine {
                        return false;
                    }
                    let first = bar.found.current.is_none();
                    bar.found.add(page, hits);
                    // The first match found is shown at once.
                    if first && let Some(hit) = bar.found.current().cloned() {
                        this.go_to_hit(&hit, cx);
                    }
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Goes to the next match, or the one before.
    fn step_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(hit) = self
            .search
            .as_mut()
            .and_then(|bar| bar.found.step(forward).cloned())
        else {
            return;
        };
        self.go_to_hit(&hit, cx);
        cx.notify();
    }

    /// Scrolls so `hit` shows a little below the top of the view.
    fn go_to_hit(&mut self, hit: &Hit, cx: &mut Context<Self>) {
        let Some(top) = hit.boxes.iter().map(|b| b[1]).reduce(f32::min) else {
            return;
        };
        let gap = px(if hit.page == 0 { space::S5 } else { GAP / 2.0 });
        let below = desktop_px((top * self.shown - 96.0).max(0.0));
        self.list.scroll_to(ListOffset {
            item_ix: hit.page,
            offset_in_item: gap + below,
        });
        cx.notify();
    }

    /// The find bar under the toolbar.
    fn find_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = self.search.as_ref()?;
        let th = self.theme;
        let found = &bar.found;
        let status = if found.hits.is_empty() {
            if bar.input.read(cx).text().trim().is_empty() {
                String::new()
            } else if found.done() {
                t("search-none", &[])
            } else {
                t("search-searching", &[])
            }
        } else {
            let current = found.current.map_or(0, |c| c + 1).to_string();
            let total = found.hits.len().to_string();
            let count = t("search-count", &[("current", &current), ("total", &total)]);
            if found.done() {
                count
            } else {
                format!("{count}…")
            }
        };
        Some(
            div()
                .h(px(48.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .border_b_1()
                .border_color(th.line)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .max_w(px(420.0))
                        .h(px(32.0))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .px(px(space::S3))
                        .rounded(px(radius::FULL))
                        .bg(th.field)
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(
                            svg()
                                .path("icons/search.svg")
                                .size(px(16.0))
                                .flex_shrink_0()
                                .text_color(th.dim),
                        )
                        .child(bar.input.clone()),
                )
                .child(
                    div()
                        .min_w(px(72.0))
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .whitespace_nowrap()
                        .child(status),
                )
                .child(icon_button(
                    "find-previous",
                    "chevron-up",
                    t("search-previous", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.step_search(false, cx)),
                ))
                .child(icon_button(
                    "find-next",
                    "chevron-down",
                    t("search-next", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.step_search(true, cx)),
                ))
                .child(div().flex_1())
                .child(icon_button(
                    "find-close",
                    "close",
                    t("search-close", &[]),
                    th,
                    cx.listener(|this, _, window, cx| this.close_search(window, cx)),
                ))
                .into_any_element(),
        )
    }

    /// The page and character under `position` in the window, if a page
    /// shown is there and its text has been read. Reading starts here if
    /// it has not, so it is ready by the time the pointer presses.
    fn spot_at(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<Spot> {
        let shown = desktop_px(self.shown);
        let (page, bounds) = {
            let bounds = self.page_bounds.borrow();
            // The page under the pointer, or the nearest one above or
            // below it.
            bounds
                .iter()
                .min_by(|(_, a), (_, b)| {
                    let away = |r: &gpui::Bounds<gpui::Pixels>| {
                        let above = r.top() - position.y;
                        let below = position.y - r.bottom();
                        above.max(below).max(px(0.0))
                    };
                    away(a)
                        .partial_cmp(&away(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(page, bounds)| (*page, *bounds))?
        };
        let texts = self.texts.clone()?;
        let Some(text) = texts.get(page)?.get() else {
            self.read_text(page, cx);
            return None;
        };
        let point = (
            (position.x - bounds.left()) / shown,
            (position.y - bounds.top()) / shown,
        );
        let index = select::char_at(text, point)?;
        Some(Spot { page, index })
    }

    /// Reads page `page`'s text on a background thread, if not read yet.
    fn read_text(&mut self, page: usize, cx: &mut Context<Self>) {
        let (Some(texts), Content::Ready(opened)) = (self.texts.clone(), &self.content) else {
            return;
        };
        if texts.get(page).is_none_or(|t| t.get().is_some()) || !self.reading_text.insert(page) {
            return;
        }
        let doc = opened.doc.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    texts[page].get_or_init(|| doc.text(page));
                })
                .await;
            this.update(cx, |this, cx| {
                this.reading_text.remove(&page);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn press(&mut self, event: &gpui::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.selection = self.spot_at(event.position, cx).map(|spot| Selection {
            anchor: spot,
            head: spot,
        });
        self.selecting = self.selection.is_some();
        cx.notify();
    }

    fn drag(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        let spot = self.spot_at(event.position, cx);
        if !self.selecting || event.pressed_button != Some(gpui::MouseButton::Left) {
            self.selecting = false;
            return;
        }
        if let (Some(selection), Some(spot)) = (self.selection.as_mut(), spot)
            && selection.head != spot
        {
            selection.head = spot;
            cx.notify();
        }
    }

    fn release_pointer(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        // A click without a drag selects nothing.
        if self.selection.is_some_and(|s| s.anchor == s.head) {
            self.selection = None;
            cx.notify();
        }
    }

    /// The selected text, pages joined by line breaks.
    fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let texts = self.texts.as_ref()?;
        let parts: Vec<String> = selection
            .pages()
            .filter_map(|page| {
                let text = texts.get(page)?.get()?;
                let range = selection.on_page(page, text.boxes.len())?;
                Some(select::text_of(text, range))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    fn copy_text(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// Whether pages are shown as night pages: as set by hand, otherwise
    /// when the window is dark (docs/PLAN.md §4.5, auto mode).
    fn night_pages(&self) -> bool {
        self.night.unwrap_or(self.dark)
    }

    fn toggle_night(&mut self, cx: &mut Context<Self>) {
        self.night = Some(!self.night_pages());
        self.redraw_thumbs();
        cx.notify();
    }

    /// Thumbnails are drawn again in the new colours. Pages redraw by
    /// themselves, keeping the old picture until the new one comes.
    fn redraw_thumbs(&mut self) {
        let old = self.thumbs.reset(self.page_count());
        self.released.extend(old);
    }

    /// The colour of a page not drawn yet: its paper, as it will be drawn.
    fn paper(&self) -> gpui::Hsla {
        let [r, g, b] = if self.night_pages() {
            katna_pdf_render::recolor::flip([255, 255, 255])
        } else {
            [255, 255, 255]
        };
        gpui::rgb(u32::from_be_bytes([0, r, g, b])).into()
    }

    /// Whether the side pane is shown now.
    fn pane_shown(&self) -> bool {
        self.pane && matches!(self.content, Content::Ready(_))
    }

    /// Page `page`'s thumbnail, asked for if it is not drawn yet.
    fn render_thumb(
        &mut self,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let th = self.theme;
        let Content::Ready(opened) = &self.content else {
            return div().into_any_element();
        };
        let (w, h) = opened.doc.page_size(page).unwrap_or((612.0, 792.0));
        let image = self.thumbs.image(page);
        if image.is_none() {
            self.thumbs.want(page);
            self.pump_thumbs(window.scale_factor(), cx);
        }
        let current = page == self.current_page();
        let label = (page + 1).to_string();
        div()
            .id(("thumb", page))
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(space::S2))
            .py(px(space::S3))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.go_to_page(page, cx)))
            .child(
                div()
                    .w(px(thumbnails::WIDTH))
                    .h(px(thumbnails::WIDTH * h / w.max(1.0)))
                    .bg(self.paper())
                    .shadow_sm()
                    .border_2()
                    .border_color(if current {
                        gpui::rgba(th.katna.accent).into()
                    } else {
                        gpui::transparent_black()
                    })
                    .children(image.map(|image| img(ImageSource::Render(image)).size_full())),
            )
            .child(
                div()
                    .text_size(px(text::CAPTION))
                    .text_color(if current { th.text } else { th.dim })
                    .child(label),
            )
            .into_any_element()
    }

    /// Draws the next waiting thumbnail, one at a time.
    fn pump_thumbs(&mut self, scale_factor: f32, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let Some(page) = self.thumbs.next() else {
            return;
        };
        let width = opened.doc.page_size(page).map_or(612.0, |(w, _)| w);
        let scale = thumbnails::WIDTH * katna_ui::scale::scale() * scale_factor / width.max(1.0);
        let doc = opened.doc.clone();
        let generation = self.generation;
        let night = self.night_pages();
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move { draw(doc.as_ref(), page, scale, night) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    this.released.extend(image);
                    return;
                }
                match image {
                    Some(image) => {
                        let freed = this.thumbs.drawn(page, image);
                        this.released.extend(freed);
                    }
                    None => this.thumbs.failed(page),
                }
                this.pump_thumbs(scale_factor, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Scrolls so `dest` is at the top of the view: its height on the
    /// page when it gives one, otherwise the page's top.
    fn go_to(&mut self, dest: katna_pdf_cos::Destination, cx: &mut Context<Self>) {
        let count = self.page_count();
        let Content::Ready(opened) = &self.content else {
            return;
        };
        if count == 0 {
            return;
        }
        let page = dest.page.min(count - 1);
        let height = opened.doc.page_size(page).map_or(792.0, |(_, h)| h);
        // Each page sits below a gap (`render_page`); the height is
        // counted from the page's bottom, as PDF counts it.
        let gap = px(if page == 0 { space::S5 } else { GAP / 2.0 });
        let below_top = dest
            .top
            .map_or(0.0, |top| (height - top).clamp(0.0, height) * self.shown);
        self.list.scroll_to(ListOffset {
            item_ix: page,
            offset_in_item: if dest.top.is_some() {
                gap + desktop_px(below_top)
            } else {
                px(0.0)
            },
        });
        cx.notify();
    }

    /// The side pane: its tabs in a header lined up with the toolbar,
    /// then the page thumbnails or the outline.
    fn pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.pane_shown() {
            return None;
        }
        let th = self.theme;
        let thumb = th.katna.text_dim & 0xffff_ff00 | 0x99;
        // Contents only when the file has an outline.
        let tab = if self.contents.is_some() {
            self.tab
        } else {
            PaneTab::Pages
        };
        let body = match (tab, self.contents.as_ref()) {
            (PaneTab::Contents, Some(contents)) => {
                let rows = contents.rows().into_iter().map(|row| {
                    let (path, dest) = (row.path.clone(), row.dest);
                    let fold = cx.listener(move |this, _: &(), _, cx| {
                        if let Some(contents) = this.contents.as_mut() {
                            contents.toggle(&path);
                            cx.notify();
                        }
                    });
                    let go = cx.listener(move |this, _: &(), _, cx| {
                        if let Some(dest) = dest {
                            this.go_to(dest, cx);
                        }
                    });
                    contents::row(
                        row,
                        th,
                        move |window, cx| fold(&(), window, cx),
                        move |window, cx| go(&(), window, cx),
                    )
                });
                let list = div()
                    .id("contents-list")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&contents.scroll)
                    .py(px(space::S2))
                    .flex()
                    .flex_col()
                    .children(rows);
                contents.bar.clone().wrap(
                    "contents-scroll",
                    &contents.scroll,
                    list,
                    thumb,
                    window,
                    cx,
                )
            }
            _ => {
                let list = list(
                    self.thumbs.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_thumb(ix, window, cx)),
                )
                .size_full();
                self.thumbs.bar.clone().wrap(
                    "thumbs-scroll",
                    &self.thumbs.list,
                    list,
                    thumb,
                    window,
                    cx,
                )
            }
        };
        let tab_button = |id: &'static str, which: PaneTab, label: String| {
            let selected = tab == which;
            div()
                .id(id)
                .h(px(32.0))
                .px(px(space::S4))
                .flex()
                .items_center()
                .rounded(px(radius::FULL))
                .text_size(px(text::SMALL))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if selected { th.text } else { th.dim })
                .when(selected, |d| d.bg(th.field))
                .hover(move |s| s.bg(th.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = which;
                    cx.notify();
                }))
                .child(label)
        };
        let mut header = div()
            .h(px(48.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S3))
            .border_b_1()
            .border_color(th.line)
            .child(tab_button(
                "tab-pages",
                PaneTab::Pages,
                t("pane-pages", &[]),
            ));
        if self.contents.is_some() {
            header = header.child(tab_button(
                "tab-contents",
                PaneTab::Contents,
                t("pane-contents", &[]),
            ));
        }
        Some(
            div()
                .w(px(contents::WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(th.line)
                .child(header)
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element(),
        )
    }

    /// Frees every drawn page now: the tab is closing.
    pub fn release(&mut self, window: &mut Window) {
        self.generation += 1;
        self.forget_pages();
        for image in self.released.drain(..) {
            window.drop_image(image).ok();
        }
    }

    fn forget_pages(&mut self) {
        self.released
            .extend(self.pages.drain().map(|(_, drawn)| drawn.image));
        self.drawing.clear();
        self.queue.clear();
        self.list.reset(0);
    }

    /// Points to logical pixels for the current zoom, in a card
    /// `width` logical pixels wide.
    fn points_to_pixels(&self, width: f32) -> f32 {
        match (&self.zoom, &self.content) {
            (Zoom::Fixed(zoom), _) => zoom * POINT,
            (Zoom::FitWidth, Content::Ready(opened)) => {
                ((width - 2.0 * MARGIN) / opened.widest).clamp(0.05, 16.0 * POINT)
            }
            (Zoom::FitWidth, _) => POINT,
        }
    }

    fn set_zoom(&mut self, zoom: Zoom, cx: &mut Context<Self>) {
        if self.zoom != zoom {
            self.zoom = zoom;
            // Heights change: measure again, keeping the same place.
            self.list.remeasure();
            cx.notify();
        }
    }

    fn step_zoom(&mut self, up: bool, cx: &mut Context<Self>) {
        let now = self.shown / POINT;
        let next = if up {
            STEPS.iter().copied().find(|s| *s > now * 1.01)
        } else {
            STEPS.iter().rev().copied().find(|s| *s < now * 0.99)
        };
        if let Some(next) = next {
            self.set_zoom(Zoom::Fixed(next), cx);
        }
    }

    fn page_count(&self) -> usize {
        match &self.content {
            Content::Ready(opened) => opened.doc.page_count(),
            _ => 0,
        }
    }

    fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        let count = self.page_count();
        if count == 0 {
            return;
        }
        self.list.scroll_to(ListOffset {
            item_ix: page.min(count - 1),
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    fn current_page(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }

    /// The list scrolled: forget pages far from the new view.
    fn scrolled(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if range == self.visible {
            return;
        }
        self.forget_far(range);
        cx.notify();
    }

    /// Page `page` is being laid out. The list tells [`Viewer::scrolled`]
    /// only of wheel and touch scrolling, not of keys, jumps or the
    /// scrollbar, so a page shown after those is noted here; otherwise
    /// its bitmap would be thrown away as off screen when it arrived.
    fn laid_out(&mut self, page: usize) {
        if !self.visible.contains(&page) {
            self.forget_far(shown_with(&self.visible, self.current_page(), page));
        }
    }

    /// `range` is on screen: frees the bitmaps of pages far from it.
    fn forget_far(&mut self, range: Range<usize>) {
        self.visible = range;
        let keep = self.wanted();
        let far: Vec<usize> = self
            .pages
            .keys()
            .copied()
            .filter(|p| !keep.contains(p))
            .collect();
        for page in far {
            if let Some(drawn) = self.pages.remove(&page) {
                self.released.push(drawn.image);
            }
        }
        self.queue.retain(|p| keep.contains(p));
    }

    /// The pages whose bitmaps are kept: those on screen and a few
    /// either side.
    fn wanted(&self) -> Range<usize> {
        self.visible.start.saturating_sub(KEEP)..self.visible.end + KEEP
    }

    /// Asks for page `page` at `scale` device pixels per point, unless it
    /// is drawn or being drawn at about that scale already.
    fn want(&mut self, page: usize, scale: f32, cx: &mut Context<Self>) {
        let close = |s: f32| (s / scale - 1.0).abs() < 0.01;
        let night = self.night_pages();
        if self
            .pages
            .get(&page)
            .is_some_and(|d| close(d.scale) && d.night == night)
            || self.drawing.get(&page).is_some_and(|s| close(*s))
        {
            return;
        }
        self.queue.retain(|p| *p != page);
        self.queue.push(page);
        if self.queue.len() > QUEUE {
            self.queue.remove(0);
        }
        self.pump(scale, cx);
    }

    /// Starts drawing the newest requests while workers are free.
    fn pump(&mut self, scale: f32, cx: &mut Context<Self>) {
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let night = self.night_pages();
        while self.drawing.len() < WORKERS {
            let Some(page) = self.queue.pop() else {
                break;
            };
            if self.drawing.contains_key(&page) {
                continue;
            }
            self.drawing.insert(page, scale);
            let doc = opened.doc.clone();
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { draw(doc.as_ref(), page, scale, night) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        if let Some(image) = image {
                            this.released.push(image);
                        }
                        return;
                    }
                    this.drawing.remove(&page);
                    if let Some(image) = image {
                        if this.wanted().contains(&page) {
                            let drawn = Drawn {
                                scale,
                                night,
                                image,
                            };
                            if let Some(old) = this.pages.insert(page, drawn) {
                                this.released.push(old.image);
                            }
                        } else {
                            this.released.push(image);
                        }
                    }
                    this.pump(scale, cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn render_page(
        &mut self,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Content::Ready(opened) = &self.content else {
            return div().into_any_element();
        };
        let (w, h) = opened.doc.page_size(page).unwrap_or((612.0, 792.0));
        let shown = self.shown;
        let device = shown * window.scale_factor();
        let image = self.pages.get(&page).map(|d| d.image.clone());
        self.laid_out(page);
        self.want(page, device, cx);
        // Matches on this page, the current one stronger, and the
        // selected text.
        let accent = self.theme.katna.accent & 0xffff_ff00;
        let mut marks = Vec::new();
        let mark = |b: &[f32; 4], alpha: u32| {
            div()
                .absolute()
                .left(desktop_px(b[0] * shown))
                .top(desktop_px(b[1] * shown))
                .w(desktop_px((b[2] - b[0]) * shown))
                .h(desktop_px((b[3] - b[1]) * shown))
                .rounded(px(radius::XS / 2.0))
                .bg(gpui::rgba(accent | alpha))
        };
        if let (Some(selection), Some(text)) = (
            self.selection,
            self.texts.as_ref().and_then(|t| t.get(page)?.get()),
        ) && let Some(range) = selection.on_page(page, text.boxes.len())
        {
            marks.extend(text.line_boxes(range).iter().map(|b| mark(b, 0x55)));
        }
        if let Some(bar) = &self.search {
            for (i, hit) in bar.found.hits.iter().enumerate() {
                if hit.page != page {
                    continue;
                }
                let alpha = if Some(i) == bar.found.current {
                    0x77
                } else {
                    0x33
                };
                marks.extend(hit.boxes.iter().map(|b| mark(b, alpha)));
            }
        }
        div()
            .w_full()
            .flex()
            .justify_center()
            .pt(px(if page == 0 { space::S5 } else { GAP / 2.0 }))
            .pb(px(if page + 1 == self.page_count() {
                space::S5
            } else {
                GAP / 2.0
            }))
            .child(
                div()
                    .w(desktop_px(w * shown))
                    .h(desktop_px(h * shown))
                    .flex_shrink_0()
                    .bg(self.paper())
                    .shadow_sm()
                    .relative()
                    .cursor(gpui::CursorStyle::IBeam)
                    .children(image.map(|image| img(ImageSource::Render(image)).size_full()))
                    .children(marks)
                    .child({
                        // Notes where the page is, for the pointer.
                        let record = self.page_bounds.clone();
                        gpui::canvas(
                            move |bounds, _, _| {
                                record.borrow_mut().insert(page, bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full()
                    }),
            )
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let mut bar = div()
            .h(px(48.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S3))
            .border_b_1()
            .border_color(th.line);
        let title = match &self.content {
            Content::Ready(opened) => opened.name.clone(),
            Content::Opening(name) => name.clone(),
            _ => t("app-name", &[]),
        };
        if matches!(self.content, Content::Ready(_)) {
            bar = bar.child(icon_button(
                "side-pane",
                "folders-pane",
                t("side-pane-toggle", &[]),
                th,
                cx.listener(|this, _, _, cx| this.toggle_pane(cx)),
            ));
            bar = bar.child(icon_button(
                "search",
                "search",
                t("search-open", &[]),
                th,
                cx.listener(|this, _, window, cx| this.open_search(window, cx)),
            ));
        }
        bar = bar
            .child(icon_button(
                "open",
                "folder",
                t("open-file", &[]),
                th,
                cx.listener(|_, _, _, cx| cx.emit(ViewerEvent::ChooseFiles)),
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.text)
                    .child(title),
            );
        if self.page_count() > 0 {
            let page = (self.current_page() + 1).to_string();
            let count = self.page_count().to_string();
            bar = bar
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .px(px(space::S3))
                        .py(px(space::S2))
                        .rounded(px(radius::XS))
                        .bg(th.field)
                        .child(t("page-of", &[("page", &page), ("count", &count)])),
                )
                .child(separator(th))
                .child(icon_button(
                    "zoom-out",
                    "zoom-out",
                    t("zoom-out", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.step_zoom(false, cx)),
                ))
                .child(
                    div()
                        .id("zoom-level")
                        .min_w(px(52.0))
                        .h(px(32.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(radius::FULL))
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .hover(move |s| s.bg(th.hover))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.set_zoom(Zoom::Fixed(1.0), cx)))
                        .child(format!("{}%", (self.shown / POINT * 100.0).round())),
                )
                .child(icon_button(
                    "zoom-in",
                    "zoom-in",
                    t("zoom-in", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.step_zoom(true, cx)),
                ))
                .child(icon_button(
                    "fit-width",
                    "fit-width",
                    t("zoom-fit-width", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitWidth, cx)),
                ))
                .child(separator(th))
                .child(icon_button(
                    "night",
                    "contrast",
                    t("night-pages", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.toggle_night(cx)),
                ));
        }
        bar
    }

    fn message(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let body = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(space::S4))
            .max_w(px(420.0));
        let body = match &self.content {
            Content::Opening(name) => body.child(
                div()
                    .text_size(px(text::BODY))
                    .text_color(th.dim)
                    .child(t("opening", &[("name", name)])),
            ),
            Content::Failed(message) => body.child(
                div()
                    .text_size(px(text::BODY))
                    .text_color(th.error)
                    .text_center()
                    .child(message.clone()),
            ),
            _ => body
                .child(
                    svg()
                        .path("icons/document.svg")
                        .size(px(48.0))
                        .text_color(th.dim),
                )
                .child(
                    div()
                        .text_size(px(text::TITLE))
                        .text_color(th.text)
                        .child(t("empty-title", &[])),
                )
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(t("empty-detail", &[])),
                )
                .child(
                    widgets::filled_button("empty-open", t("empty-open", &[]), &th.katna)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ViewerEvent::ChooseFiles))),
                )
                .children(self.recent(cx)),
        };
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(th.backdrop)
            .child(body)
    }
}

impl Viewer {
    /// The files opened lately, for the start screen.
    fn recent(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let th = self.theme;
        let entries: Vec<_> = cx
            .global::<History>()
            .recent()
            .iter()
            .take(RECENT)
            .cloned()
            .collect();
        if entries.is_empty() {
            return None;
        }
        let rows = entries.into_iter().enumerate().map(|(i, entry)| {
            let folder = entry
                .path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let name = file_name(&entry.path);
            let path = entry.path;
            div()
                .id(("recent", i))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S3))
                .py(px(space::S2))
                .rounded(px(radius::SM))
                .cursor_pointer()
                .hover(|s| s.bg(th.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.open(path.clone(), cx)))
                .child(
                    svg()
                        .path("icons/document.svg")
                        .size(px(16.0))
                        .flex_none()
                        .text_color(th.dim),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(text::BODY))
                                .text_color(th.text)
                                .truncate()
                                .child(name),
                        )
                        .child(
                            div()
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .truncate()
                                .child(folder),
                        ),
                )
        });
        Some(
            div()
                .w(px(420.0))
                .mt(px(space::S4))
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(space::S3))
                        .pb(px(space::S2))
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(t("recent-title", &[])),
                )
                .children(rows),
        )
    }
}

impl EventEmitter<ViewerEvent> for Viewer {}

impl Focusable for Viewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.released.drain(..) {
            window.drop_image(image).ok();
        }
        let th = self.theme;
        // The pages' width: the window less its padding and the pane.
        let pane_width = if self.pane_shown() {
            contents::WIDTH
        } else {
            0.0
        };
        let card_width = unpx(window.viewport_size().width) * katna_ui::scale::scale()
            - (2.0 * space::S3 + pane_width) * katna_ui::scale::scale();
        let shown = self.points_to_pixels(card_width);
        if (shown / self.shown - 1.0).abs() > 0.001 {
            self.shown = shown;
            self.list.remeasure();
        }
        let pane = self.pane(window, cx);
        let find_bar = self.find_bar(cx);
        let pages: AnyElement = match &self.content {
            Content::Ready(_) => {
                // Reading progress: the page at the top, noted as it
                // changes.
                if let Some(path) = self.path.clone() {
                    History::opened(&path, self.current_page(), cx);
                }
                let pages = list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_page(ix, window, cx)),
                )
                .size_full();
                let thumb = th.katna.text_dim & 0xffff_ff00 | 0x99;
                // Pages laid out last frame are noted again as they are
                // laid out now.
                self.page_bounds.borrow_mut().clear();
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(th.backdrop)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event, window, cx| this.press(event, window, cx)),
                    )
                    .on_mouse_move(cx.listener(|this, event, _, cx| this.drag(event, cx)))
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.release_pointer(cx)),
                    )
                    .child(self.bar.clone().wrap(
                        "pages-scroll",
                        &self.list,
                        pages,
                        thumb,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
            _ => self.message(cx).into_any_element(),
        };
        div()
            .id("katna-pdf")
            .track_focus(&self.focus)
            .key_context("Viewer")
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_zoom(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_zoom(false, cx)))
            .on_action(cx.listener(|this, _: &FitWidth, _, cx| this.set_zoom(Zoom::FitWidth, cx)))
            .on_action(
                cx.listener(|this, _: &ActualSize, _, cx| this.set_zoom(Zoom::Fixed(1.0), cx)),
            )
            .on_action(cx.listener(|this, _: &NextPage, _, cx| {
                let page = this.current_page() + 1;
                this.go_to_page(page, cx);
            }))
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| {
                let page = this.current_page().saturating_sub(1);
                this.go_to_page(page, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleContents, _, cx| this.toggle_pane(cx)))
            .on_action(cx.listener(|this, _: &ToggleNight, _, cx| this.toggle_night(cx)))
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_search(window, cx)))
            .on_action(cx.listener(|this, _: &CopyText, _, cx| this.copy_text(cx)))
            .on_action(cx.listener(|this, _: &FindNext, _, cx| this.step_search(true, cx)))
            .on_action(cx.listener(|this, _: &FindPrevious, _, cx| this.step_search(false, cx)))
            .on_action(cx.listener(|this, _: &ScrollDown, _, cx| {
                this.list.scroll_by(px(48.0));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ScrollUp, _, cx| {
                this.list.scroll_by(px(-48.0));
                cx.notify();
            }))
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                cx.emit(ViewerEvent::Dropped(paths.paths().to_vec()));
            }))
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .overflow_hidden()
            .rounded(px(radius::SM))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .children(pane)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.toolbar(cx))
                    .children(find_bar)
                    .child(pages),
            )
    }
}

/// Katna's round icon button (`katna_ui::widgets`), with `tip` shown
/// under the pointer and read out by screen readers.
fn icon_button(
    id: &'static str,
    icon: &'static str,
    tip: String,
    th: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    widgets::icon_button(id, icon, 18.0, &th.katna)
        .tip(tip, &th.katna)
        .on_click(on_click)
}

fn separator(th: Theme) -> impl IntoElement {
    div().w(px(1.0)).h(px(20.0)).mx(px(space::S2)).bg(th.line)
}

/// The pages on screen once page `page` is laid out, with the list's
/// top at `top`: `visible` if it holds `page`, otherwise the pages from
/// the top to it.
fn shown_with(visible: &Range<usize>, top: usize, page: usize) -> Range<usize> {
    if visible.contains(&page) {
        visible.clone()
    } else {
        top.min(page)..top.max(page) + 1
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Page `page` drawn at `scale`, as a night page when `night`, ready to
/// show.
fn draw(doc: &dyn Document, page: usize, scale: f32, night: bool) -> Option<Arc<RenderImage>> {
    let mut bitmap = doc.render(page, scale)?;
    if night {
        let pictures = doc.pictures(page, scale);
        katna_pdf_render::recolor::darken(&mut bitmap, &pictures);
    }
    render_image(bitmap)
}

/// Whether `appearance` is a dark one.
fn is_dark(appearance: gpui::WindowAppearance) -> bool {
    matches!(
        appearance,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

/// A drawn page as GPUI draws images: BGRA.
fn render_image(bitmap: Bitmap) -> Option<Arc<RenderImage>> {
    let Bitmap {
        width,
        height,
        mut rgba,
    } = bitmap;
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    let image = RgbaImage::from_raw(width, height, rgba)?;
    Some(Arc::new(RenderImage::new([Frame::new(image)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_shown_after_a_jump_count_as_on_screen() {
        // Laid out where the list already was: nothing changes.
        assert_eq!(shown_with(&(4..6), 4, 5), 4..6);
        // After Space from page 0 to page 9, page 9 is on screen.
        assert_eq!(shown_with(&(0..2), 9, 9), 9..10);
        // Pages below the top as the list lays them out.
        assert_eq!(shown_with(&(9..10), 9, 10), 9..11);
        // A page above the top, drawn ahead of scrolling back.
        assert_eq!(shown_with(&(9..11), 9, 8), 8..10);
    }
}
