// SPDX-License-Identifier: GPL-3.0-or-later

//! The application menu: File, Edit, View, Go, each item's keys and the
//! action it runs. KDE's global menu serves it (`menu.rs`), and the
//! profile menu's Menu shows it in the window (`tabs.rs`).

use gpui::Action;

use crate::viewer::{
    ActualSize, AddBookmark, Annotate, CloseTab, Compare, CopyText, Export, Find, FindNext,
    FindPrevious, FitHeight, FitPage, FitWidth, NextPage, NextSpread, NextTab, Open, Organize,
    Present, PreviousPage, PreviousTab, Print, Properties, Quit, ReadAloud, ReopenSession, Save,
    ShowLibrary, Sign, Snapshot, ToggleContents, ToggleNight, ZoomIn, ZoomOut, ZoomToArea,
};

/// An item: its id (and the id of its words), and its keys as
/// dbusmenu names them. `None` is a line between groups.
pub type Entry = Option<(&'static str, &'static [&'static str])>;

const CTRL_SHIFT_E: &[&str] = &["Control", "Shift", "E"];
const CTRL_SHIFT_D: &[&str] = &["Control", "Shift", "D"];

/// The menus and their items.
pub const MENUS: &[(&str, &[Entry])] = &[
    (
        "menu-file",
        &[
            Some(("menu-open", &["Control", "O"])),
            Some(("menu-reopen-session", &[])),
            Some(("menu-save", &["Control", "S"])),
            Some(("menu-print", &["Control", "P"])),
            Some(("menu-export", CTRL_SHIFT_E)),
            Some(("menu-properties", &["Control", "D"])),
            Some(("menu-compare", CTRL_SHIFT_D)),
            Some(("menu-library", &["Control", "L"])),
            None,
            Some(("menu-sign", &[])),
            None,
            Some(("menu-close-tab", &["Control", "W"])),
            Some(("menu-quit", &["Control", "Q"])),
        ],
    ),
    (
        "menu-edit",
        &[
            Some(("menu-copy", &["Control", "C"])),
            Some(("menu-snapshot", &[])),
            None,
            Some(("menu-find", &["Control", "F"])),
            Some(("menu-find-next", &["F3"])),
            Some(("menu-find-previous", &["Shift", "F3"])),
            None,
            Some(("menu-annotate", &[])),
            Some(("menu-organize", &[])),
        ],
    ),
    (
        "menu-view",
        &[
            Some(("menu-zoom-in", &["Control", "="])),
            Some(("menu-zoom-out", &["Control", "-"])),
            Some(("menu-actual-size", &["Control", "0"])),
            Some(("menu-fit-width", &["Control", "1"])),
            Some(("menu-fit-page", &["Control", "2"])),
            Some(("menu-fit-height", &["Control", "4"])),
            Some(("menu-zoom-area", &[])),
            Some(("menu-layout", &["Control", "3"])),
            None,
            Some(("menu-side-pane", &["F7"])),
            Some(("menu-night", &["Control", "Shift", "N"])),
            Some(("menu-read-aloud", &["Control", "Shift", "Y"])),
            Some(("menu-present", &["F5"])),
        ],
    ),
    (
        "menu-go",
        &[
            Some(("menu-next-page", &["Page Down"])),
            Some(("menu-previous-page", &["Page Up"])),
            Some(("menu-add-bookmark", &["Control", "B"])),
            None,
            Some(("menu-next-tab", &["Control", "Tab"])),
            Some(("menu-previous-tab", &["Control", "Shift", "Tab"])),
        ],
    ),
];

/// The action an item runs.
pub fn action(id: &str) -> Option<Box<dyn Action>> {
    Some(match id {
        "menu-open" => Box::new(Open),
        "menu-reopen-session" => Box::new(ReopenSession),
        "menu-save" => Box::new(Save),
        "menu-print" => Box::new(Print),
        "menu-export" => Box::new(Export),
        "menu-properties" => Box::new(Properties),
        "menu-compare" => Box::new(Compare),
        "menu-library" => Box::new(ShowLibrary),
        "menu-sign" => Box::new(Sign),
        "menu-close-tab" => Box::new(CloseTab),
        "menu-quit" => Box::new(Quit),
        "menu-copy" => Box::new(CopyText),
        "menu-find" => Box::new(Find),
        "menu-find-next" => Box::new(FindNext),
        "menu-find-previous" => Box::new(FindPrevious),
        "menu-annotate" => Box::new(Annotate),
        "menu-organize" => Box::new(Organize),
        "menu-zoom-in" => Box::new(ZoomIn),
        "menu-zoom-out" => Box::new(ZoomOut),
        "menu-actual-size" => Box::new(ActualSize),
        "menu-fit-width" => Box::new(FitWidth),
        "menu-fit-page" => Box::new(FitPage),
        "menu-fit-height" => Box::new(FitHeight),
        "menu-zoom-area" => Box::new(ZoomToArea),
        "menu-snapshot" => Box::new(Snapshot),
        "menu-layout" => Box::new(NextSpread),
        "menu-side-pane" => Box::new(ToggleContents),
        "menu-night" => Box::new(ToggleNight),
        "menu-read-aloud" => Box::new(ReadAloud),
        "menu-present" => Box::new(Present),
        "menu-next-page" => Box::new(NextPage),
        "menu-previous-page" => Box::new(PreviousPage),
        "menu-add-bookmark" => Box::new(AddBookmark),
        "menu-next-tab" => Box::new(NextTab),
        "menu-previous-tab" => Box::new(PreviousTab),
        _ => return None,
    })
}

/// Keys as people read them in the window: `Ctrl Shift E`.
pub fn keys_text(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| match *k {
            "Control" => "Ctrl",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A menu's words without the `_` that marks its access key.
pub fn plain(words: &str) -> String {
    words.replace('_', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_runs_an_action() {
        for (_, entries) in MENUS {
            for (id, _) in entries.iter().flatten() {
                assert!(action(id).is_some(), "{id} runs nothing");
            }
        }
    }

    #[test]
    fn keys_read_as_people_type_them() {
        assert_eq!(keys_text(&["Control", "Shift", "E"]), "Ctrl Shift E");
        assert_eq!(plain("Fit _Width"), "Fit Width");
    }
}
