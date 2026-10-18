// SPDX-License-Identifier: GPL-3.0-or-later

//! The contents pane: the document's outline (its bookmarks) beside the
//! pages, as Okular's side pane shows it (docs/PLAN.md, Phase 1). Items
//! with children open and close; clicking an item goes to its page.

use std::collections::HashSet;

use gpui::{FontWeight, ScrollHandle, div, prelude::*, svg};
use katna_pdf_cos::{Destination, OutlineItem};
use katna_ui::tokens::{radius, space, text};
use katna_ui::{ScrollBar, px};

use crate::theme::Theme;

/// The pane's width.
pub const WIDTH: f32 = 280.0;
/// Each level of the outline is indented this much more.
const INDENT: f32 = 16.0;
const ROW: f32 = 32.0;

/// An item, by its place in the outline: indices from the top level down.
type Path = Vec<usize>;

pub struct Contents {
    items: Vec<OutlineItem>,
    /// The items shown open.
    open: HashSet<Path>,
    pub scroll: ScrollHandle,
    pub bar: ScrollBar,
}

/// One line of the pane.
#[derive(Debug, PartialEq)]
pub struct Row {
    pub path: Path,
    pub depth: usize,
    pub title: String,
    pub dest: Option<Destination>,
    /// `Some(open)` when the item has children.
    pub fold: Option<bool>,
}

impl Contents {
    /// The outline, opened as the file asks.
    pub fn new(items: Vec<OutlineItem>) -> Self {
        let mut open = HashSet::new();
        fn opened(items: &[OutlineItem], path: &mut Path, open: &mut HashSet<Path>) {
            for (i, item) in items.iter().enumerate() {
                path.push(i);
                if item.open && !item.children.is_empty() {
                    open.insert(path.clone());
                }
                opened(&item.children, path, open);
                path.pop();
            }
        }
        opened(&items, &mut Vec::new(), &mut open);
        Self {
            items,
            open,
            scroll: ScrollHandle::new(),
            bar: ScrollBar::default(),
        }
    }

    /// Opens a closed item or closes an open one.
    pub fn toggle(&mut self, path: &Path) {
        if !self.open.remove(path) {
            self.open.insert(path.clone());
        }
    }

    /// The lines shown: every item whose parents are all open.
    pub fn rows(&self) -> Vec<Row> {
        fn walk(items: &[OutlineItem], open: &HashSet<Path>, path: &mut Path, out: &mut Vec<Row>) {
            for (i, item) in items.iter().enumerate() {
                path.push(i);
                let is_open = open.contains(path);
                out.push(Row {
                    path: path.clone(),
                    depth: path.len() - 1,
                    title: item.title.clone(),
                    dest: item.dest,
                    fold: (!item.children.is_empty()).then_some(is_open),
                });
                if is_open {
                    walk(&item.children, open, path, out);
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &self.open, &mut Vec::new(), &mut out);
        out
    }
}

/// One line, its fold arrow toggling and the rest going to its page.
pub fn row(
    row: Row,
    th: Theme,
    on_fold: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
    on_go: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let fold = row.fold.map(|open| {
        div()
            .id("fold")
            .size(px(20.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::XS))
            .hover(move |s| s.bg(th.hover))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                on_fold(window, cx);
            })
            .child(
                svg()
                    .path(if open {
                        "icons/chevron-down.svg"
                    } else {
                        "icons/chevron-right.svg"
                    })
                    .size(px(16.0))
                    .text_color(th.dim),
            )
    });
    div()
        .id(gpui::ElementId::Name(
            format!(
                "outline-{}",
                row.path
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("-")
            )
            .into(),
        ))
        .h(px(ROW))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(space::S1))
        .pl(px(space::S2 + INDENT * row.depth as f32))
        .pr(px(space::S3))
        .mx(px(space::S2))
        .rounded(px(radius::XS))
        .hover(move |s| s.bg(th.hover))
        .cursor_pointer()
        .on_click(move |_, window, cx| on_go(window, cx))
        // Items without children line up with their siblings' titles.
        .child(fold.map_or_else(
            || div().w(px(20.0)).flex_shrink_0().into_any_element(),
            IntoElement::into_any_element,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(px(text::SMALL))
                .when(row.depth == 0, |d| d.font_weight(FontWeight::MEDIUM))
                .text_color(if row.dest.is_some() { th.text } else { th.dim })
                .child(row.title),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, open: bool, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            title: title.into(),
            dest: None,
            open,
            children,
        }
    }

    #[test]
    fn rows_follow_what_is_open() {
        let mut contents = Contents::new(vec![
            item(
                "A",
                true,
                vec![item("A1", false, vec![item("A1a", false, vec![])])],
            ),
            item("B", false, vec![item("B1", false, vec![])]),
        ]);
        let titles = |c: &Contents| c.rows().into_iter().map(|r| r.title).collect::<Vec<_>>();
        // A opens as the file asks; B stays closed.
        assert_eq!(titles(&contents), ["A", "A1", "B"]);
        contents.toggle(&vec![1]);
        assert_eq!(titles(&contents), ["A", "A1", "B", "B1"]);
        contents.toggle(&vec![0, 0]);
        let rows = contents.rows();
        assert_eq!(rows[2].title, "A1a");
        assert_eq!((rows[2].depth, rows[2].fold), (2, None));
        assert_eq!(rows[1].fold, Some(true));
        contents.toggle(&vec![0]);
        assert_eq!(titles(&contents), ["A", "B", "B1"]);
    }
}
