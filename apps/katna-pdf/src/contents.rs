// SPDX-License-Identifier: GPL-3.0-or-later

//! The contents pane: the document's outline (its bookmarks) beside the
//! pages, as Okular's side pane shows it (docs/PLAN.md, Phase 1). Items
//! with children open and close; clicking an item goes to its page.

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
pub type Path = Vec<usize>;

/// A change to the outline from the pane's buttons.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A new item after the one picked (at the end when none is), going
    /// to this place.
    Add(String, Destination),
    Rename(String),
    /// The picked item goes to this place now.
    Place(Destination),
    Up,
    Down,
    /// Under the item above it, as its last child.
    Indent,
    /// Out from under its parent, just after it.
    Outdent,
    Delete,
}

pub struct Contents {
    pub items: Vec<OutlineItem>,
    /// The item picked, for the pane's buttons to act on.
    pub chosen: Option<Path>,
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
        Self {
            items,
            chosen: None,
            scroll: ScrollHandle::new(),
            bar: ScrollBar::default(),
        }
    }

    /// Opens a closed item or closes an open one.
    pub fn toggle(&mut self, path: &Path) {
        if let Some(item) = get_mut(&mut self.items, path) {
            item.open = !item.open;
        }
    }

    /// The outline after `change`, and the item to pick then; nothing
    /// when the change cannot be made.
    pub fn changed(&self, change: &Change) -> Option<(Vec<OutlineItem>, Option<Path>)> {
        let mut items = self.items.clone();
        let chosen = self.chosen.clone().filter(|p| get(&items, p).is_some());
        let path = match (change, chosen) {
            (Change::Add(title, dest), at) => {
                let item = OutlineItem {
                    title: title.clone(),
                    dest: Some(*dest),
                    open: false,
                    children: Vec::new(),
                };
                let mut at = at.unwrap_or_else(|| vec![items.len().saturating_sub(1)]);
                if items.is_empty() {
                    items.push(item);
                    return Some((items, Some(vec![0])));
                }
                let (list, i) = siblings(&mut items, &at)?;
                list.insert(i + 1, item);
                *at.last_mut()? += 1;
                at
            }
            (_, None) => return None,
            (Change::Rename(title), Some(at)) => {
                get_mut(&mut items, &at)?.title = title.trim().to_owned();
                at
            }
            (Change::Place(dest), Some(at)) => {
                get_mut(&mut items, &at)?.dest = Some(*dest);
                at
            }
            (Change::Up, Some(mut at)) => {
                let (list, i) = siblings(&mut items, &at)?;
                if i == 0 {
                    return None;
                }
                list.swap(i, i - 1);
                *at.last_mut()? -= 1;
                at
            }
            (Change::Down, Some(mut at)) => {
                let (list, i) = siblings(&mut items, &at)?;
                if i + 1 >= list.len() {
                    return None;
                }
                list.swap(i, i + 1);
                *at.last_mut()? += 1;
                at
            }
            (Change::Indent, Some(at)) => {
                let (list, i) = siblings(&mut items, &at)?;
                if i == 0 {
                    return None;
                }
                let item = list.remove(i);
                let above = &mut list[i - 1];
                above.children.push(item);
                above.open = true;
                let mut at = at;
                *at.last_mut()? -= 1;
                at.push(above.children.len() - 1);
                at
            }
            (Change::Outdent, Some(at)) => {
                if at.len() < 2 {
                    return None;
                }
                let parent = at[..at.len() - 1].to_vec();
                let item = {
                    let (list, i) = siblings(&mut items, &at)?;
                    list.remove(i)
                };
                let (list, p) = siblings(&mut items, &parent)?;
                list.insert(p + 1, item);
                let mut at = parent;
                *at.last_mut()? += 1;
                at
            }
            (Change::Delete, Some(at)) => {
                let (list, i) = siblings(&mut items, &at)?;
                list.remove(i);
                // The next one is picked, or the one before it.
                let mut at = at;
                if i >= list.len() {
                    if i == 0 {
                        at.pop();
                        return Some((items, (!at.is_empty()).then_some(at)));
                    }
                    *at.last_mut()? -= 1;
                }
                at
            }
        };
        Some((items, Some(path)))
    }

    /// Opens every item above `path`, so it shows.
    pub fn reveal(&mut self, path: &Path) {
        for n in 1..path.len() {
            if let Some(item) = get_mut(&mut self.items, &path[..n]) {
                item.open = true;
            }
        }
    }

    /// The lines shown: every item whose parents are all open.
    pub fn rows(&self) -> Vec<Row> {
        fn walk(items: &[OutlineItem], path: &mut Path, out: &mut Vec<Row>) {
            for (i, item) in items.iter().enumerate() {
                path.push(i);
                let is_open = item.open && !item.children.is_empty();
                out.push(Row {
                    path: path.clone(),
                    depth: path.len() - 1,
                    title: item.title.clone(),
                    dest: item.dest,
                    fold: (!item.children.is_empty()).then_some(is_open),
                });
                if is_open {
                    walk(&item.children, path, out);
                }
                path.pop();
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }
}

fn get<'a>(items: &'a [OutlineItem], path: &[usize]) -> Option<&'a OutlineItem> {
    let (&first, rest) = path.split_first()?;
    let item = items.get(first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        get(&item.children, rest)
    }
}

fn get_mut<'a>(items: &'a mut [OutlineItem], path: &[usize]) -> Option<&'a mut OutlineItem> {
    let (&first, rest) = path.split_first()?;
    let item = items.get_mut(first)?;
    if rest.is_empty() {
        Some(item)
    } else {
        get_mut(&mut item.children, rest)
    }
}

/// The list holding the item at `path`, and its place in it.
fn siblings<'a>(
    items: &'a mut Vec<OutlineItem>,
    path: &[usize],
) -> Option<(&'a mut Vec<OutlineItem>, usize)> {
    let (&last, parents) = path.split_last()?;
    let list = match parents {
        [] => items,
        _ => &mut get_mut(items, parents)?.children,
    };
    (last < list.len()).then_some((list, last))
}

/// One line, its fold arrow toggling and the rest going to its page.
/// `chosen` is drawn picked; `editor`, when given, takes the title's place
/// (the title being renamed). `on_go` is told how many clicks there were.
pub fn row(
    row: Row,
    th: Theme,
    chosen: bool,
    editor: Option<gpui::AnyElement>,
    on_fold: impl Fn(&mut gpui::Window, &mut gpui::App) + 'static,
    on_go: impl Fn(usize, &mut gpui::Window, &mut gpui::App) + 'static,
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
        .when(chosen, |d| d.bg(th.field))
        .hover(move |s| s.bg(th.hover))
        .cursor_pointer()
        .on_click(move |e, window, cx| on_go(e.click_count(), window, cx))
        // Items without children line up with their siblings' titles.
        .child(fold.map_or_else(
            || div().w(px(20.0)).flex_shrink_0().into_any_element(),
            IntoElement::into_any_element,
        ))
        .child(match editor {
            Some(editor) => div().flex_1().min_w_0().child(editor).into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(px(text::SMALL))
                .when(row.depth == 0, |d| d.font_weight(FontWeight::MEDIUM))
                .text_color(if row.dest.is_some() { th.text } else { th.dim })
                .child(row.title)
                .into_any_element(),
        })
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

    fn titles(items: &[OutlineItem]) -> String {
        items
            .iter()
            .map(|i| {
                if i.children.is_empty() {
                    i.title.clone()
                } else {
                    format!("{}({})", i.title, titles(&i.children))
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_outline_changes_from_the_buttons() {
        let place = Destination { page: 3, top: None };
        let mut c = Contents::new(Vec::new());
        // Nothing picked: only adding works.
        assert_eq!(c.changed(&Change::Up), None);
        let (items, at) = c.changed(&Change::Add("A".into(), place)).unwrap();
        assert_eq!((titles(&items), at.clone()), ("A".into(), Some(vec![0])));
        c = Contents::new(vec![
            item("A", true, vec![item("A1", false, vec![])]),
            item("B", false, vec![]),
            item("C", false, vec![]),
        ]);
        let step = |c: &mut Contents, change: Change, want: &str, at: &[usize]| {
            let (items, chosen) = c.changed(&change).unwrap();
            assert_eq!(titles(&items), want, "{change:?}");
            assert_eq!(chosen.as_deref(), Some(at), "{change:?}");
            c.items = items;
            c.chosen = chosen;
        };
        c.chosen = Some(vec![1]);
        step(&mut c, Change::Add("N".into(), place), "A(A1) B N C", &[2]);
        assert_eq!(c.items[2].dest, Some(place));
        step(&mut c, Change::Up, "A(A1) N B C", &[1]);
        step(&mut c, Change::Indent, "A(A1 N) B C", &[0, 1]);
        step(&mut c, Change::Up, "A(N A1) B C", &[0, 0]);
        assert_eq!(c.changed(&Change::Up), None);
        assert_eq!(c.changed(&Change::Indent), None);
        step(&mut c, Change::Outdent, "A(A1) N B C", &[1]);
        assert_eq!(c.changed(&Change::Outdent), None);
        step(
            &mut c,
            Change::Rename("  New ".into()),
            "A(A1) New B C",
            &[1],
        );
        step(&mut c, Change::Down, "A(A1) B New C", &[2]);
        step(&mut c, Change::Down, "A(A1) B C New", &[3]);
        assert_eq!(c.changed(&Change::Down), None);
        step(&mut c, Change::Delete, "A(A1) B C", &[2]);
        c.chosen = Some(vec![0, 0]);
        step(&mut c, Change::Delete, "A B C", &[0]);
        c.chosen = Some(vec![9]);
        assert_eq!(c.changed(&Change::Delete), None);
    }
}
