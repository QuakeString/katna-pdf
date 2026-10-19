// SPDX-License-Identifier: GPL-3.0-or-later

//! The menu bar for KDE's global menu (Plasma's Global Menu applet and
//! the window title's menu button), as Katna Mail serves its own: the
//! items go on the session bus with `com.canonical.dbusmenu`
//! (`katna_platform::dbusmenu`), and Katna's GPUI tells Plasma where they
//! are. A click runs the same action as the item's keys.

use gpui::{Action, App, AsyncApp, WindowHandle};
use katna_platform::dbusmenu::{Menu, MenuItem};

use crate::tabs::Tabs;
use crate::text::t;
use crate::viewer::{
    ActualSize, Annotate, CloseTab, CopyText, Find, FindNext, FindPrevious, FitPage, FitWidth,
    NextPage, NextSpread, NextTab, Open, Organize, PreviousPage, PreviousTab, Print, Properties,
    Quit, Save, Sign, ToggleContents, ToggleNight, ZoomIn, ZoomOut,
};

/// The menus, their items, and each item's keys.
fn items() -> Vec<MenuItem> {
    let item = |id: &str, keys: &[&str]| {
        MenuItem::action(t(id, &[]), id).shortcut(keys.iter().map(|k| (*k).to_owned()).collect())
    };
    let ctrl = |key: &'static str| ["Control", key];
    vec![
        MenuItem::submenu(
            t("menu-file", &[]),
            vec![
                item("menu-open", &ctrl("O")),
                item("menu-save", &ctrl("S")),
                item("menu-print", &ctrl("P")),
                item("menu-properties", &ctrl("D")),
                MenuItem::Separator,
                item("menu-sign", &[]),
                MenuItem::Separator,
                item("menu-close-tab", &ctrl("W")),
                item("menu-quit", &ctrl("Q")),
            ],
        ),
        MenuItem::submenu(
            t("menu-edit", &[]),
            vec![
                item("menu-copy", &ctrl("C")),
                MenuItem::Separator,
                item("menu-find", &ctrl("F")),
                item("menu-find-next", &["F3"]),
                item("menu-find-previous", &["Shift", "F3"]),
                MenuItem::Separator,
                item("menu-annotate", &[]),
                item("menu-organize", &[]),
            ],
        ),
        MenuItem::submenu(
            t("menu-view", &[]),
            vec![
                item("menu-zoom-in", &ctrl("=")),
                item("menu-zoom-out", &ctrl("-")),
                item("menu-actual-size", &ctrl("0")),
                item("menu-fit-width", &ctrl("1")),
                item("menu-fit-page", &ctrl("2")),
                item("menu-layout", &ctrl("3")),
                MenuItem::Separator,
                item("menu-side-pane", &["F7"]),
                item("menu-night", &["Control", "Shift", "N"]),
            ],
        ),
        MenuItem::submenu(
            t("menu-go", &[]),
            vec![
                item("menu-next-page", &["Page Down"]),
                item("menu-previous-page", &["Page Up"]),
                MenuItem::Separator,
                item("menu-next-tab", &ctrl("Tab")),
                item("menu-previous-tab", &["Control", "Shift", "Tab"]),
            ],
        ),
    ]
}

/// The action an item runs.
fn action(id: &str) -> Option<Box<dyn Action>> {
    Some(match id {
        "menu-open" => Box::new(Open),
        "menu-save" => Box::new(Save),
        "menu-print" => Box::new(Print),
        "menu-properties" => Box::new(Properties),
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
        "menu-layout" => Box::new(NextSpread),
        "menu-side-pane" => Box::new(ToggleContents),
        "menu-night" => Box::new(ToggleNight),
        "menu-next-page" => Box::new(NextPage),
        "menu-previous-page" => Box::new(PreviousPage),
        "menu-next-tab" => Box::new(NextTab),
        "menu-previous-tab" => Box::new(PreviousTab),
        _ => return None,
    })
}

/// Serves the menu bar and points the windows opened from now on to it.
/// Clicks arrive on the returned channel, as item ids; nothing is served
/// when there is no session bus.
pub fn serve(cx: &mut App) -> Option<async_channel::Receiver<String>> {
    let connection = futures_lite::future::block_on(zbus::Connection::session()).ok()?;
    let service = connection.unique_name()?.to_string();
    let (sender, clicks) = async_channel::unbounded();
    let path = crate::ids::menu_bar_path();
    let menu =
        futures_lite::future::block_on(Menu::serve(&connection, &path, items(), move |id| {
            let _ = sender.try_send(id.to_owned());
        }));
    match menu {
        Ok(menu) => {
            cx.set_global(MenuBar(menu));
            katna_ui::native::set_kde_appmenu(service, path);
            Some(clicks)
        }
        Err(err) => {
            eprintln!("katna-pdf: no menu bar for the global menu: {err}");
            None
        }
    }
}

/// The menu served, kept for as long as the app runs.
struct MenuBar(#[allow(dead_code)] Menu);

impl gpui::Global for MenuBar {}

/// Runs the items clicked in `window`.
pub async fn run(
    window: WindowHandle<Tabs>,
    clicks: async_channel::Receiver<String>,
    cx: &mut AsyncApp,
) {
    while let Ok(id) = clicks.recv().await {
        let Some(action) = action(&id) else {
            continue;
        };
        let _ = window.update(cx, |_, window, cx| window.dispatch_action(action, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every item runs an action, and has its English text.
    #[test]
    fn every_item_does_something() {
        fn ids(items: &[MenuItem], out: &mut Vec<String>) {
            for item in items {
                match item {
                    MenuItem::Action { action, label, .. } => {
                        assert_ne!(label, action, "{action} has no text");
                        out.push(action.clone());
                    }
                    MenuItem::Submenu { items, .. } => ids(items, out),
                    MenuItem::Separator => {}
                }
            }
        }
        let mut all = Vec::new();
        ids(&items(), &mut all);
        assert!(all.len() > 20);
        for id in all {
            assert!(action(&id).is_some(), "{id} runs nothing");
        }
    }
}
