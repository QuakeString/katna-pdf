// SPDX-License-Identifier: GPL-3.0-or-later

//! The library (docs/PLAN.md §4.6): the PDFs in the folders a person
//! watches, as covers in a grid, with places down the side (all, opened
//! lately, starred, each folder, tag and smart collection, copies and
//! missing files), a search over titles and every page's text, and a
//! strip for the selected document's stars and tags. The catalog is
//! `katna_pdf_library`'s; folders are read again in the background, through
//! the app's worker processes, each time the library is shown.

use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ImageSource,
    PathPromptOptions, RenderImage, Subscription, UniformListScrollHandle, Window, div, img,
    prelude::*, svg, uniform_list,
};
use image::{Frame, RgbaImage};
use katna_pdf_library::{Doc, Filter, Hit, Library, Sort};
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets::{self, Tip};
use katna_ui::{px, unpx};

use crate::text::t;
use crate::theme::Theme;

/// A cover's width, and its box's height, in logical pixels.
const COVER: f32 = 140.0;
const COVER_BOX: f32 = 180.0;
/// A grid cell: the cover's box, its name and what it is, and the gap.
const CELL_WIDTH: f32 = COVER + 2.0 * space::S3;
const CELL_HEIGHT: f32 = COVER_BOX + 64.0;
/// Covers kept drawn; beyond this the ones farthest from view are freed.
const KEEP: usize = 240;
/// How long a folder must be quiet after a change before it is read
/// again, so a copy of many files is read once.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);
/// Pages found by a search, at most.
const HITS: usize = 40;
const HITS_SHOWN: usize = 6;
const SIDE_WIDTH: f32 = 248.0;

/// What the library asks of the window.
pub enum LibraryEvent {
    /// Open a file, at a page when given (from 0).
    Open(PathBuf, Option<usize>),
    /// Back to the documents.
    Close,
}

/// A place down the side.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    All,
    Recent,
    Starred,
    Folder(PathBuf),
    Tag(String),
    Collection(String),
    Copies,
    Missing,
}

/// What a place's remove button does.
type Remove = Box<dyn Fn(&mut LibraryView, &mut Context<LibraryView>)>;

/// How far a scan has come.
enum Progress {
    Reading(usize, usize),
    Done(Result<katna_pdf_library::ScanReport, String>),
}

pub struct LibraryView {
    focus: FocusHandle,
    theme: Theme,
    lib: Result<Library, String>,
    place: Place,
    sort: Sort,
    search: Entity<TextInput>,
    docs: Vec<Doc>,
    hits: Vec<(Hit, String)>,
    folders: Vec<(PathBuf, usize)>,
    tags: Vec<(String, usize)>,
    collections: Vec<(String, String)>,
    counts: Counts,
    covers: HashMap<i64, Arc<RenderImage>>,
    /// Covers to free on the next frame (that needs the window).
    released: Vec<Arc<RenderImage>>,
    /// Covers there are none of, not asked for again.
    no_cover: std::collections::HashSet<i64>,
    scanning: Option<(usize, usize)>,
    /// Asked to scan again while scanning.
    scan_again: bool,
    /// What the last scan could not read, to say so.
    failed: usize,
    selected: Option<i64>,
    tags_field: Entity<TextInput>,
    grid: UniformListScrollHandle,
    /// The grid's width last frame, for how many covers a row holds.
    width: Rc<Cell<f32>>,
    /// Watches the folders; dropped and made again when they change.
    watcher: Option<notify::RecommendedWatcher>,
    _inputs: [Subscription; 2],
}

#[derive(Default)]
struct Counts {
    all: usize,
    recent: usize,
    starred: usize,
    copies: usize,
    missing: usize,
}

impl LibraryView {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        let accent = gpui::rgba(theme.katna.accent).into();
        let search = cx.new(|cx| {
            let mut input = TextInput::new(t("library-search", &[]), cx);
            input.set_accent(accent);
            input
        });
        let tags_field = cx.new(|cx| {
            let mut input = TextInput::new(t("library-tags-placeholder", &[]), cx);
            input.set_accent(accent);
            input
        });
        let searching = cx.subscribe(&search, |this, _, event, cx| match event {
            InputEvent::Changed | InputEvent::Submit => this.reload(cx),
            InputEvent::Cancel => cx.emit(LibraryEvent::Close),
        });
        let tagging = cx.subscribe(&tags_field, |this, _, event, cx| {
            if let InputEvent::Submit = event {
                this.save_tags(cx);
            }
        });
        let mut this = Self {
            focus: cx.focus_handle(),
            theme,
            lib: Library::open_default().map_err(|e| e.to_string()),
            place: Place::All,
            sort: Sort::Title,
            search,
            docs: Vec::new(),
            hits: Vec::new(),
            folders: Vec::new(),
            tags: Vec::new(),
            collections: Vec::new(),
            counts: Counts::default(),
            covers: HashMap::new(),
            released: Vec::new(),
            no_cover: Default::default(),
            scanning: None,
            scan_again: false,
            failed: 0,
            selected: None,
            tags_field,
            grid: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(800.0)),
            watcher: None,
            _inputs: [searching, tagging],
        };
        this.reload(cx);
        this.watch(cx);
        this
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// The library is shown: its folders are read again.
    pub fn shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus_handle(cx), cx);
        self.reload(cx);
        self.scan(cx);
    }

    /// Watches the folders, to read them again when a PDF in them is
    /// added, changed, moved or taken away.
    fn watch(&mut self, cx: &mut Context<Self>) {
        use notify::{EventKind, RecursiveMode, Watcher};
        self.watcher = None;
        let folders = self
            .lib
            .as_ref()
            .ok()
            .and_then(|lib| lib.folders().ok())
            .unwrap_or_default();
        if folders.is_empty() {
            return;
        }
        let (sender, changes) = async_channel::unbounded();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            let pdf = event
                .paths
                .iter()
                .any(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
            // A folder moved or taken away may hold PDFs.
            let gone = matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
            );
            if (pdf || gone) && !matches!(event.kind, EventKind::Access(_)) {
                let _ = sender.try_send(());
            }
        });
        let Ok(mut watcher) = watcher else {
            return;
        };
        for folder in &folders {
            let _ = watcher.watch(folder, RecursiveMode::Recursive);
        }
        self.watcher = Some(watcher);
        cx.spawn(async move |this, cx| {
            // Ends when the watcher, and so its sender, is dropped.
            while changes.recv().await.is_ok() {
                cx.background_executor().timer(SETTLE).await;
                while changes.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.scan(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads the folders in the background; the grid fills in as it goes.
    fn scan(&mut self, cx: &mut Context<Self>) {
        let Ok(lib) = &self.lib else {
            return;
        };
        if lib.folders().map_or(true, |f| f.is_empty()) {
            return;
        }
        if self.scanning.is_some() {
            self.scan_again = true;
            return;
        }
        self.scanning = Some((0, 0));
        let (sender, progress) = async_channel::unbounded();
        cx.background_spawn(async move {
            // Its own connection: the catalog is shared with the view.
            let result = Library::open_default().and_then(|lib| {
                lib.scan(crate::engine::engine(), &mut |done, total, _| {
                    let _ = sender.try_send(Progress::Reading(done, total));
                })
            });
            let _ = sender.try_send(Progress::Done(result.map_err(|e| e.to_string())));
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let mut last = 0;
            while let Ok(progress) = progress.recv().await {
                let stop = this
                    .update(cx, |this, cx| match progress {
                        Progress::Reading(done, total) => {
                            this.scanning = Some((done, total));
                            // New covers show every so often.
                            if done >= last + 24 {
                                last = done;
                                this.reload(cx);
                            }
                            cx.notify();
                            false
                        }
                        Progress::Done(result) => {
                            this.scanning = None;
                            this.failed = result.as_ref().map_or(0, |r| r.failed.len());
                            // Covers of changed files are read again.
                            this.no_cover.clear();
                            let old: Vec<_> = this.covers.drain().map(|(_, c)| c).collect();
                            this.released.extend(old);
                            this.reload(cx);
                            if std::mem::take(&mut this.scan_again) {
                                this.scan(cx);
                            }
                            true
                        }
                    })
                    .unwrap_or(true);
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    /// Reads the place's documents, the search's hits and the side's
    /// counts from the catalog again.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().to_owned();
        let Ok(lib) = &self.lib else {
            return;
        };
        let mut filter = Filter::parse(&query);
        if filter.sort == Sort::default() {
            filter.sort = self.sort;
        }
        let mut copies = Vec::new();
        match &self.place {
            Place::All => {}
            Place::Recent => filter.sort = Sort::Opened,
            Place::Starred => filter.rating = Some(filter.rating.unwrap_or(1).max(1)),
            Place::Folder(path) => filter.folder = Some(path.to_string_lossy().into_owned()),
            Place::Tag(tag) => filter.tags.push(tag.clone()),
            Place::Collection(name) => {
                if let Some((_, saved)) = self.collections.iter().find(|(n, _)| n == name) {
                    let saved = Filter::parse(saved);
                    filter.words.extend(saved.words);
                    filter.tags.extend(saved.tags);
                    filter.author = filter.author.or(saved.author);
                    filter.signed = filter.signed.or(saved.signed);
                    filter.forms = filter.forms.or(saved.forms);
                    filter.scanned = filter.scanned.or(saved.scanned);
                    filter.locked = filter.locked.or(saved.locked);
                    filter.missing = filter.missing.or(saved.missing);
                    filter.rating = filter.rating.or(saved.rating);
                    filter.folder = filter.folder.or(saved.folder);
                    if saved.sort != Sort::default() {
                        filter.sort = saved.sort;
                    }
                }
            }
            Place::Copies => copies = lib.duplicates().unwrap_or_default(),
            Place::Missing => filter.missing = Some(true),
        }
        self.docs = if self.place == Place::Copies {
            // Each group together, the groups by their first's name.
            copies.into_iter().flatten().collect()
        } else {
            let mut docs = lib.docs(&filter).unwrap_or_default();
            // Documents whose text has the words count as found too.
            if !filter.words.is_empty() {
                let found: std::collections::HashSet<i64> = lib
                    .search_docs(&filter.words.join(" "))
                    .unwrap_or_default()
                    .into_iter()
                    .chain(docs.iter().map(|d| d.id))
                    .collect();
                let unworded = Filter {
                    words: Vec::new(),
                    ..filter.clone()
                };
                docs = lib.docs(&unworded).unwrap_or_default();
                docs.retain(|d| found.contains(&d.id));
            }
            if self.place == Place::Recent {
                docs.retain(|d| d.opened.is_some());
            }
            docs
        };
        self.hits = if filter.words.is_empty() {
            Vec::new()
        } else {
            let names: HashMap<i64, String> = lib
                .docs(&Filter::default())
                .unwrap_or_default()
                .into_iter()
                .map(|d| (d.id, d.name()))
                .collect();
            lib.search(&filter.words.join(" "), HITS)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|hit| {
                    let name = names.get(&hit.doc)?.clone();
                    Some((hit, name))
                })
                .collect()
        };
        let all = lib.docs(&Filter::default()).unwrap_or_default();
        self.folders = lib
            .folders()
            .unwrap_or_default()
            .into_iter()
            .map(|f| {
                let n = all.iter().filter(|d| d.path.starts_with(&f)).count();
                (f, n)
            })
            .collect();
        self.tags = lib.tags().unwrap_or_default();
        self.collections = lib.collections().unwrap_or_default();
        self.counts = Counts {
            all: all.len(),
            recent: all.iter().filter(|d| d.opened.is_some()).count(),
            starred: all.iter().filter(|d| d.rating > 0).count(),
            copies: lib.duplicates().map_or(0, |g| g.iter().map(Vec::len).sum()),
            missing: lib
                .docs(&Filter {
                    missing: Some(true),
                    ..Filter::default()
                })
                .map_or(0, |d| d.len()),
        };
        if self
            .selected
            .is_some_and(|id| !self.docs.iter().any(|d| d.id == id))
        {
            self.selected = None;
        }
        cx.notify();
    }

    fn go(&mut self, place: Place, cx: &mut Context<Self>) {
        self.place = place;
        self.selected = None;
        self.grid = UniformListScrollHandle::new();
        self.reload(cx);
    }

    fn add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                this.update(cx, |this, cx| {
                    if let Ok(lib) = &this.lib {
                        for path in &paths {
                            let _ = lib.add_folder(path);
                        }
                    }
                    this.reload(cx);
                    this.watch(cx);
                    this.scan(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn remove_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if let Ok(lib) = &self.lib {
            let _ = lib.remove_folder(&folder);
        }
        if self.place == Place::Folder(folder) {
            self.place = Place::All;
        }
        self.watch(cx);
        self.reload(cx);
    }

    /// Keeps the search typed as a smart collection, named by it.
    fn keep_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).text().trim().to_owned();
        if query.is_empty() {
            return;
        }
        if let Ok(lib) = &self.lib {
            let _ = lib.set_collection(&query, &query);
        }
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.collections = self
            .lib
            .as_ref()
            .ok()
            .and_then(|l| l.collections().ok())
            .unwrap_or_default();
        self.go(Place::Collection(query), cx);
    }

    fn remove_collection(&mut self, name: String, cx: &mut Context<Self>) {
        if let Ok(lib) = &self.lib {
            let _ = lib.set_collection(&name, "");
        }
        if self.place == Place::Collection(name) {
            self.place = Place::All;
        }
        self.reload(cx);
    }

    fn select(&mut self, id: i64, cx: &mut Context<Self>) {
        self.selected = Some(id);
        let tags = self
            .docs
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.tags.join(", "))
            .unwrap_or_default();
        self.tags_field.update(cx, |f, cx| f.set_text(tags, cx));
        cx.notify();
    }

    fn open(&mut self, id: i64, page: Option<usize>, cx: &mut Context<Self>) {
        let Ok(lib) = &self.lib else {
            return;
        };
        let Some(doc) = lib.doc(id).ok().flatten() else {
            return;
        };
        if doc.missing {
            self.select(id, cx);
            return;
        }
        let _ = lib.mark_opened(id);
        cx.emit(LibraryEvent::Open(doc.path, page));
    }

    fn rate(&mut self, id: i64, stars: u8, cx: &mut Context<Self>) {
        if let Ok(lib) = &self.lib {
            let _ = lib.set_rating(id, stars);
        }
        self.reload(cx);
    }

    fn save_tags(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else {
            return;
        };
        let tags: Vec<String> = self
            .tags_field
            .read(cx)
            .text()
            .split(',')
            .map(|t| t.trim().to_owned())
            .collect();
        if let Ok(lib) = &self.lib {
            let _ = lib.set_tags(id, &tags);
        }
        self.reload(cx);
    }

    fn forget(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Ok(lib) = &self.lib {
            let _ = lib.forget(id);
        }
        self.selected = None;
        self.reload(cx);
    }

    /// Document `id`'s cover, read from the catalog on first need.
    fn cover(&mut self, id: i64, near: &Range<usize>) -> Option<Arc<RenderImage>> {
        if let Some(cover) = self.covers.get(&id) {
            return Some(cover.clone());
        }
        if self.no_cover.contains(&id) {
            return None;
        }
        let cover = self
            .lib
            .as_ref()
            .ok()
            .and_then(|lib| lib.cover(id).ok().flatten())
            .and_then(|cover| {
                let mut rgba = cover.rgba;
                // GPUI draws BGRA.
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let image = RgbaImage::from_raw(cover.width, cover.height, rgba)?;
                Some(Arc::new(RenderImage::new([Frame::new(image)])))
            });
        let Some(cover) = cover else {
            self.no_cover.insert(id);
            return None;
        };
        if self.covers.len() >= KEEP {
            // Free the covers of documents out of view.
            let shown: std::collections::HashSet<i64> = self
                .docs
                .get(near.clone())
                .unwrap_or_default()
                .iter()
                .map(|d| d.id)
                .collect();
            let old: Vec<i64> = self
                .covers
                .keys()
                .filter(|k| !shown.contains(k))
                .copied()
                .collect();
            for k in old {
                if let Some(c) = self.covers.remove(&k) {
                    self.released.push(c);
                }
            }
        }
        self.covers.insert(id, cover.clone());
        Some(cover)
    }

    fn columns(&self) -> usize {
        ((self.width.get() - 2.0 * space::S4) / CELL_WIDTH)
            .floor()
            .max(1.0) as usize
    }

    fn title(&self) -> String {
        match &self.place {
            Place::All => t("library-all", &[]),
            Place::Recent => t("library-recent", &[]),
            Place::Starred => t("library-starred", &[]),
            Place::Folder(path) => folder_name(path),
            Place::Tag(tag) => tag.clone(),
            Place::Collection(name) => name.clone(),
            Place::Copies => t("library-copies", &[]),
            Place::Missing => t("library-missing", &[]),
        }
    }
}

impl EventEmitter<LibraryEvent> for LibraryView {}

impl Focusable for LibraryView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.released.drain(..) {
            window.drop_image(image).ok();
        }
        let th = self.theme;
        div()
            .id("library")
            .key_context("Library")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(LibraryEvent::Close);
                }
            }))
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(space::S3))
            .child(self.side(cx))
            .child(self.main(cx))
            .text_color(th.text)
    }
}

impl LibraryView {
    fn side(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let mut side = div()
            .id("library-side")
            .w(px(SIDE_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .py(px(space::S2))
            .pr(px(space::S2))
            .rounded(px(radius::MD))
            .bg(th.card)
            .overflow_y_scroll();
        let counts = [
            (
                Place::All,
                "library",
                t("library-all", &[]),
                self.counts.all,
            ),
            (
                Place::Recent,
                "clock",
                t("library-recent", &[]),
                self.counts.recent,
            ),
            (
                Place::Starred,
                "star",
                t("library-starred", &[]),
                self.counts.starred,
            ),
        ];
        for (i, (place, icon, name, n)) in counts.into_iter().enumerate() {
            side = side.child(self.place_row(("place", i), place, icon, name, Some(n), None, cx));
        }
        let th = self.theme;
        let add = widgets::icon_button("library-add-folder", "plus", 16.0, &th.katna)
            .size(px(28.0))
            .tip(t("library-add-folder", &[]), &th.katna)
            .on_click(cx.listener(|this, _, window, cx| this.add_folder(window, cx)))
            .into_any_element();
        side = side.child(self.heading("library-folders", Some(add)));
        if self.folders.is_empty() {
            side = side.child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S1))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("library-no-folders", &[])),
            );
        }
        for (i, (folder, n)) in self.folders.clone().into_iter().enumerate() {
            let remove = folder.clone();
            side = side.child(self.place_row(
                ("folder", i),
                Place::Folder(folder.clone()),
                "folder",
                folder_name(&folder),
                Some(n),
                Some((
                    "library-remove-folder",
                    Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                        this.remove_folder(remove.clone(), cx)
                    }),
                )),
                cx,
            ));
        }
        if !self.tags.is_empty() {
            side = side.child(self.heading("library-tags", None));
            for (i, (tag, n)) in self.tags.clone().into_iter().enumerate() {
                side = side.child(self.place_row(
                    ("tag", i),
                    Place::Tag(tag.clone()),
                    "tag",
                    tag,
                    Some(n),
                    None,
                    cx,
                ));
            }
        }
        if !self.collections.is_empty() {
            side = side.child(self.heading("library-collections", None));
            for (i, (name, _)) in self.collections.clone().into_iter().enumerate() {
                let remove = name.clone();
                side = side.child(self.place_row(
                    ("collection", i),
                    Place::Collection(name.clone()),
                    "search",
                    name,
                    None,
                    Some((
                        "library-remove-collection",
                        Box::new(move |this: &mut Self, cx: &mut Context<Self>| {
                            this.remove_collection(remove.clone(), cx)
                        }),
                    )),
                    cx,
                ));
            }
        }
        if self.counts.copies > 0 || self.counts.missing > 0 {
            side = side.child(self.heading("library-tidy", None));
            if self.counts.copies > 0 {
                side = side.child(self.place_row(
                    ("place", 10),
                    Place::Copies,
                    "copies",
                    t("library-copies", &[]),
                    Some(self.counts.copies),
                    None,
                    cx,
                ));
            }
            if self.counts.missing > 0 {
                side = side.child(self.place_row(
                    ("place", 11),
                    Place::Missing,
                    "shield-alert",
                    t("library-missing", &[]),
                    Some(self.counts.missing),
                    None,
                    cx,
                ));
            }
        }
        side
    }

    fn heading(&self, id: &'static str, action: Option<AnyElement>) -> impl IntoElement {
        let th = self.theme;
        div()
            .flex()
            .items_center()
            .pl(px(space::S4))
            .pt(px(space::S3))
            .pb(px(space::S1))
            .child(
                div()
                    .flex_1()
                    .text_size(px(text::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(th.dim)
                    .child(t(id, &[])),
            )
            .children(action)
    }

    #[allow(clippy::too_many_arguments)]
    fn place_row(
        &self,
        id: (&'static str, usize),
        place: Place,
        icon: &'static str,
        name: String,
        count: Option<usize>,
        remove: Option<(&'static str, Remove)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let th = self.theme;
        let current = self.place == place;
        let group: gpui::SharedString = format!("{}-{}", id.0, id.1).into();
        div()
            .id(id)
            .group(group.clone())
            .h(px(36.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S4))
            .pr(px(space::S2))
            .rounded_r(px(radius::FULL))
            .cursor_pointer()
            .when(current, |d| {
                d.bg(gpui::rgba(th.katna.nav_selected))
                    .text_color(gpui::rgba(th.katna.nav_selected_text))
            })
            .when(!current, |d| d.hover(move |s| s.bg(th.hover)))
            .on_click(cx.listener(move |this, _, _, cx| this.go(place.clone(), cx)))
            .child(
                svg()
                    .path(format!("icons/{icon}.svg"))
                    .size(px(18.0))
                    .flex_none()
                    .text_color(if current {
                        gpui::rgba(th.katna.nav_selected_text).into()
                    } else {
                        th.dim
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(text::BODY))
                    .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(name),
            )
            .children(remove.map(|(tip, remove)| {
                div()
                    .invisible()
                    .group_hover(group.clone(), |s| s.visible())
                    .child(
                        widgets::icon_button((tip, id.1), "close", 14.0, &th.katna)
                            .size(px(24.0))
                            .tip(t(tip, &[]), &th.katna)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                remove(this, cx)
                            })),
                    )
            }))
            .children(count.map(|n| {
                div()
                    .flex_none()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(n.to_string())
            }))
    }

    fn main(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let has_query = !self.search.read(cx).text().trim().is_empty();
        let progress = self.scanning.map(|(done, total)| {
            let line = if total == 0 {
                t("library-scanning", &[])
            } else {
                t(
                    "library-reading",
                    &[("done", &done.to_string()), ("total", &total.to_string())],
                )
            };
            div()
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(line)
        });
        let sort_name = format!("library-sort-{}", self.sort.name());
        let header = div()
            .flex_none()
            .h(px(48.0))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S3))
            .border_b_1()
            .border_color(th.line)
            .child(
                widgets::icon_button("library-back", "chevron-left", 18.0, &th.katna)
                    .tip(t("library-back", &[]), &th.katna)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(LibraryEvent::Close))),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(text::TITLE))
                    .child(self.title()),
            )
            .children(progress)
            .child(div().flex_1())
            .child(
                div()
                    .id("library-search-box")
                    .w(px(320.0))
                    .h(px(32.0))
                    .tip(t("library-search-tip", &[]), &th.katna)
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S3))
                    .rounded(px(radius::FULL))
                    .bg(th.field)
                    .text_size(px(text::BODY))
                    .child(
                        svg()
                            .path("icons/search.svg")
                            .size(px(16.0))
                            .flex_none()
                            .text_color(th.dim),
                    )
                    .child(div().flex_1().min_w_0().child(self.search.clone())),
            )
            .when(has_query, |d| {
                d.child(
                    widgets::icon_button("library-keep", "bookmark-plus", 18.0, &th.katna)
                        .tip(t("library-keep", &[]), &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| this.keep_search(cx))),
                )
            })
            .child(
                div()
                    .id("library-sort")
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .rounded(px(radius::FULL))
                    .border_1()
                    .border_color(th.line)
                    .text_size(px(text::SMALL))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .tip(t("library-sort-tip", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let at = Sort::ALL.iter().position(|s| *s == this.sort).unwrap_or(0);
                        this.sort = Sort::ALL[(at + 1) % Sort::ALL.len()];
                        this.reload(cx);
                    }))
                    .child(t(&sort_name, &[])),
            );
        let body: AnyElement = if let Err(e) = &self.lib {
            self.message(t("library-unavailable", &[("error", e)]), None)
        } else if self.folders.is_empty() {
            let add = widgets::filled_button(
                "library-add-first-folder",
                t("library-add-folder", &[]),
                &th.katna,
            )
            .on_click(cx.listener(|this, _, window, cx| this.add_folder(window, cx)))
            .into_any_element();
            self.message(t("library-empty", &[]), Some(add))
        } else if self.docs.is_empty() && self.hits.is_empty() {
            let message = if self.scanning.is_some() {
                t("library-scanning", &[])
            } else {
                t("library-nothing", &[])
            };
            self.message(message, None)
        } else {
            self.grid(cx).into_any_element()
        };
        let hits = (!self.hits.is_empty()).then(|| self.hits(cx).into_any_element());
        let details = self.details(cx);
        let failed = (self.failed > 0 && self.scanning.is_none()).then(|| {
            div()
                .px(px(space::S4))
                .py(px(space::S1))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(if self.failed == 1 {
                    t("library-failed-one", &[])
                } else {
                    t("library-failed", &[("count", &self.failed.to_string())])
                })
        });
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .rounded(px(radius::MD))
            .bg(th.card)
            .overflow_hidden()
            .child(header)
            .children(hits)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .children(failed)
            .children(details)
    }

    fn message(&self, message: String, button: Option<AnyElement>) -> AnyElement {
        let th = self.theme;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(space::S4))
            .child(
                svg()
                    .path("icons/library.svg")
                    .size(px(48.0))
                    .text_color(th.dim),
            )
            .child(
                div()
                    .max_w(px(420.0))
                    .text_center()
                    .text_size(px(text::BODY))
                    .text_color(th.dim)
                    .child(message),
            )
            .children(button)
            .into_any_element()
    }

    /// The pages whose text holds the search's words.
    fn hits(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let rows = self
            .hits
            .iter()
            .take(HITS_SHOWN)
            .enumerate()
            .map(|(i, (hit, name))| {
                let (doc, page) = (hit.doc, hit.page);
                div()
                    .id(("hit", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S3))
                    .py(px(space::S1))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(doc, Some(page), cx)))
                    .child(
                        div()
                            .w(px(200.0))
                            .flex_none()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t(
                                "library-hit-page",
                                &[("name", name), ("page", &(page + 1).to_string())],
                            )),
                    )
                    .child(snippet(&hit.snippet, th))
            });
        let more = self.hits.len().saturating_sub(HITS_SHOWN);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .px(px(space::S3))
            .py(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                div()
                    .px(px(space::S3))
                    .pb(px(space::S1))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("library-found-inside", &[])),
            )
            .children(rows)
            .when(more > 0, |d| {
                d.child(
                    div()
                        .px(px(space::S3))
                        .text_size(px(text::SMALL))
                        .text_color(th.dim)
                        .child(if more == 1 {
                            t("library-more-hits-one", &[])
                        } else {
                            t("library-more-hits", &[("count", &more.to_string())])
                        }),
                )
            })
    }

    fn grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns();
        let rows = self.docs.len().div_ceil(columns);
        let width = self.width.clone();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        width.set(unpx(bounds.size.width) * katna_ui::scale::scale());
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                uniform_list(
                    "library-grid",
                    rows,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let near =
                            range.start * columns..(range.end * columns).min(this.docs.len());
                        range
                            .map(|row| this.grid_row(row, columns, &near, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.grid)
                .size_full()
                .px(px(space::S4))
                .py(px(space::S3)),
            )
    }

    fn grid_row(
        &mut self,
        row: usize,
        columns: usize,
        near: &Range<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let start = row * columns;
        let end = (start + columns).min(self.docs.len());
        let cells: Vec<AnyElement> = (start..end)
            .map(|i| self.cell(i, near, cx).into_any_element())
            .collect();
        div()
            .h(px(CELL_HEIGHT))
            .flex()
            .children(cells)
            .into_any_element()
    }

    fn cell(&mut self, i: usize, near: &Range<usize>, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let doc = self.docs[i].clone();
        let cover = self.cover(doc.id, near);
        let selected = self.selected == Some(doc.id);
        let id = doc.id;
        let mut marks: Vec<(&str, String)> = Vec::new();
        if doc.missing {
            marks.push(("library-mark-missing", t("library-mark-missing", &[])));
        }
        if doc.signed {
            marks.push(("library-mark-signed", t("library-mark-signed", &[])));
        }
        if doc.locked {
            marks.push(("library-mark-locked", t("library-mark-locked", &[])));
        }
        if doc.forms {
            marks.push(("library-mark-form", t("library-mark-form", &[])));
        }
        if doc.scanned {
            marks.push(("library-mark-scanned", t("library-mark-scanned", &[])));
        }
        let accent = gpui::rgba(th.katna.accent);
        div()
            .id(("doc", doc.id as u64 as usize))
            .w(px(CELL_WIDTH))
            .h(px(CELL_HEIGHT))
            .flex_none()
            .p(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .when(selected, |d| d.bg(gpui::rgba(th.katna.row_selected)))
            .when(!selected, |d| d.hover(move |s| s.bg(th.hover)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.open(id, None, cx);
                } else {
                    this.select(id, cx);
                }
            }))
            .child(
                div()
                    .h(px(COVER_BOX))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_end()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .bg(th.backdrop)
                    .overflow_hidden()
                    .child(match cover {
                        Some(cover) => {
                            let (w, h) = cover_size(&cover);
                            div()
                                .w(px(w))
                                .h(px(h))
                                .mb(px(space::S2))
                                .bg(gpui::white())
                                .shadow(widgets::elevation(&th.katna, 1.0))
                                .when(doc.missing, |d| d.opacity(0.4))
                                .child(img(ImageSource::Render(cover)).size_full())
                                .into_any_element()
                        }
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                svg()
                                    .path(if doc.locked {
                                        "icons/lock.svg"
                                    } else {
                                        "icons/document.svg"
                                    })
                                    .size(px(36.0))
                                    .text_color(th.dim),
                            )
                            .into_any_element(),
                    })
                    .children(marks.first().map(|(_, mark)| {
                        div()
                            .absolute()
                            .top(px(space::S2))
                            .left(px(space::S2))
                            .px(px(space::S2))
                            .rounded(px(radius::FULL))
                            .bg(th.card)
                            .text_size(px(text::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if doc.missing { th.error } else { accent.into() })
                            .child(mark.clone())
                    })),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(doc.name()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(doc_line(&doc)),
            )
    }

    /// The selected document's strip: where it is, its stars and tags.
    fn details(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let id = self.selected?;
        let doc = self.docs.iter().find(|d| d.id == id)?.clone();
        let th = self.theme;
        let stars = (1..=5u8).map(|n| {
            let on = doc.rating >= n;
            widgets::icon_button_colored(
                ("library-star", n as usize),
                if on { "star-filled" } else { "star" },
                18.0,
                if on { th.katna.star } else { th.katna.text_dim },
                &th.katna,
            )
            .size(px(28.0))
            .tip(t("library-stars", &[("count", &n.to_string())]), &th.katna)
            .on_click(cx.listener(move |this, _, _, cx| {
                // The star given again takes the stars away.
                let stars = if doc.rating == n { 0 } else { n };
                this.rate(id, stars, cx)
            }))
        });
        let path = doc.path.to_string_lossy().into_owned();
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(space::S4))
                .py(px(space::S2))
                .border_t_1()
                .border_color(th.line)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::BODY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(doc.name()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::SMALL))
                                .text_color(if doc.missing { th.error } else { th.dim })
                                .child(if doc.missing {
                                    t("library-gone", &[("path", &path)])
                                } else {
                                    path
                                }),
                        ),
                )
                .child(div().flex().children(stars))
                .child(
                    div()
                        .w(px(240.0))
                        .h(px(32.0))
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .px(px(space::S3))
                        .rounded(px(radius::SM))
                        .bg(th.field)
                        .text_size(px(text::SMALL))
                        .child(
                            svg()
                                .path("icons/tag.svg")
                                .size(px(14.0))
                                .flex_none()
                                .text_color(th.dim),
                        )
                        .child(div().flex_1().min_w_0().child(self.tags_field.clone())),
                )
                .map(|d| {
                    if doc.missing {
                        d.child(
                            widgets::outlined_button(
                                "library-forget",
                                t("library-forget", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.forget(id, cx))),
                        )
                    } else {
                        d.child(
                            widgets::filled_button(
                                "library-open",
                                t("library-open", &[]),
                                &th.katna,
                            )
                            .on_click(cx.listener(move |this, _, _, cx| this.open(id, None, cx))),
                        )
                    }
                }),
        )
    }
}

/// A cover's size in its box, keeping its shape.
fn cover_size(cover: &RenderImage) -> (f32, f32) {
    let size = cover.size(0);
    let (w, h) = (size.width.0 as f32, size.height.0 as f32);
    let fit = (COVER / w.max(1.0)).min((COVER_BOX - 2.0 * space::S2) / h.max(1.0));
    (w * fit, h * fit)
}

/// A search's words around a match, the match (between `[` and `]`) in
/// bold.
fn snippet(snippet: &str, th: Theme) -> impl IntoElement {
    let flat = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts = Vec::new();
    let mut rest = flat.as_str();
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        parts.push((rest[..open].to_owned(), false));
        parts.push((rest[open + 1..open + close].to_owned(), true));
        rest = &rest[open + close + 1..];
    }
    parts.push((rest.to_owned(), false));
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(text::SMALL))
        .text_color(th.dim)
        .children(
            parts
                .into_iter()
                .filter(|(s, _)| !s.is_empty())
                .map(move |(s, bold)| {
                    div()
                        .flex_none()
                        .when(bold, |d| {
                            d.font_weight(FontWeight::BOLD).text_color(th.text)
                        })
                        .child(s.replace(' ', "\u{a0}"))
                }),
        )
}

/// What a document is, in a line: its pages and size.
fn doc_line(doc: &Doc) -> String {
    let size = size_text(doc.size);
    let id = if doc.pages == 1 {
        "library-doc-line-one"
    } else {
        "library-doc-line"
    };
    t(id, &[("pages", &doc.pages.to_string()), ("size", &size)])
}

fn size_text(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        t("size-kb", &[("n", &format!("{:.0}", kb.max(1.0)))])
    } else {
        t("size-mb", &[("n", &format!("{:.1}", kb / 1024.0))])
    }
}

fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
