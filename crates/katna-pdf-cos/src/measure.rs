// SPDX-License-Identifier: GPL-3.0-or-later

//! The scale lengths on a page are measured in (ISO 32000-2 §12.9): a
//! drawing's file may say that a point on paper is so many metres on the
//! ground (a page's `/VP` viewports, each with a `/Measure` dictionary),
//! and measurement annotations carry the scale they were made in.

use crate::document::Document;
use crate::object::{Dict, Name, Object};
use crate::outline::text_string;

/// How long a point on the page is, in `unit`s.
#[derive(Debug, Clone, PartialEq)]
pub struct Scale {
    /// Units per point.
    pub factor: f64,
    /// What the lengths are in: `mm`, `m`, `in`, `ft`…
    pub unit: String,
}

impl Default for Scale {
    /// The paper itself, in millimetres.
    fn default() -> Self {
        Scale {
            factor: 25.4 / 72.0,
            unit: "mm".into(),
        }
    }
}

impl Scale {
    /// A length in points, in this scale's units, as people write it:
    /// `12.35 m`.
    pub fn length(&self, points: f64) -> String {
        format!("{} {}", shown(points * self.factor), self.unit)
    }

    /// An area in square points, in this scale's square units: `4.2 m²`.
    pub fn area(&self, points: f64) -> String {
        format!(
            "{} {}²",
            shown(points * self.factor * self.factor),
            self.unit
        )
    }

    /// The scale set so that `points` on the page are `real` `unit`s.
    pub fn calibrated(points: f64, real: f64, unit: &str) -> Option<Scale> {
        (points > 0.0 && real > 0.0 && real.is_finite()).then(|| Scale {
            factor: real / points,
            unit: unit.to_owned(),
        })
    }

    /// The `/Measure` dictionary for this scale (a rectilinear one).
    pub(crate) fn dict(&self) -> Object {
        let format = |unit: &str, factor: f64| {
            Object::Array(vec![Object::Dict(Dict(vec![
                (Name::new("Type"), Object::name("NumberFormat")),
                (Name::new("U"), Object::string(unit)),
                (Name::new("C"), Object::Real(factor)),
                (Name::new("D"), Object::Int(100)),
            ]))])
        };
        Object::Dict(Dict(vec![
            (Name::new("Type"), Object::name("Measure")),
            (Name::new("Subtype"), Object::name("RL")),
            (
                Name::new("R"),
                Object::string(&format!("1 pt = {} {}", shown(self.factor), self.unit)),
            ),
            (Name::new("X"), format(&self.unit, self.factor)),
            (Name::new("D"), format(&self.unit, 1.0)),
            (Name::new("A"), format(&format!("{}²", self.unit), 1.0)),
        ]))
    }
}

/// A number with as many decimals as its size calls for.
fn shown(v: f64) -> String {
    let digits = match v.abs() {
        a if a >= 100.0 => 1,
        a if a >= 1.0 => 2,
        _ => 3,
    };
    let s = format!("{v:.digits$}");
    // 12.50 → 12.5, 3.00 → 3.
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" || s == "-0" {
        "0".into()
    } else {
        s.to_owned()
    }
}

impl Document {
    /// The scale `measure` (a `/Measure` dictionary, or a reference to
    /// one) gives, when it is a rectilinear one.
    pub(crate) fn read_scale(&self, measure: &Object) -> Option<Scale> {
        let Object::Dict(m) = self.resolve(measure) else {
            return None;
        };
        if m.name("Subtype").is_some_and(|s| s != b"RL") {
            return None;
        }
        let Object::Array(x) = self.resolve_key(&m, "X") else {
            return None;
        };
        // The first format turns points into the first unit; further
        // ones only split it (feet, then inches).
        let Object::Dict(first) = self.resolve(x.first()?) else {
            return None;
        };
        let factor = self.resolve_key(&first, "C").as_number()?;
        let unit = match self.resolve_key(&first, "U") {
            Object::String { bytes, .. } => text_string(&bytes),
            _ => String::new(),
        };
        (factor > 0.0 && factor.is_finite()).then_some(Scale { factor, unit })
    }

    /// The scale page `page` is drawn in, when its file says: the first
    /// of its viewports with a rectilinear measure.
    pub fn page_scale(&self, page: usize) -> Option<Scale> {
        let r = *self.pages().get(page)?;
        let Some(Object::Dict(dict)) = self.get(r) else {
            return None;
        };
        let Object::Array(viewports) = self.resolve_key(&dict, "VP") else {
            return None;
        };
        viewports.iter().find_map(|vp| {
            let Object::Dict(vp) = self.resolve(vp) else {
                return None;
            };
            self.read_scale(vp.get("Measure")?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_read_as_people_write_them() {
        let paper = Scale::default();
        assert_eq!(paper.length(72.0), "25.4 mm");
        let site = Scale::calibrated(100.0, 5.0, "m").unwrap();
        assert_eq!(site.length(250.0), "12.5 m");
        assert_eq!(site.area(100.0 * 100.0), "25 m²");
        assert_eq!(site.length(1.0), "0.05 m");
        assert_eq!(Scale::calibrated(0.0, 5.0, "m"), None);
        assert_eq!(shown(1234.56), "1234.6");
    }
}
