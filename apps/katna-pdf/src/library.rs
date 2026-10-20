// SPDX-License-Identifier: GPL-3.0-or-later

//! The library (docs/PLAN.md §4.6): the PDFs in the folders a person
//! watches, as covers in a grid, with places down the side (all, opened
//! lately, starred, each folder, tag and smart collection, copies and
//! missing files), a search over titles and every page's text, and a
//! strip for the selected document's stars and tags. Documents chosen
//! with Ctrl or Shift are merged, signed, checked, renamed, copied or
//! tagged together. The drives of Katna Mail's accounts are browsed
//! too (`drive.rs`). The catalog is
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

mod drive;
mod mail;

use crate::drives::Drive;
use crate::text::t;
use crate::theme::Theme;
use crate::viewer::sign::{Verdict, verdict};

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
    /// Sign these files together, starting from the first.
    Sign(Vec<PathBuf>),
    /// Open the newer file and compare it with the older.
    Compare { older: PathBuf, newer: PathBuf },
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
    /// The drive of this account of Katna Mail's.
    Drive(i64),
    /// The PDFs attached to mail in Katna Mail.
    Mail,
}

impl Place {
    /// Outside the catalog: a drive, or mail.
    fn elsewhere(&self) -> bool {
        matches!(self, Place::Drive(_) | Place::Mail)
    }
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
    /// The folders were read since the app started: showing the library
    /// again does not read them again (the watcher catches changes).
    scanned: bool,
    /// What the last scan could not read, to say so.
    failed: usize,
    /// The document last clicked, where a Shift-click range starts.
    selected: Option<i64>,
    /// Every document chosen: the one clicked, or many with Ctrl and
    /// Shift.
    chosen: Vec<i64>,
    tags_field: Entity<TextInput>,
    /// The tag given to every chosen document.
    add_tag: Entity<TextInput>,
    /// The new names of the chosen documents, while renaming them.
    pattern: Entity<TextInput>,
    renaming: bool,
    /// A bulk action under way.
    busy: bool,
    /// What the last bulk action did.
    notice: Option<String>,
    /// The chosen files' signatures, once checked.
    checks: Vec<(i64, String, Verdict)>,
    grid: UniformListScrollHandle,
    /// The grid's width last frame, for how many covers a row holds.
    width: Rc<Cell<f32>>,
    /// The drives of Katna Mail's accounts, and the one shown.
    drives: Vec<Drive>,
    drive: Option<drive::DriveView>,
    /// The PDFs attached to mail, and what of them is shown.
    mail_pdfs: Vec<crate::mail::MailPdf>,
    mail: Option<mail::MailView>,
    /// Watches the folders; dropped and made again when they change.
    watcher: Option<notify::RecommendedWatcher>,
    _inputs: [Subscription; 4],
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
        let add_tag = cx.new(|cx| {
            let mut input = TextInput::new(t("library-add-tag", &[]), cx);
            input.set_accent(accent);
            input
        });
        let pattern = cx.new(|cx| {
            let mut input = TextInput::new(t("library-rename-pattern", &[]), cx);
            input.set_accent(accent);
            input.set_text("{author} - {title}", cx);
            input
        });
        let adding = cx.subscribe(&add_tag, |this, _, event, cx| match event {
            InputEvent::Submit => this.tag_chosen(cx),
            InputEvent::Cancel => this.clear_chosen(cx),
            InputEvent::Changed => {}
        });
        let naming = cx.subscribe(&pattern, |this, _, event, cx| match event {
            InputEvent::Submit => this.rename_chosen(cx),
            InputEvent::Cancel => {
                this.renaming = false;
                cx.notify();
            }
            InputEvent::Changed => cx.notify(),
        });
        let searching = cx.subscribe(&search, |this, input, event, cx| match event {
            // A drive is searched on Enter: each search asks its server.
            InputEvent::Submit if this.drive.is_some() => {
                let words = input.read(cx).text().to_owned();
                this.search_drive(&words, cx);
            }
            InputEvent::Changed if this.drive.is_some() => {
                if input.read(cx).text().trim().is_empty() {
                    this.search_drive("", cx);
                }
            }
            InputEvent::Changed | InputEvent::Submit if this.place == Place::Mail => {
                this.filter_mail(cx)
            }
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
            scanned: false,
            failed: 0,
            selected: None,
            chosen: Vec::new(),
            tags_field,
            add_tag,
            pattern,
            renaming: false,
            busy: false,
            notice: None,
            checks: Vec::new(),
            grid: UniformListScrollHandle::new(),
            width: Rc::new(Cell::new(800.0)),
            drives: Vec::new(),
            drive: None,
            mail_pdfs: Vec::new(),
            mail: None,
            watcher: None,
            _inputs: [searching, tagging, adding, naming],
        };
        this.reload(cx);
        this.watch(cx);
        this.load_drives(cx);
        this.load_mail(cx);
        this
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// The library is shown. The first time, its folders are read;
    /// later, what is kept shows at once, covers and all, and the
    /// folders' watcher reads what changes.
    pub fn shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.search.focus_handle(cx), cx);
        self.reload(cx);
        if !std::mem::replace(&mut self.scanned, true) {
            self.scan(cx);
        }
        self.load_drives(cx);
        self.load_mail(cx);
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
                            // Covers of changed files are read again; when
                            // nothing changed, those kept stay.
                            let changed = result
                                .as_ref()
                                .map_or(true, |r| r.added + r.updated + r.moved > 0);
                            if changed {
                                this.no_cover.clear();
                                let old: Vec<_> = this.covers.drain().map(|(_, c)| c).collect();
                                this.released.extend(old);
                            }
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
            Place::Drive(_) | Place::Mail => {}
        }
        let in_drive = self.place.elsewhere();
        self.docs = if in_drive {
            Vec::new()
        } else if self.place == Place::Copies {
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
        self.hits = if filter.words.is_empty() || in_drive {
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
        let docs = &self.docs;
        self.chosen.retain(|id| docs.iter().any(|d| d.id == *id));
        cx.notify();
    }

    fn go(&mut self, place: Place, cx: &mut Context<Self>) {
        let was_elsewhere = self.place.elsewhere();
        self.place = place;
        self.selected = None;
        self.chosen.clear();
        self.renaming = false;
        self.drive = None;
        self.mail = None;
        if let Place::Drive(account) = self.place {
            self.search.update(cx, |s, cx| {
                s.set_text("", cx);
                s.set_placeholder(t("library-drive-search", &[]));
            });
            self.open_drive(account, cx);
        } else if self.place == Place::Mail {
            self.search.update(cx, |s, cx| {
                s.set_text("", cx);
                s.set_placeholder(t("library-mail-search", &[]));
            });
            self.filter_mail(cx);
        } else if was_elsewhere {
            self.search.update(cx, |s, cx| {
                s.set_text("", cx);
                s.set_placeholder(t("library-search", &[]));
            });
        }
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
        self.chosen = vec![id];
        self.renaming = false;
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
        let Some(id) = self.single() else {
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
        self.chosen.clear();
        self.reload(cx);
    }

    /// The one document chosen, when only one is.
    fn single(&self) -> Option<i64> {
        match self.chosen[..] {
            [id] => Some(id),
            _ => None,
        }
    }

    /// The chosen documents still where they were, in the grid's order.
    fn chosen_docs(&self) -> Vec<Doc> {
        self.docs
            .iter()
            .filter(|d| self.chosen.contains(&d.id) && !d.missing)
            .cloned()
            .collect()
    }

    /// Ctrl-click: `id` chosen too, or no longer.
    fn toggle_chosen(&mut self, id: i64, cx: &mut Context<Self>) {
        match self.chosen.iter().position(|c| *c == id) {
            Some(at) => {
                self.chosen.remove(at);
            }
            None => self.chosen.push(id),
        }
        self.selected = Some(id);
        self.renaming = false;
        if let Some(one) = self.single() {
            self.select(one, cx);
        }
        cx.notify();
    }

    /// Shift-click: every document from the one last clicked to `id`.
    fn choose_to(&mut self, id: i64, cx: &mut Context<Self>) {
        let at = |id: Option<i64>| self.docs.iter().position(|d| Some(d.id) == id);
        let (Some(from), Some(to)) = (at(self.selected).or(Some(0)), at(Some(id))) else {
            return;
        };
        let (a, b) = (from.min(to), from.max(to));
        self.chosen = self.docs[a..=b].iter().map(|d| d.id).collect();
        self.renaming = false;
        if let Some(one) = self.single() {
            self.select(one, cx);
        }
        cx.notify();
    }

    fn choose_all(&mut self, cx: &mut Context<Self>) {
        self.chosen = self.docs.iter().map(|d| d.id).collect();
        self.renaming = false;
        cx.notify();
    }

    fn clear_chosen(&mut self, cx: &mut Context<Self>) {
        self.chosen.clear();
        self.selected = None;
        self.renaming = false;
        self.checks.clear();
        cx.notify();
    }

    /// The chosen documents' pages as one new file, saved where the
    /// person says and opened.
    fn merge_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let docs = self.chosen_docs();
        if docs.len() < 2 || self.busy {
            return;
        }
        let folder = docs[0].path.parent().map(PathBuf::from).unwrap_or_default();
        let name = t("library-merged-name", &[]);
        let target = cx.prompt_for_new_path(&folder, Some(&name));
        let paths: Vec<PathBuf> = docs.into_iter().map(|d| d.path).collect();
        self.busy = true;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let target = match target.await {
                Ok(Ok(Some(path))) if path.extension().is_some() => path,
                Ok(Ok(Some(path))) => path.with_extension("pdf"),
                _ => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let out = target.clone();
            let merged = cx
                .background_executor()
                .spawn(async move { katna_pdf_library::merge(&paths, &out) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match merged {
                    Ok(pages) => {
                        this.notice = Some(t(
                            "library-merged",
                            &[
                                ("name", &folder_name(&target)),
                                ("pages", &pages.to_string()),
                            ],
                        ));
                        cx.emit(LibraryEvent::Open(target, None));
                    }
                    Err(e) => {
                        this.notice = Some(t("library-merge-failed", &[("error", &e)]));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Two files chosen: the one changed last compared with the other.
    fn compare_chosen(&mut self, cx: &mut Context<Self>) {
        let mut docs = self.chosen_docs();
        if docs.len() != 2 {
            return;
        }
        docs.sort_by_key(|d| d.modified);
        let newer = docs.pop().map(|d| d.path);
        let older = docs.pop().map(|d| d.path);
        if let (Some(older), Some(newer)) = (older, newer) {
            cx.emit(LibraryEvent::Compare { older, newer });
        }
    }

    fn sign_chosen(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .chosen_docs()
            .into_iter()
            .filter(|d| !d.locked)
            .map(|d| d.path)
            .collect();
        if !paths.is_empty() {
            cx.emit(LibraryEvent::Sign(paths));
        }
    }

    /// Checks the chosen files' signatures, offline, in the background.
    fn check_chosen(&mut self, cx: &mut Context<Self>) {
        let docs = self.chosen_docs();
        if docs.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        self.checks.clear();
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checks = cx
                .background_executor()
                .spawn(async move {
                    docs.into_iter()
                        .map(|d| (d.id, d.name(), verdict(&d.path)))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.checks = checks;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_renaming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = true;
        self.notice = None;
        let focus = self.pattern.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Renames the chosen documents by the pattern typed.
    fn rename_chosen(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.chosen_docs().iter().map(|d| d.id).collect();
        let pattern = self.pattern.read(cx).text().trim().to_owned();
        let Ok(lib) = &self.lib else {
            return;
        };
        if ids.is_empty() || pattern.is_empty() {
            return;
        }
        self.notice = Some(match lib.rename(&ids, &pattern) {
            Ok(report) => report_line("library-renamed", &report),
            Err(e) => e.to_string(),
        });
        self.renaming = false;
        self.reload(cx);
    }

    /// Copies the chosen files into a folder the person picks.
    fn copy_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self.chosen_docs().iter().map(|d| d.id).collect();
        if ids.is_empty() {
            return;
        }
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("library-copy-to", &[]).into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(folder))) = chosen.await else {
                return;
            };
            let Some(folder) = folder.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Ok(lib) = &this.lib {
                    this.notice = Some(match lib.copy_to(&ids, &folder) {
                        Ok(report) => report_line("library-copied", &report),
                        Err(e) => e.to_string(),
                    });
                }
                this.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Makes the chosen files smaller in their places, their pages kept
    /// as they are.
    fn compress_chosen(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let files: Vec<(i64, PathBuf)> = self
            .chosen_docs()
            .iter()
            .map(|d| (d.id, d.path.clone()))
            .collect();
        if files.is_empty() {
            return;
        }
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (report, saved) = cx
                .background_executor()
                .spawn(async move { katna_pdf_library::compress(&files) })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                let mut line = report_line("library-compressed", &report);
                if saved > 0 {
                    line = format!(
                        "{line} {}",
                        t("library-compress-saved", &[("size", &size_text(saved))])
                    );
                }
                this.notice = Some(line);
                this.reload(cx);
                this.scan(cx);
            })
            .ok();
        })
        .detach();
    }

    fn tag_chosen(&mut self, cx: &mut Context<Self>) {
        let tag = self.add_tag.read(cx).text().trim().to_owned();
        let ids: Vec<i64> = self.chosen.clone();
        if let Ok(lib) = &self.lib
            && !tag.is_empty()
        {
            let _ = lib.add_tag(&ids, &tag);
        }
        self.add_tag.update(cx, |f, cx| f.set_text("", cx));
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
            Place::Mail => t("library-mail-title", &[]),
            Place::Drive(_) => self.drive.as_ref().map_or_else(String::new, |v| {
                t(
                    "library-drive-title",
                    &[
                        ("drive", &drive::provider_name(&v.drive.provider)),
                        ("address", &v.drive.address),
                    ],
                )
            }),
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
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.key == "escape" {
                    if this.renaming {
                        this.renaming = false;
                        cx.notify();
                    } else if this.chosen.len() > 1 || !this.checks.is_empty() {
                        this.clear_chosen(cx);
                    } else {
                        cx.emit(LibraryEvent::Close);
                    }
                } else if key.key == "a"
                    && key.modifiers.secondary()
                    && this.focus.is_focused(window)
                {
                    this.choose_all(cx);
                }
            }))
            .flex_1()
            .min_h_0()
            .flex()
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
            .bg(th.card)
            .border_r_1()
            .border_color(th.line)
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
        if !self.drives.is_empty() {
            side = side.child(self.heading("library-drives", None));
            for (i, d) in self.drives.clone().into_iter().enumerate() {
                side = side.child(self.place_row(
                    ("drive", i),
                    Place::Drive(d.account),
                    "cloud",
                    d.address,
                    None,
                    None,
                    cx,
                ));
            }
        }
        if !self.mail_pdfs.is_empty() {
            side = side
                .child(self.heading("library-mail", None))
                .child(self.place_row(
                    ("mail", 0),
                    Place::Mail,
                    "attachment",
                    t("library-mail-files", &[]),
                    Some(self.mail_pdfs.len()),
                    None,
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
        let in_drive = self.drive.is_some();
        let in_mail = self.place == Place::Mail;
        let elsewhere = in_drive || in_mail;
        let has_query = !self.search.read(cx).text().trim().is_empty() && !elsewhere;
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
                    .tip(
                        t(
                            if in_drive {
                                "library-drive-search-tip"
                            } else if in_mail {
                                "library-mail-search-tip"
                            } else {
                                "library-search-tip"
                            },
                            &[],
                        ),
                        &th.katna,
                    )
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
            .when(!elsewhere, |d| {
                d.child(
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
                )
            });
        let body: AnyElement = if in_drive {
            self.drive_body(cx)
        } else if in_mail {
            self.mail_body(cx)
        } else if let Err(e) = &self.lib {
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
        let checks = (!self.checks.is_empty()).then(|| self.checks(cx).into_any_element());
        let details = if in_drive {
            self.drive_details(cx)
        } else if in_mail {
            self.mail_details(cx)
        } else if self.chosen.len() > 1 {
            Some(self.bulk(cx).into_any_element())
        } else {
            self.details(cx).map(IntoElement::into_any_element)
        };
        let notice = self.notice.clone().map(|notice| {
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S4))
                .py(px(space::S1))
                .text_size(px(text::SMALL))
                .text_color(th.dim)
                .child(div().flex_1().min_w_0().child(notice))
                .child(
                    widgets::icon_button("library-notice-close", "close", 14.0, &th.katna)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        })),
                )
        });
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
            .bg(th.card)
            .overflow_hidden()
            .child(header)
            .children(hits)
            .children(checks)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .children(failed)
            .children(notice)
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
        self.grid_frame(
            "library-grid",
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let near = range.start * columns..(range.end * columns).min(this.docs.len());
                range
                    .map(|row| this.grid_row(row, columns, &near, cx))
                    .collect::<Vec<_>>()
            }),
        )
    }

    /// A grid of `rows` rows, drawn by `rows_in` as they come into view,
    /// that keeps its width for [`LibraryView::columns`].
    fn grid_frame<R: IntoElement>(
        &self,
        id: &'static str,
        rows: usize,
        rows_in: impl 'static + Fn(Range<usize>, &mut Window, &mut gpui::App) -> Vec<R>,
    ) -> impl IntoElement {
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
                uniform_list(id, rows, rows_in)
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
        let selected = self.chosen.contains(&doc.id);
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
            .on_click(
                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    // Away from the search, so Ctrl+A chooses every document.
                    window.focus(&this.focus, cx);
                    let keys = event.modifiers();
                    if keys.shift {
                        this.choose_to(id, cx);
                    } else if keys.secondary() {
                        this.toggle_chosen(id, cx);
                    } else if event.click_count() >= 2 {
                        this.open(id, None, cx);
                    } else {
                        this.select(id, cx);
                    }
                }),
            )
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

    /// The strip for many chosen documents: what can be done to them all.
    fn bulk(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let count = self.chosen.len();
        let strip = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S4))
            .py(px(space::S2))
            .border_t_1()
            .border_color(th.line);
        let field = |input: &Entity<TextInput>, icon: &'static str, width: f32| {
            div()
                .w(px(width))
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
                        .path(icon)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(th.dim),
                )
                .child(div().flex_1().min_w_0().child(input.clone()))
        };
        if self.renaming {
            let pattern = self.pattern.read(cx).text().trim().to_owned();
            let docs = self.chosen_docs();
            let example = docs
                .first()
                .map(|d| format!("{}.pdf", katna_pdf_library::name_from(d, &pattern, 1)))
                .unwrap_or_default();
            let keys = katna_pdf_library::PATTERN_KEYS
                .iter()
                .map(|k| format!("{{{k}}}"))
                .collect::<Vec<_>>()
                .join(" ");
            return strip
                .child(field(&self.pattern, "icons/rename.svg", 280.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::SMALL))
                                .child(t("library-rename-example", &[("name", &example)])),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(text::SMALL))
                                .text_color(th.dim)
                                .child(t("library-rename-keys", &[("keys", &keys)])),
                        ),
                )
                .child(
                    widgets::outlined_button(
                        "library-rename-cancel",
                        t("library-cancel", &[]),
                        &th.katna,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.renaming = false;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::filled_button(
                        "library-rename-go",
                        t("library-rename-go", &[("count", &docs.len().to_string())]),
                        &th.katna,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.rename_chosen(cx))),
                )
                .into_any_element();
        }
        let action = |id: &'static str, icon: &'static str| {
            widgets::icon_button(id, icon, 18.0, &th.katna)
                .size(px(32.0))
                .tip(t(id, &[]), &th.katna)
        };
        strip
            .child(
                widgets::icon_button("library-unchoose", "close", 16.0, &th.katna)
                    .tip(t("library-unchoose", &[]), &th.katna)
                    .on_click(cx.listener(|this, _, _, cx| this.clear_chosen(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if self.busy {
                        t("library-working", &[])
                    } else {
                        t("library-chosen", &[("count", &count.to_string())])
                    }),
            )
            .child(field(&self.add_tag, "icons/tag.svg", 180.0))
            .child(
                action("library-merge", "copies")
                    .on_click(cx.listener(|this, _, window, cx| this.merge_chosen(window, cx))),
            )
            .when(count == 2, |strip| {
                strip.child(
                    action("library-compare", "compare")
                        .on_click(cx.listener(|this, _, _, cx| this.compare_chosen(cx))),
                )
            })
            .child(
                action("library-sign", "signature")
                    .on_click(cx.listener(|this, _, _, cx| this.sign_chosen(cx))),
            )
            .child(
                action("library-check", "shield-check")
                    .on_click(cx.listener(|this, _, _, cx| this.check_chosen(cx))),
            )
            .child(
                action("library-rename", "rename")
                    .on_click(cx.listener(|this, _, window, cx| this.start_renaming(window, cx))),
            )
            .child(
                action("library-compress", "compress")
                    .on_click(cx.listener(|this, _, _, cx| this.compress_chosen(cx))),
            )
            .child(
                action("library-copy", "download")
                    .on_click(cx.listener(|this, _, window, cx| this.copy_chosen(window, cx))),
            )
            .into_any_element()
    }

    /// The chosen files' signatures, as checked.
    fn checks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let rows = self
            .checks
            .iter()
            .enumerate()
            .map(|(i, (id, name, verdict))| {
                let id = *id;
                let (icon, colour, line): (&str, gpui::Hsla, String) = match verdict {
                    Verdict::Unsigned => ("document", th.dim, t("library-check-unsigned", &[])),
                    Verdict::Good(n) => (
                        "shield-check",
                        gpui::rgba(th.valid).into(),
                        if *n == 1 {
                            t("library-check-good-one", &[])
                        } else {
                            t("library-check-good", &[("count", &n.to_string())])
                        },
                    ),
                    Verdict::Unverified(why) => (
                        "shield-check",
                        gpui::rgba(th.katna.warning).into(),
                        t("library-check-unverified", &[("why", why)]),
                    ),
                    Verdict::Bad(why) => ("shield-alert", th.error, sentence(why)),
                    Verdict::Unreadable(why) => (
                        "shield-alert",
                        th.error,
                        t("library-check-unreadable", &[("error", why)]),
                    ),
                };
                div()
                    .id(("check", i))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S3))
                    .py(px(space::S1))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .hover(move |s| s.bg(th.hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.open(id, None, cx)))
                    .child(
                        svg()
                            .path(format!("icons/{icon}.svg"))
                            .size(px(16.0))
                            .flex_none()
                            .text_color(colour),
                    )
                    .child(
                        div()
                            .w(px(220.0))
                            .flex_none()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(line),
                    )
            });
        div()
            .id("library-checks")
            .flex_none()
            .max_h(px(240.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .px(px(space::S3))
            .py(px(space::S2))
            .border_b_1()
            .border_color(th.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .px(px(space::S3))
                    .pb(px(space::S1))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(text::SMALL))
                            .text_color(th.dim)
                            .child(t("library-checked", &[])),
                    )
                    .child(
                        widgets::icon_button("library-checks-close", "close", 14.0, &th.katna)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.checks.clear();
                                cx.notify();
                            })),
                    ),
            )
            .children(rows)
    }

    /// The selected document's strip: where it is, its stars and tags.
    fn details(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let id = self.single()?;
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

/// `text` starting with a capital, as a sentence on its own.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What a bulk change did: how many were done, and the first that
/// could not be.
fn report_line(id: &str, report: &katna_pdf_library::BulkReport) -> String {
    let mut line = t(id, &[("count", &report.done.len().to_string())]);
    if let Some((path, why)) = report.failed.first() {
        line.push(' ');
        line.push_str(&t(
            "library-bulk-failed",
            &[
                ("count", &report.failed.len().to_string()),
                ("name", &folder_name(path)),
                ("error", why),
            ],
        ));
    }
    line
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

pub(crate) fn size_text(bytes: u64) -> String {
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
