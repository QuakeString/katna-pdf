// SPDX-License-Identifier: GPL-3.0-or-later

//! The menu bar for KDE's global menu (Plasma's Global Menu applet and
//! the window title's menu button), as Katna Mail serves its own: the
//! items go on the session bus with `com.canonical.dbusmenu`
//! (`katna_platform::dbusmenu`), and Katna's GPUI tells Plasma where they
//! are. A click runs the same action as the item's keys.

use gpui::{App, AsyncApp, WindowHandle};
use katna_platform::dbusmenu::{Menu, MenuItem};

use crate::menus::{MENUS, action};
use crate::tabs::Tabs;
use crate::text::t;

/// The menus as KDE's global menu takes them (`menus::MENUS`).
fn items() -> Vec<MenuItem> {
    MENUS
        .iter()
        .map(|(title, entries)| {
            MenuItem::submenu(
                t(title, &[]),
                entries
                    .iter()
                    .map(|entry| match entry {
                        Some((id, keys)) => MenuItem::action(t(id, &[]), *id)
                            .shortcut(keys.iter().map(|k| (*k).to_owned()).collect()),
                        None => MenuItem::Separator,
                    })
                    .collect(),
            )
        })
        .collect()
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
