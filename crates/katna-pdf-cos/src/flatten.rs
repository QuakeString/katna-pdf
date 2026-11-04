// SPDX-License-Identifier: GPL-3.0-or-later

//! Flattening: a form's fields and the comments drawn into the pages
//! themselves, as they look, so they can no longer be filled in or
//! changed, and the form and annotations taken out.

use crate::insert::PageAdd;
use crate::object::{Dict, Object};
use crate::{Document, EditError};

/// What [`Document::flatten`] draws into the pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flatten {
    /// The form's fields, and the form itself taken out.
    pub forms: bool,
    /// The other annotations: comments, stamps, drawings, and the rest.
    /// Links stay, since they draw nothing and still work.
    pub comments: bool,
}

/// Hidden, or not shown on screen.
const HIDDEN: i64 = 2;
const NO_VIEW: i64 = 32;

impl Document {
    /// The file with its form's fields and its comments drawn into the
    /// pages, as they look, and taken out as fields and annotations.
    pub fn flatten(&self, what: Flatten) -> Result<Vec<u8>, EditError> {
        let mut update = self.update()?;
        let mut widgets_left = false;
        for page in 0..self.pages().len() {
            let page_ref = self.pages()[page];
            let Some(Object::Dict(dict)) = self.get(page_ref) else {
                continue;
            };
            let Object::Array(annots) = self.resolve_key(&dict, "Annots") else {
                continue;
            };
            let mut keep = Vec::new();
            let mut drawn: Vec<(Dict, Object)> = Vec::new();
            for annot in &annots {
                let Object::Dict(a) = self.resolve(annot) else {
                    continue;
                };
                let subtype = a.name("Subtype").unwrap_or(b"");
                let widget = subtype == b"Widget";
                let mine = match subtype {
                    b"Widget" => what.forms,
                    b"Link" => false,
                    _ => what.comments,
                };
                if !mine {
                    if widget {
                        widgets_left = true;
                    }
                    keep.push(annot.clone());
                    continue;
                }
                let flags = self.resolve_key(&a, "F").as_int().unwrap_or(0);
                // A comment's pop-up window is not part of the page.
                if subtype != b"Popup"
                    && flags & (HIDDEN | NO_VIEW) == 0
                    && let Some(stream) = self.normal_appearance(&a)
                {
                    drawn.push((a.clone(), stream));
                }
            }
            if drawn.is_empty() && keep.len() == annots.len() {
                continue;
            }
            let mut add = PageAdd::new(self, page)?;
            let mut content = Vec::new();
            for (a, stream) in drawn {
                let Some(rect) = self.rect_of(&a) else {
                    continue;
                };
                let Object::Stream(s) = &self.resolve(&stream) else {
                    continue;
                };
                let Some(matrix) = self.placing(&s.dict, rect) else {
                    continue;
                };
                let r = match stream {
                    Object::Ref(r) => r,
                    other => update.add(other),
                };
                let key = add.xobject(r);
                content.extend_from_slice(
                    format!(
                        "q {} cm /{key} Do Q\n",
                        matrix
                            .iter()
                            .map(|v| format!("{v:.4}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .as_bytes(),
                );
            }
            add.finish(&mut update, &content, false);
            // `finish` wrote the page; its annotations change too.
            let mut dict = update
                .get(page_ref)
                .and_then(|o| o.as_dict().cloned())
                .unwrap_or(dict);
            if keep.is_empty() {
                dict.remove("Annots");
            } else {
                dict.set("Annots", Object::Array(keep));
            }
            update.replace(page_ref, Object::Dict(dict));
        }
        if what.forms
            && !widgets_left
            && let Some(Object::Ref(root)) = self.trailer().get("Root")
            && let Some(Object::Dict(mut catalog)) = self.get(*root)
            && catalog.get("AcroForm").is_some()
        {
            catalog.remove("AcroForm");
            update.replace(*root, Object::Dict(catalog));
        }
        Ok(update.finish())
    }

    /// The annotation's normal appearance stream, as a reference where
    /// it is one, for the state it is in.
    fn normal_appearance(&self, a: &Dict) -> Option<Object> {
        let Object::Dict(ap) = self.resolve_key(a, "AP") else {
            return None;
        };
        let n = ap.get("N")?;
        let pick = |o: &Object| match self.resolve(o) {
            Object::Stream(_) => Some(o.clone()),
            _ => None,
        };
        match self.resolve(n) {
            Object::Stream(_) => pick(n),
            Object::Dict(states) => {
                let state = a.name("AS").map(<[u8]>::to_vec);
                let entry = match state {
                    Some(state) => states.0.iter().find(|(k, _)| k.as_bytes() == state),
                    None if states.0.len() == 1 => states.0.first(),
                    None => None,
                }?;
                pick(&entry.1)
            }
            _ => None,
        }
    }

    fn rect_of(&self, a: &Dict) -> Option<[f64; 4]> {
        let Object::Array(rect) = self.resolve_key(a, "Rect") else {
            return None;
        };
        let v: Vec<f64> = rect
            .iter()
            .filter_map(|n| self.resolve(n).as_number())
            .collect();
        let [x0, y0, x1, y1] = v[..] else {
            return None;
        };
        Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
    }

    /// The matrix that puts a form XObject with `dict`'s `/BBox` and
    /// `/Matrix` into `rect` (§12.5.5).
    fn placing(&self, dict: &Dict, rect: [f64; 4]) -> Option<[f64; 6]> {
        let numbers = |key: &str| -> Option<Vec<f64>> {
            match self.resolve_key(dict, key) {
                Object::Array(a) => Some(
                    a.iter()
                        .filter_map(|n| self.resolve(n).as_number())
                        .collect(),
                ),
                _ => None,
            }
        };
        let bbox = numbers("BBox")?;
        let [bx0, by0, bx1, by1] = bbox[..] else {
            return None;
        };
        let m = match numbers("Matrix") {
            Some(m) if m.len() == 6 => [m[0], m[1], m[2], m[3], m[4], m[5]],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        };
        let apply = |x: f64, y: f64| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let corners = [
            apply(bx0, by0),
            apply(bx1, by0),
            apply(bx1, by1),
            apply(bx0, by1),
        ];
        let (mut tx0, mut ty0, mut tx1, mut ty1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in corners {
            tx0 = tx0.min(x);
            ty0 = ty0.min(y);
            tx1 = tx1.max(x);
            ty1 = ty1.max(y);
        }
        let sx = if tx1 - tx0 > 1e-9 {
            (rect[2] - rect[0]) / (tx1 - tx0)
        } else {
            1.0
        };
        let sy = if ty1 - ty0 > 1e-9 {
            (rect[3] - rect[1]) / (ty1 - ty0)
        } else {
            1.0
        };
        Some([sx, 0.0, 0.0, sy, rect[0] - tx0 * sx, rect[1] - ty0 * sy])
    }
}
