// SPDX-License-Identifier: GPL-3.0-or-later

//! The command box (S12, Ctrl+K): one box that finds what the app can
//! do (the menus' items, with their keys), a page by its number, and
//! the words in the file, and runs or goes to the one chosen.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, Context, Entity, Focusable, KeyDownEvent, Subscription, Window, div, prelude::*,
};
use katna_ui::px;
use katna_ui::text_input::{InputEvent, TextInput};
use katna_ui::tokens::{radius, space, text};

use super::{Content, Viewer};
use crate::float;
use crate::menus::{self, MENUS};
use crate::search::{self, Hit};
use crate::text::t;

/// The most of each kind the box lists.
const MOST: usize = 6;

/// The command box, while open.
pub(super) struct CommandBox {
    input: Entity<TextInput>,
    /// The line chosen, counted over all the lines shown.
    chosen: usize,
    /// Words found in the file for what is typed.
    hits: Vec<Hit>,
    /// Moves on with each thing typed, so an older search stops.
    generation: Arc<AtomicU64>,
    _input: Subscription,
}

/// A line of the box.
#[derive(Clone)]
enum Line {
    /// A menu item: its id and keys.
    Do(&'static str, &'static [&'static str]),
    /// A page, from 0.
    Page(usize),
    /// A match in the file: its index in the hits.
    Found(usize),
}

/// Other words people look for an item by, beside its own (English,
/// like the words they stand beside).
fn also(id: &str) -> &'static str {
    match id {
        "menu-night" => "night dark invert sepia colors",
        "menu-sign" => "signature certificate token dsc certify",
        "menu-compare" => "diff versions changes",
        "menu-organize" => "pages reorder rotate delete move",
        "menu-export" => "png jpeg image word docx text csv",
        "menu-properties" => "metadata title author password lock size",
        "menu-library" => "files folders",
        "menu-read-aloud" => "speech speak voice",
        "menu-present" => "slideshow fullscreen",
        "menu-annotate" => "comment highlight note",
        "menu-settings" => "preferences options",
        "menu-side-pane" => "sidebar thumbnails bookmarks",
        "menu-go-to-page" => "jump",
        _ => "",
    }
}

/// The menu items whose words hold every word of `query`, as `words`
/// gives an item's words.
fn matching(
    query: &str,
    words: impl Fn(&str) -> String,
) -> Vec<(&'static str, &'static [&'static str])> {
    let query = query.to_lowercase();
    let parts: Vec<&str> = query.split_whitespace().collect();
    if parts.is_empty() {
        return Vec::new();
    }
    MENUS
        .iter()
        .flat_map(|(_, entries)| entries.iter().flatten())
        .filter(|(id, _)| {
            let said = format!("{} {}", menus::plain(&words(id)), also(id)).to_lowercase();
            parts.iter().all(|p| said.contains(p))
        })
        .copied()
        .take(MOST)
        .collect()
}

impl Viewer {
    /// Opens the command box, or closes it.
    pub(super) fn toggle_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command.is_some() {
            self.close_command(window, cx);
            return;
        }
        let input = cx.new(|cx| TextInput::new(t("command-placeholder", &[]), cx));
        input.update(cx, |input, _| {
            input.set_accent(gpui::rgba(self.theme.katna.accent).into());
        });
        let sub = cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
            InputEvent::Changed => this.command_typed(cx),
            InputEvent::Submit => this.run_command(window, cx),
            InputEvent::Cancel => this.close_command(window, cx),
        });
        window.focus(&input.focus_handle(cx), cx);
        self.command = Some(CommandBox {
            input,
            chosen: 0,
            hits: Vec::new(),
            generation: Arc::new(AtomicU64::new(0)),
            _input: sub,
        });
        cx.notify();
    }

    fn close_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.command.take() {
            command.generation.fetch_add(1, Ordering::Relaxed);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// What is typed changed: the first line chosen, and the file
    /// searched again.
    fn command_typed(&mut self, cx: &mut Context<Self>) {
        let (Some(command), Some(texts), Content::Ready(opened)) =
            (self.command.as_mut(), self.texts.clone(), &self.content)
        else {
            return;
        };
        command.chosen = 0;
        command.hits.clear();
        let mine = command.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let query = command.input.read(cx).text().trim().to_owned();
        cx.notify();
        if query.chars().count() < 2 {
            return;
        }
        let (doc, generation) = (opened.doc.clone(), command.generation.clone());
        let from = self.current_page();
        let options = self.find_options;
        let (sender, receiver) = async_channel::unbounded();
        cx.background_executor()
            .spawn(async move {
                let mut count = 0;
                search::run(
                    doc.as_ref(),
                    &texts,
                    (&query, options),
                    from,
                    &generation,
                    mine,
                    |_, hits| {
                        count += hits.len();
                        sender.send_blocking(hits).is_ok() && count < MOST
                    },
                );
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(hits) = receiver.recv().await {
                let going = this.update(cx, |this, cx| {
                    let Some(command) = this.command.as_mut() else {
                        return false;
                    };
                    if command.generation.load(Ordering::Relaxed) != mine {
                        return false;
                    }
                    command.hits.extend(hits);
                    command.hits.truncate(MOST);
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// The lines for what is typed: what can be done, a page, then what
    /// the file says.
    fn command_lines(&self, cx: &Context<Self>) -> Vec<Line> {
        let Some(command) = self.command.as_ref() else {
            return Vec::new();
        };
        let query = command.input.read(cx).text().trim().to_owned();
        let mut lines: Vec<Line> = matching(&query, |id| t(id, &[]))
            .into_iter()
            .map(|(id, keys)| Line::Do(id, keys))
            .collect();
        if let Ok(n) = query
            .trim_start_matches(['p', 'P', '.', ' '])
            .parse::<usize>()
            && n >= 1
            && n <= self.page_count()
        {
            lines.insert(0, Line::Page(n - 1));
        }
        lines.extend((0..command.hits.len()).map(Line::Found));
        lines
    }

    /// Runs or goes to the line chosen.
    fn run_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let lines = self.command_lines(cx);
        let Some(command) = self.command.as_ref() else {
            return;
        };
        let Some(line) = lines.get(command.chosen).cloned() else {
            return;
        };
        let hit = match &line {
            Line::Found(i) => command.hits.get(*i).cloned(),
            _ => None,
        };
        self.close_command(window, cx);
        match line {
            Line::Do(id, _) => {
                if let Some(action) = menus::action(id) {
                    window.dispatch_action(action, cx);
                }
            }
            Line::Page(page) => self.go_to_page(page, cx),
            Line::Found(_) => {
                if let Some(hit) = hit {
                    self.go_to_hit(&hit, cx);
                }
            }
        }
    }

    fn command_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let count = self.command_lines(cx).len();
        let Some(command) = self.command.as_mut() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "down" if count > 0 => command.chosen = (command.chosen + 1) % count,
            "up" if count > 0 => command.chosen = (command.chosen + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The command box over the window, while open.
    pub(super) fn command_box(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let command = self.command.as_ref()?;
        let th = self.theme;
        let query = command.input.read(cx).text().trim().to_owned();
        let lines = self.command_lines(cx);
        let chosen = command.chosen.min(lines.len().saturating_sub(1));
        // The words with what was typed in bold.
        let marked = |words: &str, on: bool| {
            let lower = words.to_lowercase();
            let q = query.to_lowercase();
            let at = (!q.is_empty()).then(|| lower.find(&q)).flatten();
            let colour = if on { th.accent_text } else { th.text };
            let row = div()
                .flex()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(colour);
            match at {
                Some(at) if words.is_char_boundary(at) && words.is_char_boundary(at + q.len()) => {
                    row.child(words[..at].to_owned())
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(words[at..at + q.len()].to_owned()),
                        )
                        .child(words[at + q.len()..].to_owned())
                }
                _ => row.child(words.to_owned()),
            }
        };
        let heading = |id: &str| {
            div()
                .px(px(space::S4))
                .pt(px(space::S3))
                .pb(px(space::S1))
                .text_size(px(text::MICRO))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(th.dim)
                .child(t(id, &[]).to_uppercase())
        };
        let mut list = div().flex().flex_col().pb(px(space::S2));
        let mut last_kind = None;
        for (n, line) in lines.iter().enumerate() {
            let kind = match line {
                Line::Do(..) => 0,
                Line::Page(_) => 1,
                Line::Found(_) => 2,
            };
            if last_kind != Some(kind) {
                list = list.child(heading(match kind {
                    0 => "command-do",
                    1 => "command-go",
                    _ => "command-in-file",
                }));
                last_kind = Some(kind);
            }
            let on = n == chosen;
            let (glyph, words, end) = match line {
                Line::Do(id, keys) => (
                    "bolt",
                    menus::plain(&t(id, &[])),
                    (!keys.is_empty()).then(|| menus::keys_text(keys)),
                ),
                Line::Page(page) => (
                    "page",
                    t("command-page", &[("page", &(page + 1).to_string())]),
                    None,
                ),
                Line::Found(i) => {
                    let hit = &command.hits[*i];
                    let (before, found, after) = &hit.context;
                    (
                        "search",
                        format!("{before}{found}{after}"),
                        Some(t(
                            "command-page-short",
                            &[("page", &(hit.page + 1).to_string())],
                        )),
                    )
                }
            };
            let hover = th.hover;
            list = list.child(
                div()
                    .id(("command-line", n))
                    .mx(px(space::S2))
                    .h(px(38.0))
                    .px(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .rounded(px(radius::SM))
                    .cursor_pointer()
                    .text_size(px(text::BODY))
                    .when(on, |d| d.bg(th.accent_soft))
                    .when(!on, |d| d.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(command) = this.command.as_mut() {
                            command.chosen = n;
                        }
                        this.run_command(window, cx);
                    }))
                    .child(float::icon(
                        glyph,
                        15.0,
                        if on { th.accent_text } else { th.text2 },
                    ))
                    .child(div().flex_1().min_w_0().child(marked(&words, on)))
                    .children(end.map(|end| {
                        div()
                            .flex_none()
                            .text_size(px(text::CAPTION))
                            .text_color(th.dim)
                            .child(end)
                    })),
            );
        }
        if lines.is_empty() && !query.is_empty() {
            list = list.child(
                div()
                    .px(px(space::S4))
                    .py(px(space::S4))
                    .text_size(px(text::SMALL))
                    .text_color(th.dim)
                    .child(t("command-nothing", &[])),
            );
        }
        let card = div()
            .id("command-card")
            .occlude()
            .w(px(580.0))
            .max_h(px(520.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(th.card)
            .shadow(th.lifted())
            .when(th.dark, |d| d.border_1().border_color(th.line))
            .overflow_hidden()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.command_key(event, cx);
            }))
            .child(
                div()
                    .h(px(52.0))
                    .px(px(space::S4))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .border_b_1()
                    .border_color(th.line_soft)
                    .child(float::icon("search", 18.0, th.text2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(28.0))
                            .text_size(px(16.0))
                            .child(command.input.clone()),
                    )
                    .child(float::kbd("Esc", &th)),
            )
            .child(
                div()
                    .id("command-lines")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            )
            .child(
                div()
                    .h(px(34.0))
                    .px(px(space::S4))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .border_t_1()
                    .border_color(th.line_soft)
                    .text_size(px(text::CAPTION))
                    .text_color(th.dim)
                    .child(div().flex_1().child(t("command-hint", &[])))
                    .child(float::kbd("↑↓", &th))
                    .child(float::kbd("Enter", &th)),
            );
        Some(
            div()
                .id("command-scrim")
                .absolute()
                .inset_0()
                .occlude()
                .bg(th.scrim)
                .flex()
                .justify_center()
                .pt(px(90.0))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_command(window, cx)),
                )
                .child(div().child(float::appear("command-in", card, -6.0)))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_match_every_word_typed() {
        let words = |id: &str| id.trim_start_matches("menu-").replace('-', " ");
        let found = matching("zoom in", words);
        assert_eq!(found.first().map(|(id, _)| *id), Some("menu-zoom-in"));
        assert!(matching("fit", words).len() >= 3);
        assert!(matching("", words).is_empty());
        assert!(matching("nothing like this", words).is_empty());
        assert_eq!(
            matching("dark", words).first().map(|(id, _)| *id),
            Some("menu-night")
        );
    }
}
