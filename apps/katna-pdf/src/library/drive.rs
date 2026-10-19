// SPDX-License-Identifier: GPL-3.0-or-later

//! The library's drives: the Google Drive or OneDrive of an account of
//! Katna Mail's, browsed folder by folder or searched, its PDFs (and the
//! drive's own documents, as PDFs) downloaded through Katna and opened.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::{AnyElement, Context, FontWeight, Image, ImageFormat, div, img, prelude::*, svg};
use katna_dbus::CloudEntry;
use katna_ui::px;
use katna_ui::tokens::{radius, space, text};
use katna_ui::widgets;

use super::{CELL_HEIGHT, CELL_WIDTH, COVER_BOX, LibraryEvent, LibraryView, Place, size_text};
use crate::drives::{self, Drive, cloud_place, cloud_state};
use crate::text::t;

/// Thumbnails asked of the drive at once, at most.
const THUMBS_AT_ONCE: usize = 4;
/// How wide a thumbnail is asked for, in pixels.
const THUMB_WIDTH: u32 = 280;

/// Where in a drive the library is.
pub(super) struct DriveView {
    pub drive: Drive,
    /// The folders opened, from the top: id and name.
    trail: Vec<(String, String)>,
    /// What was shared with the account, instead of its own files.
    shared: bool,
    /// Words looked for in the whole drive, instead of a folder.
    search: Option<String>,
    items: Vec<CloudEntry>,
    /// The next page of the listing; empty on the last.
    next: String,
    state: Listing,
    /// A page is being read.
    loading: bool,
    /// Thumbnails by entry id: `None` while asked for, or when there is
    /// none.
    thumbs: HashMap<String, Option<Arc<Image>>>,
    asking: usize,
    chosen: Option<String>,
    /// The entry being downloaded to open.
    opening: Option<String>,
    /// Bumped with each new listing, so a late page of an old one is
    /// dropped.
    generation: u64,
}

enum Listing {
    Loading,
    Ready,
    Failed(String),
}

impl DriveView {
    fn new(drive: Drive) -> Self {
        Self {
            drive,
            trail: Vec::new(),
            shared: false,
            search: None,
            items: Vec::new(),
            next: String::new(),
            state: Listing::Loading,
            loading: false,
            thumbs: HashMap::new(),
            asking: 0,
            chosen: None,
            opening: None,
            generation: 0,
        }
    }

    /// The `place` and `what` of the listing shown.
    fn source(&self) -> (&'static str, String) {
        if let Some(words) = &self.search {
            (cloud_place::SEARCH, words.clone())
        } else if self.shared && self.trail.is_empty() {
            (cloud_place::SHARED, String::new())
        } else {
            let folder = self.trail.last().map_or("root", |(id, _)| id);
            (cloud_place::FOLDER, folder.to_owned())
        }
    }
}

/// The name of a drive's kind, as people know it.
pub(super) fn provider_name(provider: &str) -> String {
    match provider {
        "microsoft" => t("library-drive-microsoft", &[]),
        _ => t("library-drive-google", &[]),
    }
}

/// The format of a thumbnail the drive sent, from its first bytes.
fn format_of(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xFF, 0xD8, ..] => Some(ImageFormat::Jpeg),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        [b'G', b'I', b'F', ..] => Some(ImageFormat::Gif),
        _ => None,
    }
}

impl LibraryView {
    /// Asks Katna which drives there are, in the background.
    pub(super) fn load_drives(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async { drives::drives() })
                .await;
            this.update(cx, |this, cx| {
                if let Place::Drive(account) = this.place
                    && !found.iter().any(|d| d.account == account)
                {
                    this.drive = None;
                    this.go(Place::All, cx);
                }
                this.drives = found;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows the top of drive `account`.
    pub(super) fn open_drive(&mut self, account: i64, cx: &mut Context<Self>) {
        let Some(drive) = self.drives.iter().find(|d| d.account == account) else {
            return;
        };
        self.drive = Some(DriveView::new(drive.clone()));
        self.list_drive(false, cx);
    }

    /// Reads the listing shown again from its start, or (`more`) its
    /// next page.
    fn list_drive(&mut self, more: bool, cx: &mut Context<Self>) {
        let Some(view) = self.drive.as_mut() else {
            return;
        };
        if more && (view.next.is_empty() || view.loading) {
            return;
        }
        if !more {
            view.generation += 1;
            view.items.clear();
            view.next.clear();
            view.chosen = None;
            view.state = Listing::Loading;
        }
        view.loading = true;
        let generation = view.generation;
        let account = view.drive.account;
        let (place, what) = view.source();
        let page = view.next.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let listing = cx
                .background_executor()
                .spawn(async move { drives::list(account, place, &what, &page) })
                .await;
            this.update(cx, |this, cx| {
                let Some(view) = this.drive.as_mut() else {
                    return;
                };
                if view.generation != generation {
                    return;
                }
                view.loading = false;
                view.state = match listing {
                    Ok(listing) if listing.state == cloud_state::OK => {
                        let mut items: Vec<CloudEntry> =
                            listing.items.into_iter().filter(drives::shown).collect();
                        // Folders first, then files, each by name.
                        items.sort_by_key(|e| (!e.folder, e.name.to_lowercase()));
                        view.items.extend(items);
                        view.next = listing.next;
                        Listing::Ready
                    }
                    Ok(listing) => Listing::Failed(match listing.state.as_str() {
                        cloud_state::NEEDS_PERMISSION => t("library-drive-permission", &[]),
                        cloud_state::UNSUPPORTED => t("library-drive-unsupported", &[]),
                        _ => t("library-drive-failed", &[("error", &listing.error)]),
                    }),
                    Err(e) => Listing::Failed(t("library-drive-failed", &[("error", &e)])),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Looks for `words` in the whole drive; no words go back to the
    /// folder.
    pub(super) fn search_drive(&mut self, words: &str, cx: &mut Context<Self>) {
        let Some(view) = self.drive.as_mut() else {
            return;
        };
        let words = words.trim();
        let search = (!words.is_empty()).then(|| words.to_owned());
        if search == view.search {
            return;
        }
        view.search = search;
        self.list_drive(false, cx);
    }

    fn enter_folder(&mut self, entry: &CloudEntry, cx: &mut Context<Self>) {
        let Some(view) = self.drive.as_mut() else {
            return;
        };
        view.search = None;
        view.trail.push((entry.id.clone(), entry.name.clone()));
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.list_drive(false, cx);
    }

    /// Back to the `depth`th folder of the trail (0: the top), of the
    /// account's own files or (`shared`) what was shared with it.
    fn back_to(&mut self, depth: usize, shared: bool, cx: &mut Context<Self>) {
        let Some(view) = self.drive.as_mut() else {
            return;
        };
        view.search = None;
        view.shared = shared;
        view.trail.truncate(depth);
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.list_drive(false, cx);
    }

    /// Downloads `entry` through Katna, then opens it.
    fn open_entry(&mut self, entry: CloudEntry, cx: &mut Context<Self>) {
        let Some(view) = self.drive.as_mut() else {
            return;
        };
        if view.opening.is_some() {
            return;
        }
        view.opening = Some(entry.id.clone());
        let account = view.drive.account;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let fetched = cx
                .background_executor()
                .spawn(async move { drives::fetch(account, &entry) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.drive.as_mut() {
                    view.opening = None;
                }
                match fetched {
                    Ok(path) => cx.emit(LibraryEvent::Open(path, None)),
                    Err(e) => this.notice = Some(t("library-drive-open-failed", &[("error", &e)])),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// `entry`'s thumbnail, asked of the drive on first need.
    fn thumb(&mut self, entry: &CloudEntry, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let view = self.drive.as_mut()?;
        if let Some(thumb) = view.thumbs.get(&entry.id) {
            return thumb.clone();
        }
        if entry.thumbnail.is_empty() || view.asking >= THUMBS_AT_ONCE {
            return None;
        }
        view.thumbs.insert(entry.id.clone(), None);
        view.asking += 1;
        let (account, link, id) = (
            view.drive.account,
            entry.thumbnail.clone(),
            entry.id.clone(),
        );
        cx.spawn(async move |this, cx| {
            let image = cx
                .background_executor()
                .spawn(async move {
                    let bytes = drives::thumbnail(account, &link, THUMB_WIDTH).ok()?;
                    let format = format_of(&bytes)?;
                    Some(Arc::new(Image::from_bytes(format, bytes)))
                })
                .await;
            this.update(cx, |this, cx| {
                if let Some(view) = this.drive.as_mut() {
                    view.asking -= 1;
                    view.thumbs.insert(id, image);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
    }

    /// The drive's part of the library: where in it, and its folders and
    /// PDFs.
    pub(super) fn drive_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let Some(view) = self.drive.as_ref() else {
            return self.message(t("library-nothing", &[]), None);
        };
        let crumb = |id: (&'static str, usize), name: String, current: bool| {
            div()
                .id(id)
                .px(px(space::S2))
                .py(px(space::S1))
                .rounded(px(radius::SM))
                .text_size(px(text::SMALL))
                .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                .when(!current, |d| {
                    d.text_color(th.dim)
                        .cursor_pointer()
                        .hover(move |s| s.bg(th.hover))
                })
                .child(name)
        };
        let searching = view.search.is_some();
        let top_current = view.trail.is_empty() && !searching;
        // At the top both roots show, to switch between; deeper in, only
        // the one the folders are under, so the trail reads as a path.
        let mut trail = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .px(px(space::S4))
            .py(px(space::S1))
            .border_b_1()
            .border_color(th.line)
            .when(top_current || !view.shared, |d| {
                d.child(
                    crumb(
                        ("drive-top", 0),
                        t("library-drive-mine", &[]),
                        top_current && !view.shared,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.back_to(0, false, cx))),
                )
            })
            .when(top_current || view.shared, |d| {
                d.child(
                    crumb(
                        ("drive-shared", 0),
                        t("library-drive-shared", &[]),
                        top_current && view.shared,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.back_to(0, true, cx))),
                )
            });
        for (i, (_, name)) in view.trail.iter().enumerate() {
            let current = i + 1 == view.trail.len() && !searching;
            trail = trail
                .child(
                    svg()
                        .path("icons/chevron-right.svg")
                        .size(px(14.0))
                        .text_color(th.dim),
                )
                .child(
                    crumb(("drive-crumb", i), name.clone(), current).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let shared = this.drive.as_ref().is_some_and(|v| v.shared);
                            this.back_to(i + 1, shared, cx)
                        },
                    )),
                );
        }
        if let Some(words) = &view.search {
            trail = trail
                .child(
                    svg()
                        .path("icons/chevron-right.svg")
                        .size(px(14.0))
                        .text_color(th.dim),
                )
                .child(crumb(
                    ("drive-search", 0),
                    t("library-drive-found", &[("words", words)]),
                    true,
                ));
        }
        let body = match &view.state {
            Listing::Loading => self.message(t("library-drive-loading", &[]), None),
            Listing::Failed(why) => self.message(why.clone(), None),
            Listing::Ready if view.items.is_empty() => {
                self.message(t("library-drive-empty", &[]), None)
            }
            Listing::Ready => self.drive_grid(cx).into_any_element(),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(trail)
            .child(body)
            .into_any_element()
    }

    fn drive_grid(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns();
        let count = self.drive.as_ref().map_or(0, |v| v.items.len());
        let rows = count.div_ceil(columns);
        self.grid_frame(
            "drive-grid",
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                // The last row in view: the next page, if there is one.
                if range.end >= rows {
                    this.list_drive(true, cx);
                }
                range
                    .map(|row| {
                        let start = row * columns;
                        let end = (start + columns).min(count);
                        let cells: Vec<AnyElement> = (start..end)
                            .map(|i| this.drive_cell(i, cx).into_any_element())
                            .collect();
                        div()
                            .h(px(CELL_HEIGHT))
                            .flex()
                            .children(cells)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
    }

    fn drive_cell(&mut self, i: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let th = self.theme;
        let Some(entry) = self.drive.as_ref().and_then(|v| v.items.get(i)).cloned() else {
            return div().into_any_element();
        };
        let thumb = if entry.folder {
            None
        } else {
            self.thumb(&entry, cx)
        };
        let view = self.drive.as_ref();
        let chosen = view.is_some_and(|v| v.chosen.as_ref() == Some(&entry.id));
        let opening = view.is_some_and(|v| v.opening.as_ref() == Some(&entry.id));
        let line = if opening {
            t("library-drive-downloading", &[])
        } else if entry.folder {
            t("library-drive-folder", &[])
        } else if entry.native {
            t("library-drive-native", &[])
        } else {
            size_text(entry.size)
        };
        let icon = if entry.folder {
            "icons/folder.svg"
        } else {
            "icons/document.svg"
        };
        let clicked = entry.clone();
        div()
            .id(("drive-entry", i))
            .w(px(CELL_WIDTH))
            .h(px(CELL_HEIGHT))
            .flex_none()
            .p(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .rounded(px(radius::MD))
            .cursor_pointer()
            .when(chosen, |d| d.bg(gpui::rgba(th.katna.row_selected)))
            .when(!chosen, |d| d.hover(move |s| s.bg(th.hover)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if clicked.folder {
                    this.enter_folder(&clicked, cx);
                } else if event.click_count() >= 2 {
                    this.open_entry(clicked.clone(), cx);
                } else if let Some(view) = this.drive.as_mut() {
                    view.chosen = Some(clicked.id.clone());
                    cx.notify();
                }
            }))
            .child(
                div()
                    .h(px(COVER_BOX))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .bg(th.backdrop)
                    .overflow_hidden()
                    .child(match thumb {
                        Some(image) => div()
                            .size_full()
                            .p(px(space::S2))
                            .child(img(image).size_full().object_fit(gpui::ObjectFit::Contain))
                            .into_any_element(),
                        None => svg()
                            .path(icon)
                            .size(px(48.0))
                            .text_color(th.dim)
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(entry.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(line),
            )
            .into_any_element()
    }

    /// The selected drive file's strip: its name, and Open.
    pub(super) fn drive_details(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let th = self.theme;
        let view = self.drive.as_ref()?;
        let id = view.chosen.as_ref()?;
        let entry = view.items.iter().find(|e| &e.id == id)?.clone();
        let opening = view.opening.is_some();
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
                        .truncate()
                        .text_size(px(text::BODY))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(entry.name.clone()),
                )
                .child(
                    widgets::filled_button(
                        "library-drive-open",
                        if opening {
                            t("library-drive-downloading", &[])
                        } else {
                            t("library-open", &[])
                        },
                        &th.katna,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_entry(entry.clone(), cx)),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_are_known_by_their_first_bytes() {
        assert_eq!(format_of(b"\x89PNG\r\n"), Some(ImageFormat::Png));
        assert_eq!(format_of(b"\xFF\xD8\xFF\xE0"), Some(ImageFormat::Jpeg));
        assert_eq!(format_of(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(format_of(b"<html>"), None);
    }
}
