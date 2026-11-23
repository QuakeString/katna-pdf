// SPDX-License-Identifier: GPL-3.0-or-later

//! Right-clicking the pages, as every reader does: with text selected,
//! Copy, marking it, finding it, searching the web for it; on a link,
//! Open link and Copy link address; on a picture, Copy picture (as the
//! file holds it: a JPEG as it is, others as PNG) and Save picture as;
//! and always selecting the page's text, the hand, zoom, turning the
//! view, copying the page as a picture, printing and the properties. A
//! picture this cannot read out (JPEG 2000, JBIG2, fax) is drawn from
//! the page instead.

use std::path::Path;
use std::sync::Arc;

use gpui::{
    AnyElement, ClipboardItem, Context, Image, ImageFormat, Pixels, Point, Window, prelude::*,
};
use katna_pdf_cos::{LinkTarget, ObjectKind};
use katna_pdf_render::Region;

use super::{Content, Viewer, Zoom};
use crate::float;
use crate::text::t;

/// The menu right-clicking the pages opens.
pub(super) struct PictureMenu {
    /// Where the pointer was, in the window.
    at: Point<Pixels>,
    /// The page clicked, if the click was on one.
    page: Option<usize>,
    /// The link under the pointer.
    link: Option<LinkTarget>,
    /// The picture under the pointer, once looked for.
    picture: Option<MenuPicture>,
}

/// A picture under the pointer.
struct MenuPicture {
    page: usize,
    /// Which object on the page it is, from 1, for the file's name.
    number: usize,
    /// The picture's bytes and their kind (`jpg` or `png`), or `None`
    /// when they are to be drawn from the page's `rect` (points from its
    /// top left).
    bytes: Option<Arc<(Vec<u8>, &'static str)>>,
    rect: [f32; 4],
}

/// A picture's bytes and their kind (`jpg` or `png`).
type Picture = (Vec<u8>, &'static str);

/// Pages this picture is drawn at when it is drawn from the page.
const DRAWN_SCALE: f32 = 3.0;

/// The longest selection the menu quotes in Find.
const QUOTED: usize = 24;

impl Viewer {
    /// A right click on the pages opens their menu at once; a picture
    /// under the pointer, read from the file as it is now, adds its items
    /// when found.
    pub(super) fn picture_right_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        let at = event.position;
        let spot = self.page_point(at);
        let link = spot.and_then(|(page, point)| self.link_at(page, point));
        self.picture_menu = Some(PictureMenu {
            at,
            page: spot.map(|(page, _)| page),
            link,
            picture: None,
        });
        cx.notify();
        let Some((page, point)) = spot else {
            return;
        };
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let Some(transform) = opened.doc.page_transform(page) else {
            return;
        };
        self.read_cos(
            move |doc| {
                let objects = doc.page_objects(page);
                let (index, rect) = objects
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| o.kind == ObjectKind::Picture)
                    .map(|(i, o)| (i, katna_pdf_render::map_rect(transform, o.rect)))
                    .filter(|(_, b)| {
                        point[0] >= b[0] && point[0] <= b[2] && point[1] >= b[1] && point[1] <= b[3]
                    })
                    // The smallest, the last drawn of those as big.
                    .min_by(|a, b| {
                        let area = |r: &[f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
                        area(&a.1)
                            .partial_cmp(&area(&b.1))
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then(b.0.cmp(&a.0))
                    })?;
                let picture = doc
                    .picture_file(page, index)
                    .and_then(|p| super::objects::picture_bytes(p).ok());
                Some((index, rect, picture))
            },
            move |this, found, cx| {
                let Some((index, rect, picture)) = found else {
                    return;
                };
                // Only into the menu it was looked for.
                if let Some(menu) = this.picture_menu.as_mut().filter(|m| m.at == at) {
                    menu.picture = Some(MenuPicture {
                        page,
                        number: index + 1,
                        bytes: picture.map(Arc::new),
                        rect,
                    });
                    cx.notify();
                }
            },
            cx,
        );
    }

    /// The link at `point` (points from page `page`'s top left).
    fn link_at(&self, page: usize, point: [f32; 2]) -> Option<LinkTarget> {
        let Content::Ready(opened) = &self.content else {
            return None;
        };
        let transform = opened.doc.page_transform(page)?;
        self.links
            .as_ref()?
            .get(page)?
            .iter()
            .find(|link| {
                let b = katna_pdf_render::map_rect(transform, link.rect);
                point[0] >= b[0] && point[0] <= b[2] && point[1] >= b[1] && point[1] <= b[3]
            })
            .map(|link| link.target.clone())
    }

    /// The picture's bytes and kind, drawing it from the page when the
    /// file's own cannot be read out.
    fn menu_picture(&self, cx: &mut Context<Self>) -> Option<gpui::Task<Option<Picture>>> {
        let menu = self.picture_menu.as_ref()?.picture.as_ref()?;
        if let Some(picture) = &menu.bytes {
            let picture = picture.clone();
            return Some(
                cx.background_executor()
                    .spawn(async move { Some((picture.0.clone(), picture.1)) }),
            );
        }
        let Content::Ready(opened) = &self.content else {
            return None;
        };
        let doc = opened.doc.clone();
        let (page, r) = (menu.page, menu.rect);
        let region = Region {
            x: (r[0].max(0.0) * DRAWN_SCALE).round() as u32,
            y: (r[1].max(0.0) * DRAWN_SCALE).round() as u32,
            width: ((r[2] - r[0]) * DRAWN_SCALE).round().max(1.0) as u32,
            height: ((r[3] - r[1]) * DRAWN_SCALE).round().max(1.0) as u32,
        };
        Some(cx.background_executor().spawn(async move {
            doc.render_region(page, DRAWN_SCALE, region)
                .map(|b| (b.png(), "png"))
        }))
    }

    fn copy_menu_picture(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.menu_picture(cx) else {
            return;
        };
        self.picture_menu = None;
        cx.spawn(async move |this, cx| {
            let picture = task.await;
            // As PNG, which every program pastes.
            let png = match picture {
                Some((bytes, "jpg")) => {
                    cx.background_executor()
                        .spawn(async move { as_png(&bytes) })
                        .await
                }
                Some((bytes, _)) => Some(bytes),
                None => None,
            };
            this.update(cx, |this, cx| {
                this.notice = Some(match png {
                    Some(bytes) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                            ImageFormat::Png,
                            bytes,
                        )));
                        super::said("picture-copied", &[])
                    }
                    None => super::said("picture-copy-failed", &[]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save_menu_picture(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.menu_picture(cx) else {
            return;
        };
        let Some(menu) = self.picture_menu.take().and_then(|m| m.picture) else {
            return;
        };
        let Some(path) = self.path.clone() else {
            return;
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let Some((bytes, kind)) = task.await else {
                this.update(cx, |this, cx| {
                    this.notice = Some(super::said("picture-copy-failed", &[]));
                    cx.notify();
                })
                .ok();
                return;
            };
            let name = format!("{stem}-p{}-{}.{kind}", menu.page + 1, menu.number);
            let Ok(chosen) = this.update(cx, |_, cx| cx.prompt_for_new_path(&folder, Some(&name)))
            else {
                return;
            };
            let Ok(Ok(Some(target))) = chosen.await else {
                return;
            };
            let written = cx
                .background_executor()
                .spawn(async move {
                    std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
                    Ok::<_, String>(target)
                })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match written {
                    Ok(target) => super::said(
                        "object-saved-picture",
                        &[("name", &super::file_name(&target))],
                    ),
                    Err(error) => super::said("object-save-failed", &[("error", &error)]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Copies page `page` to the clipboard as a picture, drawn at twice
    /// its printed size.
    pub(super) fn copy_page_picture(&mut self, page: usize, cx: &mut Context<Self>) {
        self.picture_menu = None;
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let doc = opened.doc.clone();
        let Some((w, h)) = doc.page_size(page) else {
            return;
        };
        let scale = 2.0;
        let region = Region {
            x: 0,
            y: 0,
            width: (w * scale).round().max(1.0) as u32,
            height: (h * scale).round().max(1.0) as u32,
        };
        cx.spawn(async move |this, cx| {
            let png = cx
                .background_executor()
                .spawn(async move { doc.render_region(page, scale, region).map(|b| b.png()) })
                .await;
            this.update(cx, |this, cx| {
                this.notice = Some(match png {
                    Some(bytes) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                            ImageFormat::Png,
                            bytes,
                        )));
                        super::said("page-menu-page-copied", &[])
                    }
                    None => super::said("picture-copy-failed", &[]),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens Find with the text selected in it.
    fn find_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(words) = self.selected_text() else {
            return;
        };
        let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
        self.open_search(window, cx);
        if let Some(bar) = &self.search {
            bar.input.update(cx, |input, cx| input.set_text(words, cx));
        }
        self.start_search(cx);
    }

    /// The pages' menu, at the pointer, while open.
    pub(super) fn picture_menu_element(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.picture_menu.as_ref()?;
        let at = menu.at;
        let th = self.theme;
        let item = |id: &'static str, glyph: Option<&str>, words: String, keys: Option<&str>| {
            float::menu_item(id, glyph, words, keys.map(|k| k.to_owned().into()), &th)
        };
        let close = |this: &mut Viewer| this.picture_menu = None;
        let mut body = float::menu("page-menu", 260.0, &th).on_mouse_down_out(cx.listener(
            |this, _, _, cx| {
                this.picture_menu = None;
                cx.notify();
            },
        ));
        let selected = self
            .selected_text()
            .filter(|_| self.selection.is_some_and(|s| s.anchor != s.head));
        let mut groups = 0;
        if let Some(words) = selected {
            let quoted: String = words.split_whitespace().collect::<Vec<_>>().join(" ");
            // The first words, as many as fit, cut at a space.
            let mut short = String::new();
            for word in quoted.split(' ') {
                if !short.is_empty() && short.chars().count() + 1 + word.chars().count() > QUOTED {
                    break;
                }
                if !short.is_empty() {
                    short.push(' ');
                }
                short.push_str(word);
            }
            let quoted = short.chars().take(QUOTED * 2).collect::<String>();
            body = body
                .child(
                    item(
                        "page-menu-copy",
                        Some("copy"),
                        t("page-menu-copy", &[]),
                        Some("Ctrl C"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        close(this);
                        this.copy_text(cx);
                        cx.notify();
                    })),
                )
                .child(float::menu_sep(&th));
            for (id, glyph, style) in [
                (
                    "page-menu-highlight",
                    "mark",
                    katna_pdf_cos::MarkupStyle::Highlight,
                ),
                (
                    "page-menu-underline",
                    "under",
                    katna_pdf_cos::MarkupStyle::Underline,
                ),
                (
                    "page-menu-strike",
                    "strike",
                    katna_pdf_cos::MarkupStyle::StrikeOut,
                ),
            ] {
                body = body.child(
                    item(id, Some(glyph), t(id, &[]), None).on_click(cx.listener(
                        move |this, _, _, cx| {
                            close(this);
                            this.mark_selection(style, cx);
                            cx.notify();
                        },
                    )),
                );
            }
            body = body
                .child(float::menu_sep(&th))
                .child(
                    item(
                        "page-menu-find",
                        Some("search"),
                        t("page-menu-find", &[("words", &quoted)]),
                        None,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.picture_menu = None;
                        this.find_selected(window, cx);
                    })),
                )
                .child(
                    item(
                        "page-menu-web",
                        Some("globe"),
                        t("page-menu-web", &[]),
                        None,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.picture_menu = None;
                        if let Some(words) = this.selected_text() {
                            let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
                            cx.open_url(&format!(
                                "https://duckduckgo.com/?q={}",
                                query_escape(&words)
                            ));
                        }
                        cx.notify();
                    })),
                );
            groups += 1;
        }
        if let Some(link) = menu.link.clone() {
            if groups > 0 {
                body = body.child(float::menu_sep(&th));
            }
            let address = match &link {
                LinkTarget::Uri(uri) => Some(uri.trim().to_owned()),
                LinkTarget::Page(_) => None,
            };
            body = body.child(
                item(
                    "page-menu-open-link",
                    Some("link"),
                    t("page-menu-open-link", &[]),
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picture_menu = None;
                    this.follow(&link, cx);
                    cx.notify();
                })),
            );
            if let Some(address) = address {
                body = body.child(
                    item(
                        "page-menu-copy-link",
                        Some("copy"),
                        t("page-menu-copy-link", &[]),
                        None,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picture_menu = None;
                        cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
                        cx.notify();
                    })),
                );
            }
            groups += 1;
        }
        if menu.picture.is_some() {
            if groups > 0 {
                body = body.child(float::menu_sep(&th));
            }
            body = body
                .child(
                    item("picture-copy", Some("img"), t("picture-copy", &[]), None)
                        .on_click(cx.listener(|this, _, _, cx| this.copy_menu_picture(cx))),
                )
                .child(
                    item(
                        "picture-save",
                        Some("download"),
                        t("picture-save", &[]),
                        None,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.save_menu_picture(cx))),
                );
            groups += 1;
        }
        if groups > 0 {
            body = body.child(float::menu_sep(&th));
        }
        let hand = self.hand;
        body = body
            .child(
                item(
                    "page-menu-select-all",
                    Some("text"),
                    t("page-menu-select-all", &[]),
                    Some("Ctrl A"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.select_page_text(cx);
                })),
            )
            .child(
                item(
                    "page-menu-hand",
                    Some(if hand { "cursor" } else { "hand" }),
                    t(
                        if hand {
                            "page-menu-select-tool"
                        } else {
                            "page-menu-hand"
                        },
                        &[],
                    ),
                    Some(if hand { "V" } else { "H" }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.picture_menu = None;
                    this.set_hand(!hand, cx);
                })),
            )
            .child(float::menu_sep(&th))
            .child(
                item(
                    "page-menu-zoom-in",
                    Some("plus"),
                    t("page-menu-zoom-in", &[]),
                    Some("Ctrl +"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.step_zoom(true, cx);
                })),
            )
            .child(
                item(
                    "page-menu-zoom-out",
                    Some("minus"),
                    t("page-menu-zoom-out", &[]),
                    Some("Ctrl −"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.step_zoom(false, cx);
                })),
            )
            .child(
                item(
                    "page-menu-fit-width",
                    Some("fitw"),
                    t("zoom-card-fit-width", &[]),
                    Some("Ctrl 1"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.set_zoom(Zoom::FitWidth, cx);
                })),
            )
            .child(
                item(
                    "page-menu-fit-page",
                    Some("fit"),
                    t("zoom-card-fit-page", &[]),
                    Some("Ctrl 2"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.set_zoom(Zoom::FitPage, cx);
                })),
            )
            .child(
                item(
                    "page-menu-turn",
                    Some("rot"),
                    t("page-menu-turn", &[]),
                    Some("Ctrl R"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.turn_view(1, cx);
                })),
            )
            .child(float::menu_sep(&th));
        if let Some(page) = menu.page {
            body = body.child(
                item(
                    "page-menu-copy-page",
                    Some("img"),
                    t("page-menu-copy-page", &[]),
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.copy_page_picture(page, cx))),
            );
        }
        body = body
            .child(
                item(
                    "page-menu-print",
                    Some("print"),
                    t("page-menu-print", &[]),
                    Some("Ctrl P"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.toggle_printing(cx);
                })),
            )
            .child(
                item(
                    "page-menu-properties",
                    Some("info"),
                    t("page-menu-properties", &[]),
                    Some("Ctrl D"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picture_menu = None;
                    this.toggle_properties(cx);
                })),
            );
        Some(
            gpui::deferred(
                katna_ui::anchored()
                    .position(at)
                    .snap_to_window()
                    .child(float::appear("page-menu-in", body, -4.0)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

/// `words` made safe to put in a web address's query.
fn query_escape(words: &str) -> String {
    let mut out = String::new();
    for byte in words.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A JPEG's picture as a PNG.
fn as_png(jpeg: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let mut out = std::io::Cursor::new(Vec::new());
    picture.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    #[test]
    fn jpegs_become_pngs() {
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 3)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let png = super::as_png(jpeg.get_ref()).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(super::as_png(b"not a picture").is_none());
    }

    #[test]
    fn words_go_into_a_query_safely() {
        assert_eq!(super::query_escape("a b&c/é"), "a+b%26c%2F%C3%A9");
    }
}
