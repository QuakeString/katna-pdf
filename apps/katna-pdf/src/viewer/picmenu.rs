// SPDX-License-Identifier: GPL-3.0-or-later

//! Right-clicking a picture on a page, whatever tool is in hand: Copy
//! picture puts it on the clipboard as the file holds it (a JPEG as it
//! is, others as PNG), and Save picture as writes it to a file. A
//! picture this cannot read out (JPEG 2000, JBIG2, fax) is drawn from
//! the page instead.

use std::path::Path;
use std::sync::Arc;

use gpui::{AnyElement, ClipboardItem, Context, Image, ImageFormat, Pixels, Point, prelude::*};
use katna_pdf_cos::ObjectKind;
use katna_pdf_render::Region;

use super::{Content, Viewer};
use crate::float;
use crate::text::t;

/// The menu right-clicking a picture opens.
pub(super) struct PictureMenu {
    /// Where the pointer was, in the window.
    at: Point<Pixels>,
    page: usize,
    /// Which object on the page it is, from 1, for the file's name.
    number: usize,
    /// The picture's bytes and their kind (`jpg` or `png`), or `None`
    /// when they are to be drawn from the page's `rect` (points from its
    /// top left).
    picture: Option<Arc<(Vec<u8>, &'static str)>>,
    rect: [f32; 4],
}

/// A picture's bytes and their kind (`jpg` or `png`).
type Picture = (Vec<u8>, &'static str);

/// Pages this picture is drawn at when it is drawn from the page.
const DRAWN_SCALE: f32 = 3.0;

impl Viewer {
    /// A right click on the pages: a picture under the pointer gets its
    /// menu, read from the file as it is now.
    pub(super) fn picture_right_press(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) {
        self.picture_menu = None;
        let Some((page, point)) = self.page_point(event.position) else {
            return;
        };
        let Content::Ready(opened) = &self.content else {
            return;
        };
        let Some(transform) = opened.doc.page_transform(page) else {
            return;
        };
        let at = event.position;
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
                this.picture_menu = Some(PictureMenu {
                    at,
                    page,
                    number: index + 1,
                    picture: picture.map(Arc::new),
                    rect,
                });
                cx.notify();
            },
            cx,
        );
    }

    /// The picture's bytes and kind, drawing it from the page when the
    /// file's own cannot be read out.
    fn menu_picture(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Task<Option<Picture>>> {
        let menu = self.picture_menu.as_ref()?;
        if let Some(picture) = &menu.picture {
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
        let Some(menu) = self.picture_menu.take() else {
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

    /// The picture's menu, at the pointer, while open.
    pub(super) fn picture_menu_element(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let at = self.picture_menu.as_ref()?.at;
        let th = self.theme;
        let menu = float::menu("picture-menu", 220.0, &th)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.picture_menu = None;
                cx.notify();
            }))
            .child(
                float::menu_item(
                    "picture-copy",
                    Some("copy"),
                    t("picture-copy", &[]),
                    None,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.copy_menu_picture(cx))),
            )
            .child(
                float::menu_item(
                    "picture-save",
                    Some("download"),
                    t("picture-save", &[]),
                    None,
                    &th,
                )
                .on_click(cx.listener(|this, _, _, cx| this.save_menu_picture(cx))),
            );
        Some(
            gpui::deferred(
                katna_ui::anchored()
                    .position(at)
                    .snap_to_window()
                    .child(float::appear("picture-menu-in", menu, -4.0)),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
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
}
