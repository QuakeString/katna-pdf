// SPDX-License-Identifier: GPL-3.0-or-later

//! How pages come on in a presentation (ISO 32000-2 §12.4.4): a page's
//! `/Trans` says how it replaces the one before, and its `/Dur` how many
//! seconds it stays before the next comes on by itself.

use crate::document::Document;
use crate::object::Object;

/// How a page comes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionStyle {
    /// At once.
    Replace,
    /// Two lines sweep across from the middle, or in to it (`/Split`).
    Split,
    /// Several lines sweep across together (`/Blinds`).
    Blinds,
    /// A rectangle grows from the middle, or shrinks to it (`/Box`).
    Box,
    /// One line sweeps across (`/Wipe`).
    Wipe,
    /// Bit by bit (`/Dissolve`).
    Dissolve,
    /// Small squares at random, sweeping across (`/Glitter`).
    Glitter,
    /// Flown in, or the old page flown out (`/Fly`).
    Fly,
    /// The new page pushes the old one off (`/Push`).
    Push,
    /// The new page slides over the old one (`/Cover`).
    Cover,
    /// The old page slides off the new one (`/Uncover`).
    Uncover,
    /// The old page fades into the new one (`/Fade`).
    Fade,
}

/// How page comes on, as its `/Trans` says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    pub style: TransitionStyle,
    /// Seconds it takes.
    pub seconds: f32,
    /// For Split and Blinds: across (horizontal) rather than down.
    pub horizontal: bool,
    /// For Split, Box and Fly: from the middle out rather than in.
    pub outward: bool,
    /// Where it moves to, in degrees counterclockwise from left to
    /// right (0 left to right, 90 bottom to top, 180 right to left,
    /// 270 top to bottom, 315 top left to bottom right); `None` for
    /// Fly and Glitter's `/None`.
    pub direction: Option<f32>,
}

/// How a page is shown in a presentation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PageShow {
    /// How it comes on, when its file says.
    pub transition: Option<Transition>,
    /// The seconds it stays before the next comes on by itself.
    pub seconds: Option<f32>,
}

impl Document {
    /// How each page is shown in a presentation.
    pub fn page_shows(&self) -> Vec<PageShow> {
        self.pages()
            .iter()
            .map(|r| {
                let Some(Object::Dict(page)) = self.get(*r) else {
                    return PageShow::default();
                };
                let seconds = self
                    .resolve_key(&page, "Dur")
                    .as_number()
                    .map(|s| s as f32)
                    .filter(|s| s.is_finite() && *s >= 0.0);
                let transition = match self.resolve_key(&page, "Trans") {
                    Object::Dict(t) => Some(self.transition(&t)),
                    _ => None,
                };
                PageShow {
                    transition,
                    seconds,
                }
            })
            .collect()
    }

    fn transition(&self, t: &crate::object::Dict) -> Transition {
        let style = match t.name("S").unwrap_or(b"R") {
            b"Split" => TransitionStyle::Split,
            b"Blinds" => TransitionStyle::Blinds,
            b"Box" => TransitionStyle::Box,
            b"Wipe" => TransitionStyle::Wipe,
            b"Dissolve" => TransitionStyle::Dissolve,
            b"Glitter" => TransitionStyle::Glitter,
            b"Fly" => TransitionStyle::Fly,
            b"Push" => TransitionStyle::Push,
            b"Cover" => TransitionStyle::Cover,
            b"Uncover" => TransitionStyle::Uncover,
            b"Fade" => TransitionStyle::Fade,
            _ => TransitionStyle::Replace,
        };
        let seconds = self
            .resolve_key(t, "D")
            .as_number()
            .map(|s| s as f32)
            .filter(|s| s.is_finite() && *s >= 0.0)
            .unwrap_or(1.0)
            .min(10.0);
        let direction = match self.resolve_key(t, "Di") {
            Object::Name(n) if n.as_bytes() == b"None" => None,
            other => Some(
                other
                    .as_number()
                    .map_or(0.0, |d| d as f32)
                    .rem_euclid(360.0),
            ),
        };
        Transition {
            style,
            seconds,
            horizontal: t.name("Dm") != Some(b"V"),
            outward: t.name("M") != Some(b"I"),
            direction,
        }
    }
}
