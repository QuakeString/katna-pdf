// SPDX-License-Identifier: GPL-3.0-or-later

//! Typing in a text box comment in place, with its words' looks, as
//! Acrobat does: the box opens for typing where it is on the page (a
//! new one at once, one there with a double-click), with a bar over it
//! for bold, italic, underline, strikeout, the font, the size, the
//! alignment and the colour of the words picked or about to be typed.
//! The words and their looks are written into the file as rich text
//! (`/RC`) beside the plain words, and drawn in the box's appearance.

use gpui::{AnyElement, Context, Entity, Focusable, Subscription, Window, div, prelude::*};
use katna_pdf_cos::{
    AnnotChanges, AnnotKind, Annotation, Family, ParaAlign, Ref, RichPara, SpanStyle,
};
use katna_ui::px;
use katna_ui::rich::doc::Run;
use katna_ui::rich::{
    Align, Block, CharStyle, Doc, Font, Palette, Para, ParaStyle, RichEditor, RichEvent, Size,
};
use katna_ui::scale::desktop_px;
use katna_ui::tokens::{radius, space, text};

use super::Viewer;
use super::insert::{INKS, colour};
use crate::float::{self, FloatTip};
use crate::text::t;

/// A change of looks the bar makes to the words in the editor.
type Change = dyn Fn(&mut RichEditor, &mut Context<RichEditor>);

/// A text box being typed in.
pub(super) struct BoxEdit {
    pub id: Ref,
    pub page: usize,
    editor: Entity<RichEditor>,
    _editor: Subscription,
    /// The box's own size and colour, which words not set otherwise take.
    size: f32,
    ink: [f32; 3],
}

/// `c` as `0xRRGGBB`.
fn rgb(c: [f32; 3]) -> u32 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2])
}

fn of_rgb(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}

/// The editor's size step nearest to `ratio` of the box's own size.
fn size_step(ratio: f32) -> Size {
    Size::ALL
        .into_iter()
        .min_by(|a, b| {
            (a.scale() - ratio)
                .abs()
                .total_cmp(&(b.scale() - ratio).abs())
        })
        .unwrap_or_default()
}

/// How a span looks in the editor, in a box of `size` and colour `ink`.
fn char_style(s: &SpanStyle, size: f32, ink: [f32; 3]) -> CharStyle {
    CharStyle {
        bold: s.bold,
        italic: s.italic,
        underline: s.underline,
        strike: s.strike,
        font: match s.family {
            Family::Sans => Font::Sans,
            Family::Serif => Font::Serif,
            Family::Mono => Font::Fixed,
        },
        size: s
            .size
            .map_or(Size::Normal, |v| size_step(v / size.max(1.0))),
        color: s.colour.filter(|c| rgb(*c) != rgb(ink)).map(rgb),
        background: None,
        link: None,
    }
}

/// How a run the editor holds is written into the file.
fn span_style(c: &CharStyle, size: f32) -> SpanStyle {
    SpanStyle {
        bold: c.bold,
        italic: c.italic,
        underline: c.underline,
        strike: c.strike,
        family: match c.font {
            Font::Serif | Font::Garamond | Font::Georgia => Family::Serif,
            Font::Fixed => Family::Mono,
            _ => Family::Sans,
        },
        colour: c.color.map(of_rgb),
        size: (c.size != Size::Normal).then(|| (size * c.size.scale() * 10.0).round() / 10.0),
    }
}

/// The text box's words as the editor holds them.
fn doc_of(a: &Annotation, size: f32) -> Doc {
    let paras = if a.rich.is_empty() {
        katna_pdf_cos::plain_paras(&a.contents, &SpanStyle::default(), ParaAlign::Left)
    } else {
        a.rich.clone()
    };
    let blocks = paras
        .iter()
        .map(|p| {
            let mut para = Para {
                text: String::new(),
                runs: Vec::new(),
                style: ParaStyle {
                    align: match p.align {
                        ParaAlign::Left => Align::Left,
                        ParaAlign::Center => Align::Center,
                        ParaAlign::Right => Align::Right,
                    },
                    ..ParaStyle::default()
                },
            };
            for s in p.spans.iter().filter(|s| !s.text.is_empty()) {
                para.text.push_str(&s.text);
                para.runs.push(Run {
                    len: s.text.len(),
                    style: char_style(&s.style, size, a.colour),
                });
            }
            Block::Para(para)
        })
        .collect();
    Doc { blocks }
}

/// The editor's words as paragraphs to write.
fn paras_of(doc: &Doc, size: f32) -> Vec<RichPara> {
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Para(p) => Some(p),
            _ => None,
        })
        .map(|p| {
            let mut para = RichPara {
                spans: Vec::new(),
                align: match p.style.align {
                    Align::Left => ParaAlign::Left,
                    Align::Center => ParaAlign::Center,
                    Align::Right => ParaAlign::Right,
                },
            };
            for (range, style) in p.spans() {
                if let Some(text) = p.text.get(range) {
                    para.push(text, &span_style(style, size));
                }
            }
            para
        })
        .collect()
}

impl Viewer {
    /// Opens the chosen text box for typing in place.
    pub(super) fn start_box_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.chosen_annotation().cloned() else {
            return;
        };
        let (Some(id), AnnotKind::FreeText { size }) = (a.id, &a.kind) else {
            return;
        };
        if a.flags.locked || self.box_edit.as_ref().is_some_and(|b| b.id == id) {
            return;
        }
        let size = *size;
        self.finish_box_edit(true, cx);
        self.finish_comment(true, cx);
        let doc = doc_of(&a, size);
        let accent: gpui::Hsla = gpui::rgba(self.theme.katna.accent).into();
        let editor = cx.new(|cx| {
            let mut editor = RichEditor::new(t("textbox-placeholder", &[]), cx);
            editor.set_palette(Palette {
                accent,
                ..Palette::default()
            });
            let end = doc.end();
            editor.set_doc(doc, end, cx);
            editor
        });
        let sub = cx.subscribe(&editor, |this, _, event: &RichEvent, cx| match event {
            RichEvent::Submit | RichEvent::Cancel => this.finish_box_edit(true, cx),
            RichEvent::Changed | RichEvent::Selection => cx.notify(),
            _ => {}
        });
        window.focus(&editor.focus_handle(cx), cx);
        self.box_edit = Some(BoxEdit {
            id,
            page: a.page,
            editor,
            _editor: sub,
            size,
            ink: a.colour,
        });
        cx.notify();
    }

    /// Ends typing in a text box, writing what was typed when `keep`.
    pub(super) fn finish_box_edit(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.box_edit.take() else {
            return;
        };
        cx.notify();
        if !keep {
            return;
        }
        let paras = paras_of(edit.editor.read(cx).doc(), edit.size);
        let Some(mut a) = self
            .annots
            .as_ref()
            .and_then(|all| all.iter().flatten().find(|a| a.id == Some(edit.id)))
            .cloned()
        else {
            return;
        };
        let words = katna_pdf_cos::rich_plain_text(&paras);
        let rich = if katna_pdf_cos::is_styled(&paras) {
            paras
        } else {
            Vec::new()
        };
        if a.contents == words && a.rich == rich {
            return;
        }
        a.contents = words;
        a.rich = rich;
        self.change_annotations(
            AnnotChanges {
                put: vec![a],
                remove: Vec::new(),
            },
            cx,
        );
    }

    /// The text box being typed in on page `page`, where it is, with its
    /// bar of looks over it.
    pub(super) fn box_edit_overlay(&self, page: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(edit) = self.box_edit.as_ref().filter(|b| b.page == page) else {
            return Vec::new();
        };
        let Some(a) = self
            .annots
            .as_ref()
            .and_then(|all| all.get(page))
            .and_then(|on| on.iter().find(|a| a.id == Some(edit.id)))
        else {
            return Vec::new();
        };
        let Some(transform) = self.page_transform(page) else {
            return Vec::new();
        };
        let shown = self.shown;
        let b = katna_pdf_render::map_rect(transform, a.rect);
        let ink: gpui::Hsla = colour(edit.ink).into();
        let accent = gpui::rgba(self.theme.katna.accent);
        let pad = 2.0 * shown;
        let typing = div()
            .id("textbox-typing")
            .absolute()
            .left(desktop_px(b[0] * shown))
            .top(desktop_px(b[1] * shown))
            .w(desktop_px((b[2] - b[0]) * shown))
            .min_h(desktop_px((b[3] - b[1]) * shown))
            .p(desktop_px(pad))
            .bg(self.paper())
            .border_1()
            .border_color(accent)
            .cursor(gpui::CursorStyle::IBeam)
            .text_size(desktop_px(edit.size * shown))
            .line_height(desktop_px(edit.size * 1.2 * shown))
            .text_color(ink)
            // Presses in the box are the typing's, not the page's.
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(edit.editor.clone());
        let bar = self.box_edit_bar(edit, cx);
        // Over the box, or under it when the page's top is too near.
        let above = b[1] * shown >= 48.0;
        let bar = div()
            .absolute()
            .left(desktop_px(b[0] * shown))
            .top(desktop_px(if above {
                b[1] * shown - 44.0
            } else {
                b[3] * shown + 6.0
            }))
            .child(bar);
        vec![typing.into_any_element(), bar.into_any_element()]
    }

    /// The bar of looks over the text box being typed in: they apply to
    /// the words picked, or to those typed next.
    fn box_edit_bar(&self, edit: &BoxEdit, cx: &mut Context<Self>) -> AnyElement {
        let th = self.theme;
        let editor = edit.editor.clone();
        let (style, para) = {
            let e = editor.read(cx);
            (e.current_style(), e.para_style())
        };
        let letter = |id: &'static str, glyph: &'static str, tip: &'static str, on: bool| {
            let hover = th.hover;
            div()
                .id(id)
                .size(px(28.0))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .cursor_pointer()
                .text_size(px(text::BODY))
                .text_color(th.text)
                .when(on, |d| d.bg(th.accent_soft).text_color(th.accent_text))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .child(glyph)
                .ftip(t(tip, &[]), &th)
        };
        // Pressing the bar keeps the words picked and the caret where
        // they are.
        let keep = |d: gpui::Stateful<gpui::Div>| {
            d.on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
        };
        // Each change goes to the editor, which keeps the keys after.
        let act = |f: Box<Change>| {
            let editor = editor.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                editor.update(cx, |e, cx| f(e, cx));
                window.focus(&editor.focus_handle(cx), cx);
            }
        };
        let mut row = float::bar_shell(&th, radius::MD)
            .id("textbox-bar")
            .gap(px(2.0))
            .px(px(space::S1))
            .py(px(space::S1))
            .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                keep(letter("textbox-bold", "B", "textbox-bold", style.bold))
                    .font_weight(gpui::FontWeight::BOLD)
                    .on_click(act(Box::new(|e, cx| e.toggle_bold(cx)))),
            )
            .child(
                keep(letter(
                    "textbox-italic",
                    "I",
                    "textbox-italic",
                    style.italic,
                ))
                .italic()
                .on_click(act(Box::new(|e, cx| e.toggle_italic(cx)))),
            )
            .child(
                keep(letter(
                    "textbox-underline",
                    "U",
                    "textbox-underline",
                    style.underline,
                ))
                .underline()
                .on_click(act(Box::new(|e, cx| e.toggle_underline(cx)))),
            )
            .child(
                keep(letter(
                    "textbox-strike",
                    "S",
                    "textbox-strike",
                    style.strike,
                ))
                .line_through()
                .on_click(act(Box::new(|e, cx| e.toggle_strike(cx)))),
            )
            .child(float::vsep(&th));
        for (font, glyph, tip) in [
            (Font::Sans, "Aa", "textbox-sans"),
            (Font::Serif, "Aa", "textbox-serif"),
            (Font::Fixed, "Aa", "textbox-mono"),
        ] {
            let mut chip = keep(letter(tip, glyph, tip, style.font == font))
                .w(px(32.0))
                .on_click(act(Box::new(move |e, cx| e.set_font(font, cx))));
            chip = match font {
                Font::Serif => chip.font_family("Times New Roman"),
                Font::Fixed => chip.font_family("Courier New"),
                _ => chip,
            };
            row = row.child(chip);
        }
        // Smaller and bigger, a step at a time.
        let step = Size::ALL.iter().position(|s| *s == style.size).unwrap_or(1);
        for (id, glyph, by) in [
            ("textbox-smaller", "A−", -1i32),
            ("textbox-bigger", "A+", 1),
        ] {
            let to = (step as i32 + by).clamp(0, Size::ALL.len() as i32 - 1) as usize;
            row = row.child(
                keep(letter(id, glyph, id, false))
                    .w(px(32.0))
                    .on_click(act(Box::new(move |e, cx| e.set_size(Size::ALL[to], cx)))),
            );
        }
        row = row.child(float::vsep(&th));
        for (align, icon, tip) in [
            (Align::Left, "al", "textbox-left"),
            (Align::Center, "ac", "textbox-centre"),
            (Align::Right, "ar", "textbox-right"),
        ] {
            row = row.child(
                keep(float::sized_ib(
                    tip,
                    icon,
                    28.0,
                    16.0,
                    t(tip, &[]),
                    para.align == align,
                    &th,
                ))
                .on_click(act(Box::new(move |e, cx| e.set_align(align, cx)))),
            );
        }
        row = row.child(float::vsep(&th));
        let own = rgb(edit.ink);
        for (i, ink) in INKS.into_iter().take(5).enumerate() {
            let value = rgb(ink);
            let on = style.color.unwrap_or(own) == value;
            // The box's own colour is no colour of the words'.
            let set = (value != own).then_some(value);
            row = row.child(
                keep(float::swatch(
                    ("textbox-ink", i),
                    colour(ink).into(),
                    on,
                    &th,
                ))
                .ftip(t("tool-colour", &[]), &th)
                .on_click(act(Box::new(move |e, cx| e.set_color(set, cx)))),
            );
        }
        let done = cx.listener(|this, _, _, cx| this.finish_box_edit(true, cx));
        row = row.child(float::vsep(&th)).child(
            keep(float::sized_ib(
                "textbox-done",
                "check",
                28.0,
                16.0,
                t("textbox-done", &[]),
                false,
                &th,
            ))
            .on_click(done),
        );
        row.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use katna_pdf_cos::RichSpan;

    #[test]
    fn looks_go_to_the_editor_and_back() {
        let mut a = Annotation::new(0, AnnotKind::FreeText { size: 12.0 });
        a.colour = [0.0; 3];
        let mut p = RichPara::default();
        p.push("plain ", &SpanStyle::default());
        p.push(
            "bold red",
            &SpanStyle {
                bold: true,
                colour: Some([1.0, 0.0, 0.0]),
                ..SpanStyle::default()
            },
        );
        let big = RichPara {
            spans: vec![RichSpan {
                text: "Big".into(),
                style: SpanStyle {
                    size: Some(24.0),
                    family: Family::Serif,
                    ..SpanStyle::default()
                },
            }],
            align: ParaAlign::Center,
        };
        a.rich = vec![p.clone(), big];
        a.contents = "plain bold red\nBig".into();
        let doc = doc_of(&a, 12.0);
        let back = paras_of(&doc, 12.0);
        assert_eq!(back[0], p);
        assert_eq!(back[1].align, ParaAlign::Center);
        assert_eq!(back[1].spans[0].style.family, Family::Serif);
        // Sizes go by the editor's steps: twice the box's own is Huge.
        assert_eq!(back[1].spans[0].style.size, Some(24.0));
        // Plain words come back plain.
        let mut plain = Annotation::new(0, AnnotKind::FreeText { size: 12.0 });
        plain.contents = "one\ntwo".into();
        let back = paras_of(&doc_of(&plain, 12.0), 12.0);
        assert!(!katna_pdf_cos::is_styled(&back));
        assert_eq!(katna_pdf_cos::rich_plain_text(&back), "one\ntwo");
    }
}
