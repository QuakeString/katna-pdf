// SPDX-License-Identifier: GPL-3.0-or-later

//! A form's data on its own (docs/PLAN.md §4.3, "import and export form
//! data"): each field's value by its full name, as JSON (one flat
//! object) or CSV (a row of names and a row of values), and filled back
//! in from either. A check box is on with its own "on" state, `true` or
//! `yes`, and off with `false`, `no`, `Off` or nothing; a radio group
//! takes the name of the button to turn on.

use crate::{Document, FieldKind, FieldValue, UpdateError};

/// Why form data could not be read or filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormDataError {
    /// The JSON or CSV is not as expected; what was wrong, and where.
    Syntax(String),
    /// No field has this name.
    NoField(String),
    /// A value a check box, radio group or choice cannot take.
    BadValue {
        field: String,
        value: String,
    },
    Update(UpdateError),
}

impl std::fmt::Display for FormDataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormDataError::Syntax(e) => f.write_str(e),
            FormDataError::NoField(name) => write!(f, "no field is named {name}"),
            FormDataError::BadValue { field, value } => {
                write!(f, "{field} cannot be {value:?}")
            }
            FormDataError::Update(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FormDataError {}

impl Document {
    /// Each field that holds a value people fill in, by its full name,
    /// with its value (empty when it has none; `Off` for a check box or
    /// radio group that is off). Buttons and signatures are left out.
    pub fn form_data(&self) -> Vec<(String, String)> {
        self.form()
            .into_iter()
            .filter(|f| !matches!(f.kind, FieldKind::PushButton | FieldKind::Signature))
            .map(|f| {
                let off = matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio);
                let value = match f.value {
                    Some(v) => v,
                    None if off => "Off".into(),
                    None => String::new(),
                };
                (f.name, value)
            })
            .collect()
    }

    /// The file with `data` (full names and values) filled in, as an
    /// incremental update.
    pub fn fill_data(&self, data: &[(String, String)]) -> Result<Vec<u8>, FormDataError> {
        let fields = self.form();
        let mut changes = Vec::new();
        for (name, value) in data {
            let field = fields
                .iter()
                .find(|f| f.name == *name)
                .ok_or_else(|| FormDataError::NoField(name.clone()))?;
            let bad = || FormDataError::BadValue {
                field: name.clone(),
                value: value.clone(),
            };
            let states: Vec<&str> = field
                .widgets
                .iter()
                .filter_map(|w| w.on.as_deref())
                .collect();
            let off = matches!(
                value.to_ascii_lowercase().as_str(),
                "" | "off" | "false" | "no" | "0"
            );
            let change = match &field.kind {
                FieldKind::PushButton | FieldKind::Signature => continue,
                // A button's own choice first: a radio group may have one
                // named "No".
                FieldKind::CheckBox | FieldKind::Radio if states.contains(&value.as_str()) => {
                    FieldValue::State(Some(value.clone()))
                }
                FieldKind::CheckBox | FieldKind::Radio if off => FieldValue::State(None),
                FieldKind::CheckBox | FieldKind::Radio => {
                    let on = states.iter().find(|s| **s == value.as_str()).or_else(|| {
                        let yes = matches!(
                            value.to_ascii_lowercase().as_str(),
                            "true" | "yes" | "on" | "1"
                        );
                        (yes && field.kind == FieldKind::CheckBox)
                            .then(|| states.first())
                            .flatten()
                    });
                    FieldValue::State(Some(on.ok_or_else(bad)?.to_string()))
                }
                FieldKind::Choice {
                    options,
                    combo: false,
                } if !value.is_empty() && !options.contains(value) => return Err(bad()),
                _ => FieldValue::Text(value.clone()),
            };
            changes.push((field.id, change));
        }
        self.fill(&changes).map_err(FormDataError::Update)
    }
}

/// `data` as one flat JSON object.
pub fn data_to_json(data: &[(String, String)]) -> String {
    let mut out = String::from("{");
    for (i, (name, value)) in data.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("\n  ");
        json_string(&mut out, name);
        out.push_str(": ");
        json_string(&mut out, value);
    }
    out.push_str(if data.is_empty() { "}\n" } else { "\n}\n" });
    out
}

/// `data` as CSV: the names in the first row, the values in the second.
pub fn data_to_csv(data: &[(String, String)]) -> String {
    let row = |cells: Vec<&str>| {
        cells
            .iter()
            .map(|c| {
                if c.contains([',', '"', '\n', '\r']) {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    (*c).to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "{}\r\n{}\r\n",
        row(data.iter().map(|(n, _)| n.as_str()).collect()),
        row(data.iter().map(|(_, v)| v.as_str()).collect())
    )
}

/// Names and values from one flat JSON object; `true`, `false`, numbers
/// and `null` (an empty value) are taken as text.
pub fn data_from_json(text: &str) -> Result<Vec<(String, String)>, FormDataError> {
    let mut p = Json {
        chars: text.chars().collect(),
        at: 0,
    };
    p.space();
    p.expect('{')?;
    let mut out = Vec::new();
    p.space();
    if p.peek() == Some('}') {
        p.at += 1;
    } else {
        loop {
            p.space();
            let name = p.string()?;
            p.space();
            p.expect(':')?;
            p.space();
            let value = p.value()?;
            out.push((name, value));
            p.space();
            match p.next() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err(p.error("',' or '}'")),
            }
        }
    }
    p.space();
    if p.at < p.chars.len() {
        return Err(p.error("the end"));
    }
    Ok(out)
}

/// Names and values from CSV: names in the first row, values in the
/// second (only the first row of values is used).
pub fn data_from_csv(text: &str) -> Result<Vec<(String, String)>, FormDataError> {
    let rows = csv_rows(text.trim_start_matches('\u{feff}'))?;
    let mut rows = rows.into_iter().filter(|r| r.iter().any(|c| !c.is_empty()));
    let names = rows
        .next()
        .ok_or_else(|| FormDataError::Syntax("the CSV has no rows".into()))?;
    let values = rows.next().unwrap_or_default();
    Ok(names
        .into_iter()
        .enumerate()
        .filter(|(_, n)| !n.is_empty())
        .map(|(i, n)| (n, values.get(i).cloned().unwrap_or_default()))
        .collect())
}

fn csv_rows(text: &str) -> Result<Vec<Vec<String>>, FormDataError> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                cell.push('"');
            }
            ('"', true) => quoted = false,
            ('"', false) if cell.is_empty() => quoted = true,
            (',', false) => row.push(std::mem::take(&mut cell)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => cell.push(c),
        }
    }
    if quoted {
        return Err(FormDataError::Syntax(
            "a quoted CSV cell is not closed".into(),
        ));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}

fn json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Json {
    chars: Vec<char>,
    at: usize,
}

impl Json {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        self.at += 1;
        c
    }

    fn space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    fn error(&self, wanted: &str) -> FormDataError {
        FormDataError::Syntax(format!(
            "the JSON should have {wanted} at character {}",
            self.at + 1
        ))
    }

    fn expect(&mut self, c: char) -> Result<(), FormDataError> {
        if self.next() == Some(c) {
            Ok(())
        } else {
            self.at -= 1;
            Err(self.error(&format!("'{c}'")))
        }
    }

    fn string(&mut self) -> Result<String, FormDataError> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.next().ok_or_else(|| self.error("'\"'"))? {
                '"' => return Ok(out),
                '\\' => match self.next().ok_or_else(|| self.error("an escape"))? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let mut unit = self.hex()?;
                        // A surrogate pair.
                        if (0xd800..0xdc00).contains(&unit)
                            && self.chars.get(self.at..self.at + 2) == Some(&['\\', 'u'])
                        {
                            self.at += 2;
                            let low = self.hex()?;
                            unit = 0x10000
                                + ((unit - 0xd800) << 10)
                                + (low.wrapping_sub(0xdc00) & 0x3ff);
                        }
                        out.push(char::from_u32(unit).unwrap_or('\u{fffd}'));
                    }
                    c => out.push(c),
                },
                c => out.push(c),
            }
        }
    }

    fn hex(&mut self) -> Result<u32, FormDataError> {
        let digits: String = self
            .chars
            .get(self.at..self.at + 4)
            .unwrap_or(&[])
            .iter()
            .collect();
        let value = u32::from_str_radix(&digits, 16).map_err(|_| self.error("four hex digits"))?;
        self.at += 4;
        Ok(value)
    }

    fn value(&mut self) -> Result<String, FormDataError> {
        match self.peek() {
            Some('"') => self.string(),
            Some(c) if c == '-' || c.is_ascii_alphanumeric() => {
                let start = self.at;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                {
                    self.at += 1;
                }
                let word: String = self.chars[start..self.at].iter().collect();
                match word.as_str() {
                    "null" => Ok(String::new()),
                    "true" | "false" => Ok(word),
                    w if w.parse::<f64>().is_ok() => Ok(word),
                    _ => {
                        self.at = start;
                        Err(self.error("a string, number, true, false or null"))
                    }
                }
            }
            _ => Err(self.error("a string, number, true, false or null")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_csv_round_trip() {
        let data = vec![
            ("Name".to_string(), "Asha \"A\" Rao".to_string()),
            ("Address".into(), "12, MG Road\nPune".into()),
            ("Agree".into(), "Off".into()),
            ("नाम".into(), "आशा".into()),
        ];
        assert_eq!(data_from_json(&data_to_json(&data)).unwrap(), data);
        assert_eq!(data_from_csv(&data_to_csv(&data)).unwrap(), data);
        assert_eq!(data_from_json("{}").unwrap(), vec![]);
        assert_eq!(
            data_from_json(r#"{"a": true, "b": null, "c": -1.5, "d": "A😀"}"#).unwrap(),
            vec![
                ("a".into(), "true".into()),
                ("b".into(), String::new()),
                ("c".into(), "-1.5".into()),
                ("d".into(), "A😀".into()),
            ]
        );
        assert!(data_from_json(r#"{"a": [1]}"#).is_err());
        assert!(data_from_json(r#"{"a": "b"} x"#).is_err());
        assert!(data_from_csv("a,\"b\nc").is_err());
    }
}
