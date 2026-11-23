// SPDX-License-Identifier: GPL-3.0-or-later

//! Damaged files: when a file's table of objects is broken, cos rebuilds
//! it as the file opens (`Document::repairs`). The file is then written
//! again whole as a change, so it can be changed and saved like any
//! other, and the Properties card lists what was done.

use gpui::Context;
use katna_pdf_cos::Repair;

use super::{Content, Viewer};
use crate::text::t;

impl Viewer {
    /// Reads, away from the window, whether the file just opened had to
    /// be repaired; if it did, says so and writes it again whole.
    pub(super) fn check_repairs(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Content::Ready(_)) = (self.path.clone(), &self.content) else {
            return;
        };
        let key = self.key.clone();
        let current = self.unsaved();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let repairs = cx
                .background_executor()
                .spawn(async move {
                    let bytes = match current {
                        Some(bytes) => bytes.to_vec(),
                        None => std::fs::read(&path).ok()?,
                    };
                    let doc = super::cos(bytes, key.as_deref()).ok()?;
                    // A table with objects in the wrong place shows as
                    // they are read; the page tree is read first.
                    let _ = doc.pages();
                    if doc.repairs().is_empty() {
                        return None;
                    }
                    if !doc.was_repaired() {
                        // Read them all, to count them.
                        for num in doc.object_numbers().collect::<Vec<_>>() {
                            let _ = doc.get(katna_pdf_cos::Ref::new(num, 0));
                        }
                    }
                    Some(doc.repairs())
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(repairs) = repairs.filter(|r| !r.is_empty()) else {
                    return;
                };
                if this.generation != generation {
                    return;
                }
                this.repairs = repairs;
                this.notice = Some(super::said("file-repaired", &[]));
                let pages = (0..this.page_count()).collect();
                this.change_document(
                    pages,
                    "repair-failed",
                    |doc| doc.repaired_copy().map_err(|e| e.to_string()),
                    cx,
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// What was done, as a sentence to read.
pub(super) fn said(repair: &Repair) -> String {
    match repair {
        Repair::NoStartxref => t("repair-no-startxref", &[]),
        Repair::TableUnreadable { at } => t("repair-table-unreadable", &[("at", &at.to_string())]),
        Repair::NoCatalog => t("repair-no-catalog", &[]),
        Repair::TableWrong => t("repair-table-wrong", &[]),
        Repair::ObjectsFound(n) => t("repair-objects-found", &[("count", &n.to_string())]),
        Repair::ObjectsMoved(n) => t("repair-objects-moved", &[("count", &n.to_string())]),
        Repair::ObjectsInStreams(n) => t("repair-objects-in-streams", &[("count", &n.to_string())]),
        Repair::TrailerFromKeyword => t("repair-trailer-keyword", &[]),
        Repair::TrailerFromXrefStream => t("repair-trailer-xref-stream", &[]),
        Repair::CatalogByType => t("repair-catalog-by-type", &[]),
    }
}
