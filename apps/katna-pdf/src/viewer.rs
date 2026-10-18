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
use std::collections::{BTreeSet, HashMap};
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
use katna_pdf_render::recolor::PageColors;
use katna_pdf_render::{Bitmap, Document, Engine, Hayro, OpenError};
use katna_ui::scale::desktop_px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};
use katna_ui::{ScrollBar, px, unpx};

use crate::contents::{self, Contents};
use crate::form::{Editing, Form};
use crate::history::History;
use crate::organize::{self, PageOp};
use crate::search::{self, Found, Hit, Texts};
use crate::select::{self, Selection, Spot};
use crate::spread::Spread;
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
        FitPage,
        ActualSize,
        NextSpread,
        Print,
        Save,
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
        KeyBinding::new("ctrl-p", Print, None),
        KeyBinding::new("ctrl-s", Save, None),
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-1", FitWidth, None),
        KeyBinding::new("ctrl-2", FitPage, None),
        KeyBinding::new("ctrl-3", NextSpread, None),
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
    /// The tallest page fits in the view, and the widest across it.
    FitPage,
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

/// A change to the pages, as [`Viewer::change_pages`] writes it.
enum Pages {
    /// The new pages: (0 for this file, page, quarter turns added).
    Order(Vec<(usize, usize, i32)>),
    /// The pages of `file` inserted before page `at`.
    Insert { file: PathBuf, at: usize },
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
    /// It needs a password; `wrong` once one given did not open it.
    Locked {
        name: String,
        wrong: bool,
    },
    Ready(Opened),
}

struct Opened {
    name: String,
    doc: Arc<dyn Document>,
    /// The widest and the tallest page, in points, for fit width and fit
    /// page.
    widest: f32,
    tallest: f32,
}

/// A drawn page: the scale (device pixels per point) it was drawn at,
/// and in which colours.
struct Drawn {
    scale: f32,
    colors: PageColors,
    image: Arc<RenderImage>,
}

pub struct Viewer {
    focus: FocusHandle,
    /// The file shown or being opened.
    path: Option<PathBuf>,
    content: Content,
    zoom: Zoom,
    /// One page a row or two.
    spread: Spread,
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
    /// The first page of the row at the top, as of the last render: the
    /// list cannot be asked while it lays pages out.
    top: usize,
    /// Bitmaps to free on the next frame (they need the window).
    released: Vec<Arc<RenderImage>>,
    /// Bumped on each open, so a late page of the last file is dropped.
    generation: u64,
    /// Bumped when the file shown changes in place (a form filled in),
    /// so a page drawn from the old one is drawn again.
    revision: u64,
    /// The document's outline, once read, if it has one.
    contents: Option<Contents>,
    /// The form, once read, if the file has fields to fill in.
    form: Option<Form>,
    /// Each page's links, once read.
    links: Option<Arc<Vec<Vec<katna_pdf_cos::Link>>>>,
    /// Each page's text, read on first need, for search.
    texts: Option<Texts>,
    /// The find bar, while open.
    search: Option<SearchBar>,
    /// The password field, while a file asks for one, and whether to put
    /// the keys in it on the next frame.
    password: Option<(Entity<TextInput>, Subscription)>,
    focus_password: bool,
    /// A line shown under the toolbar until dismissed, such as why
    /// printing failed.
    notice: Option<String>,
    /// The form's changes the person was told closing would lose.
    close_warned: Option<u64>,
    /// The text selected with the pointer, and whether it is being
    /// dragged out now.
    selection: Option<Selection>,
    selecting: bool,
    /// Pages whose text is being read for the pointer.
    reading_text: std::collections::HashSet<usize>,
    /// Where each page shown was laid out last frame, in the window.
    page_bounds: Rc<RefCell<HashMap<usize, gpui::Bounds<gpui::Pixels>>>>,
    /// The size of the pages' view, as laid out last frame.
    view_size: Rc<std::cell::Cell<Option<gpui::Size<gpui::Pixels>>>>,
    /// The pages picked in the pages pane, for page changes, and the one
    /// a Shift+click runs from.
    picked: BTreeSet<usize>,
    anchor: Option<usize>,
    /// The file with its pages changed, until saved.
    edited: Option<Arc<Vec<u8>>>,
    /// Whether a page change is being written.
    changing_pages: bool,
    /// Whether the side pane is shown, and which of its tabs.
    pane: bool,
    tab: PaneTab,
    thumbs: Thumbs,
    theme: Theme,
    /// Whether the window is dark: pages are then shown as night pages
    /// unless [`Viewer::colors`] says otherwise.
    dark: bool,
    /// Night pages turned on or off by hand, overriding the theme.
    colors: Option<PageColors>,
    /// The colours picked last other than normal, for Ctrl+Shift+N.
    last_colors: PageColors,
    /// Whether the page colours menu is open, and when a press outside
    /// last closed it (a press on its button closes it, not again open).
    colors_menu: bool,
    colors_menu_closed: Option<std::time::Instant>,
    _appearance: Subscription,
}

impl Viewer {
    pub fn new(file: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            this.theme = Theme::for_appearance(window.appearance());
            let was = this.page_colors();
            this.dark = is_dark(window.appearance());
            if this.page_colors() != was {
                this.redraw_thumbs();
            }
            cx.notify();
        });
        let list = ListState::new(0, ListAlignment::Top, desktop_px(600.0));
        let weak = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            let rows = event.visible_range.clone();
            weak.update(cx, |this, cx| {
                let pages = this.spread.pages_on(rows, this.page_count());
                this.scrolled(pages, cx);
            })
            .ok();
        });
        let mut this = Self {
            focus,
            path: None,
            content: Content::Empty,
            zoom: Zoom::FitWidth,
            spread: Spread::default(),
            shown: 1.0,
            list,
            bar: ScrollBar::default(),
            pages: HashMap::new(),
            drawing: HashMap::new(),
            queue: Vec::new(),
            visible: 0..0,
            top: 0,
            released: Vec::new(),
            generation: 0,
            revision: 0,
            contents: None,
            form: None,
            links: None,
            texts: None,
            search: None,
            password: None,
            focus_password: false,
            notice: None,
            close_warned: None,
            selection: None,
            selecting: false,
            reading_text: Default::default(),
            page_bounds: Rc::default(),
            view_size: Rc::default(),
            picked: BTreeSet::new(),
            anchor: None,
            edited: None,
            changing_pages: false,
            pane: false,
            tab: PaneTab::Pages,
            thumbs: Thumbs::new(),
            theme: Theme::for_appearance(window.appearance()),
            dark: is_dark(window.appearance()),
            colors: None,
            last_colors: PageColors::Night,
            colors_menu: false,
            colors_menu_closed: None,
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
        self.open_with(path, String::new(), cx);
    }

    /// Opens the file asking for a password with the one typed.
    fn unlock(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some((input, _))) = (self.path.clone(), &self.password) else {
            return;
        };
        let password = input.read(cx).text().to_owned();
        if !password.is_empty() {
            self.open_with(path, password, cx);
        }
    }

    /// The password field, made on first need.
    fn password_field(&mut self, cx: &mut Context<Self>) -> Entity<TextInput> {
        if let Some((input, _)) = &self.password {
            return input.clone();
        }
        let input = cx.new(|cx| {
            let mut input = TextInput::new(t("password-placeholder", &[]), cx);
            input.set_masked(true, cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Submit) {
                this.unlock(cx);
            }
        });
        self.password = Some((input.clone(), sub));
        input
    }

    fn open_with(&mut self, path: PathBuf, password: String, cx: &mut Context<Self>) {
        let name = file_name(&path);
        let tried = !password.is_empty();
        self.path = Some(path.clone());
        self.generation += 1;
        let generation = self.generation;
        self.content = Content::Opening(name.clone());
        self.contents = None;
        self.form = None;
        self.links = None;
        self.texts = None;
        self.selection = None;
        self.reading_text.clear();
        self.picked.clear();
        self.anchor = None;
        self.edited = None;
        self.changing_pages = false;
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
                    });
                    // `None` when it asks for a password.
                    let bytes = bytes.map_err(Some)?;
                    let doc = Hayro
                        .open(Arc::new(bytes), &password)
                        .map_err(|err| match err {
                            OpenError::Locked => None,
                            OpenError::Invalid => Some(t("error-invalid", &[("name", &name)])),
                            OpenError::Unsupported => {
                                Some(t("error-encryption", &[("name", &name)]))
                            }
                        })?;
                    let (widest, tallest) = (0..doc.page_count())
                        .filter_map(|p| doc.page_size(p))
                        .fold((1.0_f32, 1.0_f32), |(w, h), (pw, ph)| {
                            (w.max(pw), h.max(ph))
                        });
                    Ok::<_, Option<String>>(Opened {
                        name,
                        doc,
                        widest,
                        tallest,
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match opened {
                    Ok(opened) => {
                        this.list.reset(this.spread.rows(opened.doc.page_count()));
                        // Where it was left last time.
                        let page = this
                            .path
                            .as_deref()
                            .and_then(|p| cx.global::<History>().page(p));
                        if let Some(page) = page.filter(|p| *p < opened.doc.page_count()) {
                            this.list.scroll_to(ListOffset {
                                item_ix: this.spread.row_of(page),
                                offset_in_item: px(0.0),
                            });
                        }
                        this.texts = Some(search::texts(opened.doc.page_count()));
                        let old = this.thumbs.reset(opened.doc.page_count());
                        this.released.extend(old);
                        this.visible = 0..1;
                        this.content = Content::Ready(opened);
                        this.password = None;
                    }
                    Err(Some(message)) => this.content = Content::Failed(message),
                    Err(None) => {
                        let input = this.password_field(cx);
                        let accent = gpui::rgba(this.theme.katna.accent).into();
                        input.update(cx, |input, cx| {
                            input.set_accent(accent);
                            input.select_all_text(cx);
                        });
                        this.focus_password = true;
                        this.content = Content::Locked {
                            name: file_name(this.path.as_deref().unwrap_or(Path::new(""))),
                            wrong: tried,
                        };
                    }
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
            let (outline, show, links, form) = cx
                .background_executor()
                .spawn(async move {
                    let Some(doc) = std::fs::read(&path)
                        .ok()
                        .and_then(|bytes| katna_pdf_cos::Document::load(bytes).ok())
                    else {
                        return (Vec::new(), false, Vec::new(), None);
                    };
                    let show = doc
                        .catalog()
                        .is_some_and(|c| c.name("PageMode") == Some(b"UseOutlines"));
                    let (outline, links, fields) = (doc.outline(), doc.links(), doc.form());
                    // The object layer's copy is kept only for a form.
                    let form = (!fields.is_empty()).then(|| (Arc::new(doc), fields));
                    (outline, show, links, form)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some((source, fields)) = form {
                    this.form = Form::new(source, fields);
                    cx.notify();
                }
                if links.iter().any(|l| !l.is_empty()) {
                    this.links = Some(Arc::new(links));
                    cx.notify();
                }
                if !outline.is_empty() {
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
            item_ix: self.spread.row_of(hit.page),
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
        // A press outside the field being typed in ends the typing.
        self.finish_typing(true, cx);
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

    /// The pages' colours: as picked, otherwise night pages when the
    /// window is dark (docs/PLAN.md §4.5, auto mode).
    fn page_colors(&self) -> PageColors {
        self.colors.unwrap_or(if self.dark {
            PageColors::Night
        } else {
            PageColors::Normal
        })
    }

    /// Shows the pages in `colors`.
    fn set_colors(&mut self, colors: PageColors, cx: &mut Context<Self>) {
        self.colors_menu = false;
        if colors != PageColors::Normal {
            self.last_colors = colors;
        }
        if colors != self.page_colors() {
            self.colors = Some(colors);
            self.redraw_thumbs();
        }
        cx.notify();
    }

    /// Normal colours, or back to the ones picked last.
    fn toggle_night(&mut self, cx: &mut Context<Self>) {
        let colors = if self.page_colors() == PageColors::Normal {
            self.last_colors
        } else {
            PageColors::Normal
        };
        self.set_colors(colors, cx);
    }

    /// The page colours menu, under its button.
    fn colors_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.colors_menu {
            return None;
        }
        let th = self.theme;
        let now = self.page_colors();
        let rows = PageColors::ALL.into_iter().map(|colors| {
            let [r, g, b] = colors.paper();
            let [ir, ig, ib] = colors.ink();
            div()
                .id(("colors", colors as usize))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S3))
                .py(px(space::S2))
                .rounded(px(radius::SM))
                .cursor_pointer()
                .hover(move |s| s.bg(th.hover))
                .on_click(cx.listener(move |this, _, _, cx| this.set_colors(colors, cx)))
                // A swatch: the paper with a line of ink.
                .child(
                    div()
                        .size(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(radius::XS))
                        .border_1()
                        .border_color(th.line)
                        .bg(gpui::rgb(u32::from_be_bytes([0, r, g, b])))
                        .child(
                            div()
                                .w(px(10.0))
                                .h(px(2.0))
                                .bg(gpui::rgb(u32::from_be_bytes([0, ir, ig, ib]))),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(t(colors_text(colors), &[])),
                )
                .when(colors == now, |row| {
                    row.child(
                        svg()
                            .path("icons/check.svg")
                            .size(px(16.0))
                            .flex_none()
                            .text_color(gpui::rgb(th.katna.accent >> 8)),
                    )
                })
        });
        let menu = div()
            .id("colors-menu")
            .occlude()
            .w(px(240.0))
            .mt(px(space::S1))
            .p(px(space::S1))
            .flex()
            .flex_col()
            .rounded(px(radius::MD))
            .bg(th.card)
            .border_1()
            .border_color(th.line)
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.colors_menu = false;
                this.colors_menu_closed = Some(std::time::Instant::now());
                cx.notify();
            }))
            .children(rows);
        Some(
            div()
                .absolute()
                .top_full()
                .right_0()
                .child(gpui::deferred(
                    katna_ui::anchored()
                        .anchor(gpui::Anchor::TopRight)
                        .child(menu),
                ))
                .into_any_element(),
        )
    }

    /// Thumbnails are drawn again in the new colours. Pages redraw by
    /// themselves, keeping the old picture until the new one comes.
    fn redraw_thumbs(&mut self) {
        let old = self.thumbs.reset(self.page_count());
        self.released.extend(old);
    }

    /// The colour of a page not drawn yet: its paper, as it will be drawn.
    fn paper(&self) -> gpui::Hsla {
        let [r, g, b] = self.page_colors().paper();
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
        let picked = self.picked.contains(&page);
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
            .when(picked, |d| d.bg(th.field))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                let keys = event.modifiers();
                this.thumb_click(page, keys.control || keys.platform, keys.shift, cx);
            }))
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
        let (generation, revision) = (self.generation, self.revision);
        let colors = self.page_colors();
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move { draw(doc.as_ref(), page, scale, colors) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    this.released.extend(image);
                    return;
                }
                if this.revision != revision {
                    // Drawn from the file before it changed.
                    this.released.extend(image);
                    this.thumbs.dropped();
                    this.pump_thumbs(scale_factor, cx);
                    cx.notify();
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
        // counted from the page's bottom, as PDF counts it, before the
        // page's crop and rotation.
        let gap = px(if page == 0 { space::S5 } else { GAP / 2.0 });
        let transform = opened.doc.page_transform(page);
        let below_top = dest.top.map_or(0.0, |top| {
            let y = transform.map_or(height - top, |t| {
                katna_pdf_render::map_point(t, (0.0, top)).1
            });
            y.clamp(0.0, height) * self.shown
        });
        self.list.scroll_to(ListOffset {
            item_ix: self.spread.row_of(page),
            offset_in_item: if dest.top.is_some() {
                gap + desktop_px(below_top)
            } else {
                px(0.0)
            },
        });
        cx.notify();
    }

    /// A link was clicked: go to its place, or open its address in the
    /// browser (web and mail addresses only).
    fn follow(&mut self, target: &katna_pdf_cos::LinkTarget, cx: &mut Context<Self>) {
        match target {
            katna_pdf_cos::LinkTarget::Page(dest) => self.go_to(*dest, cx),
            katna_pdf_cos::LinkTarget::Uri(uri) => {
                let lower = uri.trim().to_ascii_lowercase();
                if ["http://", "https://", "mailto:"]
                    .iter()
                    .any(|scheme| lower.starts_with(scheme))
                {
                    cx.open_url(uri.trim());
                }
            }
        }
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
        let actions = (tab == PaneTab::Pages).then(|| self.page_actions(cx));
        Some(
            div()
                .w(px(contents::WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_r_1()
                .border_color(th.line)
                .child(header)
                .children(actions)
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
    /// `width` logical pixels wide whose pages' view is `height` high.
    fn points_to_pixels(&self, width: f32, height: f32) -> f32 {
        let columns = self.spread.columns() as f32;
        let fit_width = |opened: &Opened| {
            (width - 2.0 * MARGIN - GAP * (columns - 1.0)) / (opened.widest * columns)
        };
        let shown = match (&self.zoom, &self.content) {
            (Zoom::Fixed(zoom), _) => zoom * POINT,
            (Zoom::FitWidth, Content::Ready(opened)) => fit_width(opened),
            (Zoom::FitPage, Content::Ready(opened)) => {
                fit_width(opened).min((height - 2.0 * GAP) / opened.tallest)
            }
            (_, _) => POINT,
        };
        shown.clamp(0.05, 16.0 * POINT)
    }

    /// Ctrl and the wheel (or a touchpad's two fingers) zoom smoothly.
    fn wheel_zoom(&mut self, delta: gpui::ScrollDelta, cx: &mut Context<Self>) {
        let lines = match delta {
            gpui::ScrollDelta::Lines(lines) => lines.y,
            gpui::ScrollDelta::Pixels(pixels) => unpx(pixels.y) / 50.0,
        };
        if lines == 0.0 {
            return;
        }
        let now = self.shown / POINT;
        let zoom = (now * 1.1_f32.powf(lines)).clamp(STEPS[0], STEPS[STEPS.len() - 1]);
        self.set_zoom(Zoom::Fixed(zoom), cx);
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
            item_ix: self.spread.row_of(page.min(count - 1)),
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    /// Goes to the next row of pages, or the one before.
    fn step_page(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.page_count();
        let row = self.list.logical_scroll_top().item_ix;
        let row = if forward {
            row + 1
        } else {
            row.saturating_sub(1)
        };
        if row < self.spread.rows(count) {
            let page = self.spread.pages_in(row, count).start;
            self.go_to_page(page, cx);
        }
    }

    /// A fillable widget was clicked: a text field is typed into, a check
    /// box or radio button changes at once.
    fn form_click(
        &mut self,
        field: usize,
        widget: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        if let Some(value) = form.clicked(field, widget) {
            if form.set(field, value) {
                self.refill(field, cx);
            }
            return;
        }
        if form
            .editing
            .as_ref()
            .is_some_and(|e| e.field == field && e.widget == widget)
        {
            return;
        }
        self.finish_typing(true, cx);
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let value = form.fields[field].value.clone().unwrap_or_default();
        let masked = matches!(
            form.fields[field].kind,
            katna_pdf_cos::FieldKind::Text { password: true, .. }
        );
        let accent = gpui::rgba(self.theme.katna.accent).into();
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", cx);
            input.set_masked(masked, cx);
            input.set_accent(accent);
            input.set_text(value, cx);
            input.select_all_text(cx);
            input
        });
        let sub = cx.subscribe(&input, |this, _, event, cx| match event {
            InputEvent::Submit => this.finish_typing(true, cx),
            InputEvent::Cancel => this.finish_typing(false, cx),
            InputEvent::Changed => {}
        });
        window.focus(&input.focus_handle(cx), cx);
        form.editing = Some(Editing {
            field,
            widget,
            input,
            _input: sub,
        });
        cx.notify();
    }

    /// Ends typing in a text field, keeping what was typed or not.
    fn finish_typing(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let Some(editing) = form.editing.take() else {
            return;
        };
        cx.notify();
        if !keep {
            return;
        }
        let mut text = editing.input.read(cx).text().to_owned();
        if let katna_pdf_cos::FieldKind::Text {
            max_len: Some(max), ..
        } = form.fields[editing.field].kind
        {
            text = text.chars().take(max).collect();
        }
        if form.set(editing.field, katna_pdf_cos::FieldValue::Text(text)) {
            self.refill(editing.field, cx);
        }
    }

    /// Writes the form's changes into the file and draws the pages of
    /// field `field` again from it.
    fn refill(&mut self, field: usize, cx: &mut Context<Self>) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let source = form.source.clone();
        let changes = form.changes.clone();
        let mine = form.generation;
        let pages = form.pages_of(field);
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let filled = cx
                .background_executor()
                .spawn(async move {
                    let bytes = Arc::new(source.fill(&changes).map_err(|e| e.to_string())?);
                    let doc = Hayro.open(bytes.clone(), "").map_err(|e| e.to_string())?;
                    Ok::<_, String>((bytes, doc))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation
                    || this.form.as_ref().is_none_or(|f| f.generation != mine)
                {
                    return;
                }
                match filled {
                    Ok((bytes, doc)) => {
                        if let Some(form) = this.form.as_mut() {
                            form.filled = Some(bytes);
                        }
                        this.swap_doc(doc, &pages);
                    }
                    Err(error) => {
                        this.notice = Some(t("form-failed", &[("error", &error)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows `doc`, the same file with changes, in place of the one shown;
    /// `pages` are the pages that changed and are drawn again.
    fn swap_doc(&mut self, doc: Arc<dyn Document>, pages: &[usize]) {
        let Content::Ready(opened) = &mut self.content else {
            return;
        };
        opened.doc = doc;
        self.revision += 1;
        for page in pages {
            // Kept on screen until drawn again.
            if let Some(drawn) = self.pages.get_mut(page) {
                drawn.scale = 0.0;
            }
            self.drawing.remove(page);
            self.released.extend(self.thumbs.forget(*page));
        }
        self.texts = Some(search::texts(self.page_count()));
    }

    /// A click on page `page`'s thumbnail: picks it (Ctrl adds or takes
    /// it away, Shift picks the run to it) and, alone, goes to it.
    fn thumb_click(&mut self, page: usize, ctrl: bool, shift: bool, cx: &mut Context<Self>) {
        self.picked = organize::click(&self.picked, self.anchor, page, ctrl, shift);
        if !shift {
            self.anchor = Some(page);
        }
        if !ctrl && !shift {
            self.go_to_page(page, cx);
        }
        cx.notify();
    }

    /// The buttons above the thumbnails that change the pages picked.
    fn page_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let some = !self.picked.is_empty() && !self.changing_pages;
        let button = |id: &'static str, icon: &'static str, tip: &str, enabled: bool| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .tip(t(tip, &[]), &th.katna)
                .when(!enabled, |b| b.opacity(0.4))
        };
        let op = |op: PageOp| cx.listener(move |this, _: &ClickEvent, _, cx| this.page_op(op, cx));
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
                button("pages-left", "rotate-ccw", "pages-turn-left", some)
                    .on_click(op(PageOp::Turn(-1))),
            )
            .child(
                button("pages-right", "rotate-cw", "pages-turn-right", some)
                    .on_click(op(PageOp::Turn(1))),
            )
            .child(
                button("pages-earlier", "arrow-up", "pages-earlier", some)
                    .on_click(op(PageOp::Earlier)),
            )
            .child(
                button("pages-later", "arrow-down", "pages-later", some)
                    .on_click(op(PageOp::Later)),
            )
            .child(
                button("pages-delete", "trash", "pages-delete", some).on_click(op(PageOp::Delete)),
            )
            .child(
                button(
                    "pages-insert",
                    "insert-below",
                    "pages-insert",
                    !self.changing_pages,
                )
                .on_click(cx.listener(|this, _, _, cx| this.choose_insert(cx))),
            )
            .child(
                button("pages-extract", "download", "pages-extract", some)
                    .on_click(cx.listener(|this, _, _, cx| this.choose_extract(cx))),
            )
            .into_any_element()
    }

    /// Turns, deletes or moves the pages picked.
    fn page_op(&mut self, op: PageOp, cx: &mut Context<Self>) {
        if self.changing_pages {
            return;
        }
        let Some((order, picked)) = organize::apply(op, self.page_count(), &self.picked) else {
            return;
        };
        let order = order.into_iter().map(|(p, turns)| (0, p, turns)).collect();
        self.change_pages(Pages::Order(order), picked, cx);
    }

    /// Asks for a PDF whose pages go after the pages picked.
    fn choose_insert(&mut self, cx: &mut Context<Self>) {
        if self.changing_pages || self.page_count() == 0 {
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
                this.update(cx, |this, cx| {
                    let at = organize::insert_at(this.page_count(), &this.picked);
                    this.change_pages(Pages::Insert { file, at }, BTreeSet::new(), cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// Asks where to save the pages picked as a file of their own.
    fn choose_extract(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.picked.is_empty() {
            return;
        }
        let pages: Vec<usize> = self.picked.iter().copied().collect();
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let suggested = t("pages-extract-name", &[("name", &stem)]);
        let chosen = cx.prompt_for_new_path(&folder, Some(&suggested));
        let current = self.unsaved();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let doc = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    let sources: Vec<_> = pages
                        .iter()
                        .map(|&page| katna_pdf_cos::PageSource {
                            doc: &doc,
                            page,
                            turns: 0,
                        })
                        .collect();
                    let out = katna_pdf_cos::compose(None, &sources).map_err(|e| e.to_string())?;
                    std::fs::write(&target, out).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => t("pages-extracted", &[("name", &file_name(&target))]),
                    Err(error) => t("pages-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Writes the file with its pages changed, off the UI thread, and
    /// shows it; `picked` are the pages picked afterwards.
    fn change_pages(&mut self, change: Pages, picked: BTreeSet<usize>, cx: &mut Context<Self>) {
        self.finish_typing(true, cx);
        let (Some(path), Content::Ready(opened)) = (self.path.clone(), &self.content) else {
            return;
        };
        if self
            .form
            .as_ref()
            .is_some_and(|f| f.dirty() && f.filled.is_none())
        {
            // A form change is still being written; try again then.
            return;
        }
        let name = opened.name.clone();
        let current = self.unsaved();
        let generation = self.generation;
        self.changing_pages = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let changed = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).map_err(|e| e.to_string())?,
                    };
                    let base = katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                    let (order, other) = match change {
                        Pages::Order(order) => (order, None),
                        Pages::Insert { file, at } => {
                            let bytes = std::fs::read(&file).map_err(|e| e.to_string())?;
                            let other =
                                katna_pdf_cos::Document::load(bytes).map_err(|e| e.to_string())?;
                            let mut order: Vec<_> = (0..at).map(|p| (0, p, 0)).collect();
                            order.extend((0..other.pages().len()).map(|p| (1, p, 0)));
                            order.extend((at..base.pages().len()).map(|p| (0, p, 0)));
                            (order, Some(other))
                        }
                    };
                    let sources = order
                        .iter()
                        .map(|&(from, page, turns)| {
                            let doc = match (from, other.as_ref()) {
                                (1, Some(other)) => other,
                                _ => &base,
                            };
                            katna_pdf_cos::PageSource { doc, page, turns }
                        })
                        .collect::<Vec<_>>();
                    let out =
                        katna_pdf_cos::compose(Some(&base), &sources).map_err(|e| e.to_string())?;
                    let bytes = Arc::new(out);
                    let doc = Hayro.open(bytes.clone(), "").map_err(|e| e.to_string())?;
                    let cos =
                        katna_pdf_cos::Document::load(bytes.to_vec()).map_err(|e| e.to_string())?;
                    Ok::<_, String>((bytes, doc, cos))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.changing_pages = false;
                match changed {
                    Ok((bytes, doc, cos)) => this.show_changed(name, bytes, doc, cos, picked),
                    Err(error) => this.notice = Some(t("pages-failed", &[("error", &error)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows the file with its pages changed in place of the one shown.
    fn show_changed(
        &mut self,
        name: String,
        bytes: Arc<Vec<u8>>,
        doc: Arc<dyn Document>,
        cos: katna_pdf_cos::Document,
        picked: BTreeSet<usize>,
    ) {
        let count = doc.page_count();
        let (widest, tallest) = (0..count)
            .filter_map(|p| doc.page_size(p))
            .fold((1.0_f32, 1.0_f32), |(w, h), (pw, ph)| {
                (w.max(pw), h.max(ph))
            });
        let page = picked
            .first()
            .copied()
            .unwrap_or_else(|| self.current_page())
            .min(count.saturating_sub(1));
        self.forget_pages();
        self.revision += 1;
        self.list.reset(self.spread.rows(count));
        self.list.scroll_to(ListOffset {
            item_ix: self.spread.row_of(page),
            offset_in_item: px(0.0),
        });
        self.visible = page..page + 1;
        let old = self.thumbs.reset(count);
        self.released.extend(old);
        self.texts = Some(search::texts(count));
        if let Some(bar) = self.search.as_mut() {
            bar.generation.fetch_add(1, Ordering::Relaxed);
            bar.found = Found::default();
        }
        self.selection = None;
        self.reading_text.clear();
        let (outline, links, fields) = (cos.outline(), cos.links(), cos.form());
        self.contents = (!outline.is_empty()).then(|| Contents::new(outline));
        self.links = links.iter().any(|l| !l.is_empty()).then(|| Arc::new(links));
        // A form's changes are in the file now.
        self.form = Form::new(Arc::new(cos), fields);
        self.content = Content::Ready(Opened {
            name,
            doc,
            widest,
            tallest,
        });
        self.edited = Some(bytes);
        self.anchor = picked.first().copied();
        self.picked = picked;
    }

    /// Whether the file shown has changes not saved: a form filled in or
    /// pages changed.
    fn dirty(&self) -> bool {
        self.edited.is_some() || self.form.as_ref().is_some_and(Form::dirty)
    }

    /// The file shown, as changed and not yet saved; `None` when it is
    /// the file on disk, or when a form change is still being written.
    fn unsaved(&self) -> Option<Arc<Vec<u8>>> {
        match self.form.as_ref().filter(|f| f.dirty()) {
            Some(form) => form.filled.clone(),
            None => self.edited.clone(),
        }
    }

    /// Whether the tab may close: not at first when there are changes
    /// not saved; the notice says so, and closing again closes.
    pub fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.dirty() {
            return true;
        }
        let mark = self.revision + self.form.as_ref().map_or(0, |f| f.generation << 32);
        if self.close_warned == Some(mark) {
            return true;
        }
        self.close_warned = Some(mark);
        self.notice = Some(t("unsaved-close", &[]));
        cx.notify();
        false
    }

    /// Writes the changed file (a form filled in, pages changed) over
    /// the file opened.
    fn save(&mut self, cx: &mut Context<Self>) {
        self.finish_typing(true, cx);
        let Some(path) = self.path.clone() else {
            return;
        };
        if !self.dirty() {
            return;
        }
        let Some(bytes) = self.unsaved() else {
            // Still being written; Save again once it is.
            return;
        };
        let mine = self.form.as_ref().map_or(0, |f| f.generation);
        let revision = self.revision;
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    save_over(&path, &bytes).map_err(|e| e.to_string())?;
                    katna_pdf_cos::Document::load(bytes.to_vec()).map_err(|e| e.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(source) => {
                        if let Some(form) = this.form.as_mut()
                            && form.generation == mine
                        {
                            form.saved(Arc::new(source));
                        }
                        if this.revision == revision {
                            this.edited = None;
                        }
                        this.notice = None;
                    }
                    Err(error) => this.notice = Some(t("save-failed", &[("error", &error)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Hands the open file to the desktop's print dialog.
    fn print(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Content::Ready(opened)) = (self.path.clone(), &self.content) else {
            return;
        };
        let title = opened.name.clone();
        cx.spawn(async move |this, cx| {
            let printed = crate::print::print(&title, &path).await;
            let notice = match printed {
                crate::print::Printed::Done => None,
                crate::print::Printed::NoDialog => Some(t("print-no-dialog", &[])),
                crate::print::Printed::Failed(error) => {
                    Some(t("print-failed", &[("error", &error)]))
                }
            };
            this.update(cx, |this, cx| {
                this.notice = notice;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Goes round the layouts: one page, two, two with a cover.
    fn next_spread(&mut self, cx: &mut Context<Self>) {
        let page = self.current_page();
        self.spread = self.spread.next();
        if let Content::Ready(opened) = &self.content {
            let count = opened.doc.page_count();
            self.list.reset(self.spread.rows(count));
            self.go_to_page(page, cx);
        }
        cx.notify();
    }

    /// The first page of the row at the top.
    fn current_page(&self) -> usize {
        let row = self.list.logical_scroll_top().item_ix;
        self.spread.pages_in(row, self.page_count()).start
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
            self.forget_far(shown_with(&self.visible, self.top, page));
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
        let colors = self.page_colors();
        if self
            .pages
            .get(&page)
            .is_some_and(|d| close(d.scale) && d.colors == colors)
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
        let colors = self.page_colors();
        while self.drawing.len() < WORKERS {
            let Some(page) = self.queue.pop() else {
                break;
            };
            if self.drawing.contains_key(&page) {
                continue;
            }
            self.drawing.insert(page, scale);
            let doc = opened.doc.clone();
            let (generation, revision) = (self.generation, self.revision);
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { draw(doc.as_ref(), page, scale, colors) })
                    .await;
                this.update(cx, |this, cx| {
                    if this.generation != generation || this.revision != revision {
                        if let Some(image) = image {
                            this.released.push(image);
                        }
                        if this.generation == generation && this.drawing.get(&page) == Some(&scale)
                        {
                            // Drawn from the file before it changed:
                            // asked for again on the next frame.
                            this.drawing.remove(&page);
                            cx.notify();
                        }
                        return;
                    }
                    this.drawing.remove(&page);
                    if let Some(image) = image {
                        if this.wanted().contains(&page) {
                            let drawn = Drawn {
                                scale,
                                colors,
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
        let transform = opened.doc.page_transform(page);
        let shown = self.shown;
        let device = shown * window.scale_factor();
        let image = self.pages.get(&page).map(|d| d.image.clone());
        self.laid_out(page);
        self.want(page, device, cx);
        // The form's fields to fill in, and the one being typed in.
        let fields: Vec<AnyElement> = match (&self.form, transform) {
            (Some(form), Some(transform)) => {
                let tint = self.theme.katna.accent & 0xffff_ff00;
                form.on_page(page)
                    .map(|(f, w, widget)| {
                        let b = katna_pdf_render::map_rect(transform, widget.rect);
                        let area = div()
                            .id(("field", f * 1000 + w))
                            .absolute()
                            .left(desktop_px(b[0] * shown))
                            .top(desktop_px(b[1] * shown))
                            .w(desktop_px((b[2] - b[0]) * shown))
                            .h(desktop_px((b[3] - b[1]) * shown));
                        let typing = form
                            .editing
                            .as_ref()
                            .filter(|e| e.field == f && e.widget == w);
                        if let Some(editing) = typing {
                            let size = ((b[3] - b[1]) * 0.7).clamp(4.0, 12.0) * shown;
                            return area
                                .flex()
                                .items_center()
                                .px(desktop_px(2.0 * shown))
                                .bg(self.paper())
                                .border_1()
                                .border_color(gpui::rgb(self.theme.katna.accent >> 8))
                                .text_size(desktop_px(size))
                                .text_color({
                                    let [r, g, b] = self.page_colors().ink();
                                    gpui::rgb(u32::from_be_bytes([0, r, g, b]))
                                })
                                .cursor(gpui::CursorStyle::IBeam)
                                // Presses inside stay in the field.
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(editing.input.clone())
                                .into_any_element();
                        }
                        area.bg(gpui::rgba(tint | 0x14))
                            .hover(move |s| s.bg(gpui::rgba(tint | 0x33)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.form_click(f, w, window, cx)
                            }))
                            .into_any_element()
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        // The page's links, as areas to click.
        let links: Vec<AnyElement> =
            match (self.links.as_ref().and_then(|l| l.get(page)), transform) {
                (Some(links), Some(transform)) => links
                    .iter()
                    .enumerate()
                    .map(|(i, link)| {
                        let b = katna_pdf_render::map_rect(transform, link.rect);
                        let target = link.target.clone();
                        let area = div()
                            .id(("link", i))
                            .absolute()
                            .left(desktop_px(b[0] * shown))
                            .top(desktop_px(b[1] * shown))
                            .w(desktop_px((b[2] - b[0]) * shown))
                            .h(desktop_px((b[3] - b[1]) * shown))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| this.follow(&target, cx)));
                        match &link.target {
                            katna_pdf_cos::LinkTarget::Uri(uri) => {
                                area.tip(uri.clone(), &self.theme.katna).into_any_element()
                            }
                            katna_pdf_cos::LinkTarget::Page(_) => area.into_any_element(),
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            };
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
            .w(desktop_px(w * shown))
            .h(desktop_px(h * shown))
            .flex_shrink_0()
            .bg(self.paper())
            .shadow_sm()
            .relative()
            .cursor(gpui::CursorStyle::IBeam)
            .children(image.map(|image| img(ImageSource::Render(image)).size_full()))
            .children(marks)
            .children(links)
            .children(fields)
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
            })
            .into_any_element()
    }

    /// Row `row` of the page list: its pages side by side, centred.
    fn render_row(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = self.page_count();
        let last = row + 1 == self.spread.rows(count);
        let pages: Vec<AnyElement> = self
            .spread
            .pages_in(row, count)
            .map(|page| self.render_page(page, window, cx))
            .collect();
        div()
            .w_full()
            .flex()
            .justify_center()
            .items_start()
            .gap(px(GAP))
            .pt(px(if row == 0 { space::S5 } else { GAP / 2.0 }))
            .pb(px(if last { space::S5 } else { GAP / 2.0 }))
            .children(pages)
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
            )
            .when(self.dirty(), |bar| {
                bar.child(
                    widgets::filled_button("save", t("save", &[]), &th.katna)
                        .tip(t("save-tip", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
            });
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
                .child(icon_button(
                    "fit-page",
                    "fit-page",
                    t("zoom-fit-page", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.set_zoom(Zoom::FitPage, cx)),
                ))
                .child(icon_button(
                    "spread",
                    "spread",
                    t(
                        match self.spread {
                            Spread::Single => "spread-single",
                            Spread::Facing => "spread-facing",
                            Spread::Book => "spread-book",
                        },
                        &[],
                    ),
                    th,
                    cx.listener(|this, _, _, cx| this.next_spread(cx)),
                ))
                .child(separator(th))
                .child(icon_button(
                    "print",
                    "print",
                    t("print", &[]),
                    th,
                    cx.listener(|this, _, _, cx| this.print(cx)),
                ))
                .child(
                    div()
                        .relative()
                        .child({
                            let button = widgets::icon_button("night", "contrast", 18.0, &th.katna);
                            // No tip over the open menu.
                            let button = if self.colors_menu {
                                button
                            } else {
                                button.tip(t("page-colors", &[]), &th.katna)
                            };
                            button.on_click(cx.listener(|this, _, _, cx| {
                                let just_closed = this
                                    .colors_menu_closed
                                    .is_some_and(|t| t.elapsed().as_millis() < 300);
                                this.colors_menu = !this.colors_menu && !just_closed;
                                cx.notify();
                            }))
                        })
                        .children(self.colors_menu(cx)),
                );
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
            Content::Locked { name, wrong } => {
                let field = self.password.as_ref().map(|(input, _)| {
                    div()
                        .w(px(280.0))
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .px(px(space::S3))
                        .rounded(px(radius::SM))
                        .bg(th.field)
                        .text_size(px(text::BODY))
                        .text_color(th.text)
                        .child(input.clone())
                });
                body.child(
                    svg()
                        .path("icons/lock.svg")
                        .size(px(48.0))
                        .text_color(th.dim),
                )
                .child(
                    div()
                        .text_size(px(text::TITLE))
                        .text_color(th.text)
                        .text_center()
                        .child(t("locked-title", &[("name", name)])),
                )
                .child(
                    div()
                        .text_size(px(text::SMALL))
                        .text_color(if *wrong { th.error } else { th.dim })
                        .child(t(
                            if *wrong {
                                "locked-wrong"
                            } else {
                                "locked-detail"
                            },
                            &[],
                        )),
                )
                .children(field)
                .child(
                    widgets::filled_button("unlock", t("locked-open", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.unlock(cx))),
                )
            }
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
            let folder = entry.path.parent().map(short_folder).unwrap_or_default();
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
        self.top = self.current_page();
        for image in self.released.drain(..) {
            window.drop_image(image).ok();
        }
        if std::mem::take(&mut self.focus_password)
            && let Some((input, _)) = &self.password
        {
            window.focus(&input.focus_handle(cx), cx);
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
        // The pages' view: as laid out last frame, or the window less the
        // toolbar at first.
        let view_height = self.view_size.get().map_or_else(
            || {
                unpx(window.viewport_size().height) * katna_ui::scale::scale()
                    - (2.0 * space::S3 + 48.0) * katna_ui::scale::scale()
            },
            |size| unpx(size.height) * katna_ui::scale::scale(),
        );
        let shown = self.points_to_pixels(card_width, view_height);
        if (shown / self.shown - 1.0).abs() > 0.001 {
            self.shown = shown;
            self.list.remeasure();
        }
        let pane = self.pane(window, cx);
        let find_bar = self.find_bar(cx);
        let notice = self.notice.clone().map(|notice| {
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S3))
                .py(px(space::S2))
                .border_b_1()
                .border_color(th.line)
                .text_size(px(text::SMALL))
                .text_color(th.error)
                .child(div().flex_1().min_w_0().child(notice))
                .child(icon_button(
                    "notice-close",
                    "close",
                    t("notice-close", &[]),
                    th,
                    cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    }),
                ))
        });
        let pages: AnyElement = match &self.content {
            Content::Ready(_) => {
                // Reading progress: the page at the top, noted as it
                // changes.
                if let Some(path) = self.path.clone() {
                    History::opened(&path, self.current_page(), cx);
                }
                let pages = list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_row(ix, window, cx)),
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
                    .child({
                        // Notes the view's size, and zooms with Ctrl and
                        // the wheel before the list scrolls with it.
                        let size = self.view_size.clone();
                        let weak = cx.entity().downgrade();
                        gpui::canvas(
                            move |bounds, _, _| {
                                size.set(Some(bounds.size));
                                bounds
                            },
                            move |_, bounds, window, _| {
                                window.on_mouse_event(
                                    move |event: &gpui::ScrollWheelEvent, phase, _, cx| {
                                        if phase == gpui::DispatchPhase::Capture
                                            && event.modifiers.control
                                            && bounds.contains(&event.position)
                                        {
                                            cx.stop_propagation();
                                            weak.update(cx, |this, cx| {
                                                this.wheel_zoom(event.delta, cx)
                                            })
                                            .ok();
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .size_full()
                    })
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
            .on_action(cx.listener(|this, _: &FitPage, _, cx| this.set_zoom(Zoom::FitPage, cx)))
            .on_action(
                cx.listener(|this, _: &ActualSize, _, cx| this.set_zoom(Zoom::Fixed(1.0), cx)),
            )
            .on_action(cx.listener(|this, _: &NextPage, _, cx| this.step_page(true, cx)))
            .on_action(cx.listener(|this, _: &PreviousPage, _, cx| this.step_page(false, cx)))
            .on_action(cx.listener(|this, _: &NextSpread, _, cx| this.next_spread(cx)))
            .on_action(cx.listener(|this, _: &Print, _, cx| this.print(cx)))
            .on_action(cx.listener(|this, _: &Save, _, cx| this.save(cx)))
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
                    .children(notice)
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

/// Writes `bytes` over the file at `path`: to a new file beside it
/// first, then moved over it, so a failure leaves the old one whole.
fn save_over(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut new = path.as_os_str().to_owned();
    new.push(".katna-new");
    let new = PathBuf::from(new);
    std::fs::write(&new, bytes)?;
    // Keep the file's permissions.
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&new, meta.permissions()).ok();
    }
    std::fs::rename(&new, path).inspect_err(|_| {
        std::fs::remove_file(&new).ok();
    })
}

/// A folder as people read it: the home folder as `~`.
fn short_folder(folder: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| folder.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => folder.display().to_string(),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Page `page` drawn at `scale` in `colors`, ready to show.
fn draw(
    doc: &dyn Document,
    page: usize,
    scale: f32,
    colors: PageColors,
) -> Option<Arc<RenderImage>> {
    let mut bitmap = doc.render(page, scale)?;
    let pictures = if colors == PageColors::Night {
        doc.pictures(page, scale)
    } else {
        Vec::new()
    };
    colors.apply(&mut bitmap, &pictures);
    render_image(bitmap)
}

/// The id of a page colours mode's name.
fn colors_text(colors: PageColors) -> &'static str {
    match colors {
        PageColors::Normal => "colors-normal",
        PageColors::Night => "colors-night",
        PageColors::Invert => "colors-invert",
        PageColors::InvertLightness => "colors-invert-lightness",
        PageColors::Sepia => "colors-sepia",
        PageColors::BlackWhite => "colors-black-white",
    }
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
