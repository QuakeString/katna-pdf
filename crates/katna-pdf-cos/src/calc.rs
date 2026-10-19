// SPDX-License-Identifier: GPL-3.0-or-later

//! Form totals and formats without JavaScript (docs/PLAN.md §4.8):
//! the calls Acrobat's form tools write into fields' actions
//! (`AFSimple_Calculate`, `AFNumber_Format`, `AFPercent_Format`,
//! `AFDate_FormatEx`, `AFDate_Format`, `AFSpecial_Format`) are read and
//! done in Rust, so Indian government forms add up and show amounts and
//! dates as they mean to. Other scripts are left alone.

use std::collections::HashMap;

use crate::{Dict, Document, Field, Object, Ref};

/// A field's value worked out from others (its `/AA /C` action).
#[derive(Debug, Clone, PartialEq)]
pub enum Calculation {
    /// `SUM`, `PRD`, `AVG`, `MIN` or `MAX` of the fields named (full
    /// names; a name with a trailing dot takes all its kids).
    Simple { op: CalcOp, fields: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcOp {
    Sum,
    Product,
    Average,
    Min,
    Max,
}

/// How a field's value is shown (its `/AA /F` action).
#[derive(Debug, Clone, PartialEq)]
pub enum Format {
    Number {
        decimals: usize,
        /// 0: 1,234.56; 1: 1234.56; 2: 1.234,56; 3: 1234,56; 4: 1'234.56.
        separators: u8,
        /// 0: -1; 1: red; 2: (1); 3: red (1). Shown without colour here.
        negative: u8,
        currency: String,
        currency_first: bool,
    },
    Percent {
        decimals: usize,
        separators: u8,
    },
    /// A date written as `dd/mm/yyyy` and the like.
    Date(String),
    /// 0: ZIP code; 1: ZIP+4; 2: phone number; 3: social security
    /// number.
    Special(u8),
}

/// What a field's scripts do that we can.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scripts {
    pub calculate: Option<Calculation>,
    pub format: Option<Format>,
}

impl Document {
    /// The calculations and formats of `fields` that we understand, by
    /// field.
    pub fn form_scripts(&self, fields: &[Field]) -> HashMap<Ref, Scripts> {
        let mut out = HashMap::new();
        for field in fields {
            let Some(Object::Dict(dict)) = self.get(field.id) else {
                continue;
            };
            let Object::Dict(aa) = self.resolve_key(&dict, "AA") else {
                continue;
            };
            let scripts = Scripts {
                calculate: self.script(&aa, "C").and_then(|s| calculation(&s)),
                format: self.script(&aa, "F").and_then(|s| format(&s)),
            };
            if scripts != Scripts::default() {
                out.insert(field.id, scripts);
            }
        }
        out
    }

    /// The JavaScript of action `key` in `aa`.
    fn script(&self, aa: &Dict, key: &str) -> Option<String> {
        let Object::Dict(action) = self.resolve_key(aa, key) else {
            return None;
        };
        match self.resolve_key(&action, "JS") {
            Object::String { bytes, .. } => Some(crate::outline::text_value(&bytes)),
            Object::Stream(s) => crate::decode(&s.dict, &s.data)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
            _ => None,
        }
    }

    /// The order fields are worked out in: the form's `/CO`, then any
    /// others with a calculation.
    pub(crate) fn calculation_order(&self, scripts: &HashMap<Ref, Scripts>) -> Vec<Ref> {
        let mut order: Vec<Ref> = match self.acro_form().map(|f| self.resolve_key(&f, "CO")) {
            Some(Object::Array(co)) => co.iter().filter_map(Object::as_ref).collect(),
            _ => Vec::new(),
        };
        order.retain(|r| scripts.get(r).is_some_and(|s| s.calculate.is_some()));
        let mut rest: Vec<Ref> = scripts
            .iter()
            .filter(|(r, s)| s.calculate.is_some() && !order.contains(r))
            .map(|(r, _)| *r)
            .collect();
        rest.sort_unstable_by_key(|r| r.num);
        order.extend(rest);
        order
    }
}

/// Works out `calc` from `values` (full names to values as typed).
pub fn calculate(calc: &Calculation, values: &HashMap<String, String>) -> String {
    let Calculation::Simple { op, fields } = calc;
    let mut numbers = Vec::new();
    for name in fields {
        if let Some(prefix) = name.strip_suffix('.') {
            let mut kids: Vec<(&String, &String)> = values
                .iter()
                .filter(|(n, _)| n.starts_with(&format!("{prefix}.")))
                .collect();
            kids.sort();
            numbers.extend(kids.into_iter().map(|(_, v)| number(v)));
        } else {
            numbers.push(values.get(name).map_or(0.0, |v| number(v)));
        }
    }
    let result = match op {
        CalcOp::Sum => numbers.iter().sum(),
        CalcOp::Product if numbers.is_empty() => 0.0,
        CalcOp::Product => numbers.iter().product(),
        CalcOp::Average if numbers.is_empty() => 0.0,
        CalcOp::Average => numbers.iter().sum::<f64>() / numbers.len() as f64,
        CalcOp::Min => numbers.iter().copied().fold(f64::INFINITY, f64::min),
        CalcOp::Max => numbers.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    };
    if !result.is_finite() {
        return String::new();
    }
    plain_number(result)
}

/// A number as JavaScript writes it: no trailing zeros, no exponent for
/// everyday sizes.
fn plain_number(n: f64) -> String {
    let rounded = (n * 1e9).round() / 1e9;
    let mut s = format!("{rounded:.9}");
    while s.contains('.') && (s.ends_with('0') || s.ends_with('.')) {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

/// The number in a value as people type it: currency signs, spaces and
/// thousands separators dropped; a comma taken as the decimal point
/// when it is the only separator and two or fewer digits follow it.
pub fn number(value: &str) -> f64 {
    let negative = value.contains('-') || (value.contains('(') && value.contains(')'));
    let kept: String = value
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let decimal_comma = !kept.contains('.')
        && kept.rsplit_once(',').is_some_and(|(_, after)| {
            (1..=2).contains(&after.len()) && kept.matches(',').count() == 1
        });
    let normal: String = if decimal_comma {
        kept.replace(',', ".")
    } else {
        kept.replace(',', "")
    };
    let n = normal.parse::<f64>().unwrap_or(0.0);
    if negative { -n } else { n }
}

/// `value` as `format` shows it; a value that does not fit the format is
/// shown as it is.
pub fn show(format: &Format, value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }
    match format {
        Format::Number {
            decimals,
            separators,
            negative,
            currency,
            currency_first,
        } => {
            let n = number(value);
            let digits = group(n.abs(), *decimals, *separators);
            let body = match (currency.is_empty(), currency_first) {
                (true, _) => digits.clone(),
                (false, true) => format!("{currency}{digits}"),
                (false, false) => format!("{digits}{currency}"),
            };
            if n < 0.0 && digits.chars().any(|c| c.is_ascii_digit() && c != '0') {
                match negative {
                    2 | 3 => format!("({body})"),
                    _ => format!("-{body}"),
                }
            } else {
                body
            }
        }
        Format::Percent {
            decimals,
            separators,
        } => {
            let n = number(value) * 100.0;
            let sign = if n < 0.0 { "-" } else { "" };
            format!("{sign}{}%", group(n.abs(), *decimals, *separators))
        }
        Format::Date(pattern) => match parse_date(value, pattern) {
            Some(date) => write_date(date, pattern),
            None => value.to_owned(),
        },
        Format::Special(kind) => {
            let digits: String = value.chars().filter(char::is_ascii_digit).collect();
            match (kind, digits.len()) {
                (0, 5) => digits,
                (1, 9) => format!("{}-{}", &digits[..5], &digits[5..]),
                (2, 10) => format!("({}) {}-{}", &digits[..3], &digits[3..6], &digits[6..]),
                (2, 7) => format!("{}-{}", &digits[..3], &digits[3..]),
                (3, 9) => format!("{}-{}-{}", &digits[..3], &digits[3..5], &digits[5..]),
                _ => value.to_owned(),
            }
        }
    }
}

/// `n` with `decimals` places, grouped in thousands as `style` says.
fn group(n: f64, decimals: usize, style: u8) -> String {
    let fixed = format!("{n:.decimals$}");
    let (whole, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let (thousands, point) = match style {
        1 => ("", "."),
        2 => (".", ","),
        3 => ("", ","),
        4 => ("'", "."),
        _ => (",", "."),
    };
    let mut grouped = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            grouped.push_str(thousands);
        }
        grouped.push(c);
    }
    if fraction.is_empty() {
        grouped
    } else {
        format!("{grouped}{point}{fraction}")
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A day: year, month (1–12), day, hour, minute.
type Date = (i32, u32, u32, u32, u32);

/// The tokens of a date pattern, longest first.
const TOKENS: [&str; 14] = [
    "yyyy", "mmmm", "dddd", "mmm", "ddd", "yy", "mm", "dd", "HH", "MM", "hh", "tt", "m", "d",
];

/// A date typed as `pattern` says, or as `yyyy-mm-dd`; the year may be
/// written with two digits.
pub fn parse_date(value: &str, pattern: &str) -> Option<Date> {
    let value = value.trim();
    if let Some(date) = by_pattern(value, pattern) {
        return Some(date);
    }
    by_pattern(value, "yyyy-mm-dd")
}

fn by_pattern(value: &str, pattern: &str) -> Option<Date> {
    let (mut y, mut mo, mut d, mut h, mut mi) = (None, None, Some(1), 0, 0);
    let mut pm = None;
    let mut v = value;
    let mut p = pattern;
    while !p.is_empty() {
        let token = TOKENS.iter().find(|t| p.starts_with(**t));
        match token {
            Some(&t) => {
                p = &p[t.len()..];
                match t {
                    "mmmm" | "mmm" => {
                        let (i, len) = MONTHS.iter().enumerate().find_map(|(i, m)| {
                            let lower = v.to_lowercase();
                            if lower.starts_with(&m.to_lowercase()) {
                                Some((i, m.len()))
                            } else if lower.starts_with(&m[..3].to_lowercase()) {
                                Some((i, 3))
                            } else {
                                None
                            }
                        })?;
                        mo = Some(i as u32 + 1);
                        v = &v[len..];
                    }
                    "dddd" | "ddd" => {
                        let len = v.chars().take_while(|c| c.is_alphabetic()).count();
                        v = &v[len..];
                    }
                    "tt" => {
                        let lower = v.to_ascii_lowercase();
                        pm = Some(lower.starts_with("pm"));
                        v = v.get(2..)?;
                    }
                    _ => {
                        let most = if t == "yyyy" { 4 } else { 2 };
                        let len = v
                            .chars()
                            .take(most)
                            .take_while(char::is_ascii_digit)
                            .count();
                        if len == 0 {
                            return None;
                        }
                        let n: u32 = v[..len].parse().ok()?;
                        v = &v[len..];
                        match t {
                            "yyyy" | "yy" => {
                                y = Some(if len <= 2 { 2000 + n as i32 } else { n as i32 });
                            }
                            "mm" | "m" => mo = Some(n),
                            "dd" | "d" => d = Some(n),
                            "HH" | "hh" => h = n,
                            "MM" => mi = n,
                            _ => {}
                        }
                    }
                }
            }
            None => {
                let c = p.chars().next()?;
                p = &p[c.len_utf8()..];
                // Any one separator stands for another.
                let got = v.chars().next()?;
                if got.is_alphanumeric() {
                    return None;
                }
                v = &v[got.len_utf8()..];
            }
        }
    }
    if !v.trim().is_empty() {
        return None;
    }
    if let Some(pm) = pm {
        h = h % 12 + if pm { 12 } else { 0 };
    }
    let (y, mo, d) = (y?, mo?, d?);
    if !(1..=12).contains(&mo) || d == 0 || d > days_in(y, mo) || h > 23 || mi > 59 {
        return None;
    }
    Some((y, mo, d, h, mi))
}

fn days_in(y: i32, m: u32) -> u32 {
    match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The day of the week, 0 for Sunday.
fn weekday(y: i32, m: u32, d: u32) -> usize {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[m as usize - 1] + d as i32).rem_euclid(7)) as usize
}

/// `date` written as `pattern` says.
pub fn write_date((y, mo, d, h, mi): Date, pattern: &str) -> String {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let mut out = String::new();
    let mut p = pattern;
    while !p.is_empty() {
        match TOKENS.iter().find(|t| p.starts_with(**t)) {
            Some(&t) => {
                p = &p[t.len()..];
                out.push_str(&match t {
                    "yyyy" => format!("{y:04}"),
                    "yy" => format!("{:02}", y.rem_euclid(100)),
                    "mmmm" => MONTHS[mo as usize - 1].to_string(),
                    "mmm" => MONTHS[mo as usize - 1][..3].to_string(),
                    "mm" => format!("{mo:02}"),
                    "m" => mo.to_string(),
                    "dddd" => DAYS[weekday(y, mo, d)].to_string(),
                    "ddd" => DAYS[weekday(y, mo, d)][..3].to_string(),
                    "dd" => format!("{d:02}"),
                    "d" => d.to_string(),
                    "HH" => format!("{h:02}"),
                    "hh" => format!("{:02}", if h % 12 == 0 { 12 } else { h % 12 }),
                    "MM" => format!("{mi:02}"),
                    "tt" => (if h < 12 { "am" } else { "pm" }).to_string(),
                    _ => String::new(),
                });
            }
            None => {
                let c = p.chars().next().unwrap_or_default();
                p = &p[c.len_utf8()..];
                out.push(c);
            }
        }
    }
    out
}

/// The `AFDate_Format` patterns, by number.
const DATE_FORMATS: [&str; 15] = [
    "m/d",
    "m/d/yy",
    "mm/dd/yy",
    "mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "dd-mmm-yy",
    "yy-mm-dd",
    "mmm-yy",
    "mmmm-yy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "m/d/yy h:MM tt",
    "m/d/yy HH:MM",
    "dd/mm/yyyy",
];

/// The arguments of the first call to `name` in `script`: strings,
/// numbers and words, an array's items flattened in.
fn call(script: &str, name: &str) -> Option<Vec<String>> {
    let at = script.find(name)?;
    let rest = &script[at + name.len()..];
    let open = rest.find('(')?;
    if !rest[..open].trim().is_empty() {
        return None;
    }
    let mut args = Vec::new();
    let mut chars = rest[open + 1..].chars().peekable();
    let mut word = String::new();
    let mut depth = 0;
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                let mut s = String::new();
                while let Some(d) = chars.next() {
                    match d {
                        '\\' => {
                            if let Some(e) = chars.next() {
                                s.push(match e {
                                    'n' => '\n',
                                    't' => '\t',
                                    e => e,
                                });
                            }
                        }
                        d if d == c => break,
                        d => s.push(d),
                    }
                }
                args.push(s);
            }
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            ')' => break,
            ',' | ' ' | '\n' | '\r' | '\t' => {
                if !word.is_empty() {
                    let w = std::mem::take(&mut word);
                    if !matches!(w.as_str(), "new" | "Array") {
                        args.push(w);
                    }
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() && !matches!(word.as_str(), "new" | "Array") {
        args.push(word);
    }
    Some(args)
}

/// A calculation script we can do ourselves.
pub fn calculation(script: &str) -> Option<Calculation> {
    let args = call(script, "AFSimple_Calculate")?;
    let (op, names) = args.split_first()?;
    let op = match op.to_ascii_uppercase().as_str() {
        "SUM" => CalcOp::Sum,
        "PRD" => CalcOp::Product,
        "AVG" => CalcOp::Average,
        "MIN" => CalcOp::Min,
        "MAX" => CalcOp::Max,
        _ => return None,
    };
    // Names may come as one string, separated by commas.
    let fields = names
        .iter()
        .flat_map(|n| n.split(','))
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect();
    Some(Calculation::Simple { op, fields })
}

/// A format script we can do ourselves.
pub fn format(script: &str) -> Option<Format> {
    let int = |s: Option<&String>| s.and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    if let Some(a) = call(script, "AFNumber_Format") {
        return Some(Format::Number {
            decimals: int(a.first()).clamp(0, 10) as usize,
            separators: int(a.get(1)).clamp(0, 4) as u8,
            negative: int(a.get(2)).clamp(0, 3) as u8,
            currency: a.get(4).cloned().unwrap_or_default(),
            currency_first: a.get(5).is_none_or(|b| b.trim() == "true"),
        });
    }
    if let Some(a) = call(script, "AFPercent_Format") {
        return Some(Format::Percent {
            decimals: int(a.first()).clamp(0, 10) as usize,
            separators: int(a.get(1)).clamp(0, 4) as u8,
        });
    }
    if let Some(a) = call(script, "AFDate_FormatEx") {
        return a.first().map(|p| Format::Date(p.clone()));
    }
    if let Some(a) = call(script, "AFDate_Format") {
        let i = int(a.first()).clamp(0, DATE_FORMATS.len() as i64 - 1) as usize;
        return Some(Format::Date(DATE_FORMATS[i].into()));
    }
    if let Some(a) = call(script, "AFSpecial_Format") {
        return Some(Format::Special(int(a.first()).clamp(0, 3) as u8));
    }
    None
}

/// The scripts that make a field calculate or show itself as asked:
/// `/AA` entries `C`, `F` and `K`, for the forms designer.
pub fn script_text(
    calc: Option<&Calculation>,
    format: Option<&Format>,
) -> [Option<(String, String)>; 3] {
    let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let c = calc.map(|Calculation::Simple { op, fields }| {
        let op = match op {
            CalcOp::Sum => "SUM",
            CalcOp::Product => "PRD",
            CalcOp::Average => "AVG",
            CalcOp::Min => "MIN",
            CalcOp::Max => "MAX",
        };
        let names: Vec<String> = fields.iter().map(|f| quote(f)).collect();
        (
            "C".to_string(),
            format!(
                "AFSimple_Calculate({}, new Array ({}));",
                quote(op),
                names.join(", ")
            ),
        )
    });
    let (f, k) = match format {
        Some(Format::Number {
            decimals,
            separators,
            negative,
            currency,
            currency_first,
        }) => {
            let args = format!(
                "{decimals}, {separators}, {negative}, 0, {}, {currency_first}",
                quote(currency)
            );
            (
                Some(format!("AFNumber_Format({args});")),
                Some(format!("AFNumber_Keystroke({args});")),
            )
        }
        Some(Format::Percent {
            decimals,
            separators,
        }) => (
            Some(format!("AFPercent_Format({decimals}, {separators});")),
            Some(format!("AFPercent_Keystroke({decimals}, {separators});")),
        ),
        Some(Format::Date(pattern)) => (
            Some(format!("AFDate_FormatEx({});", quote(pattern))),
            Some(format!("AFDate_KeystrokeEx({});", quote(pattern))),
        ),
        Some(Format::Special(kind)) => (
            Some(format!("AFSpecial_Format({kind});")),
            Some(format!("AFSpecial_Keystroke({kind});")),
        ),
        None => (None, None),
    };
    [
        c,
        f.map(|f| ("F".to_string(), f)),
        k.map(|k| ("K".to_string(), k)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculations_are_read_and_done() {
        let calc =
            calculation("AFSimple_Calculate(\"SUM\", new Array (\"Fee\", \"Tax\", \"Rows.\"));")
                .unwrap();
        let values: HashMap<String, String> = [
            ("Fee", "₹1,250.50"),
            ("Tax", "225"),
            ("Rows.0", "10"),
            ("Rows.1", "(4)"),
            ("Other", "99"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(calculate(&calc, &values), "1481.5");
        let avg = calculation("AFSimple_Calculate('AVG', 'Fee, Tax');").unwrap();
        assert_eq!(calculate(&avg, &values), "737.75");
        assert_eq!(calculation("event.value = 1;"), None);
    }

    #[test]
    fn numbers_and_dates_are_shown_as_asked() {
        let rupees = format("AFNumber_Format(2, 0, 2, 0, \"₹ \", true);").unwrap();
        assert_eq!(show(&rupees, "1234567.5"), "₹ 1,234,567.50");
        assert_eq!(show(&rupees, "-12"), "(₹ 12.00)");
        assert_eq!(show(&rupees, ""), "");
        let euro = format("AFNumber_Format(1, 2, 0, 0, \" €\", false);").unwrap();
        assert_eq!(show(&euro, "1234.56"), "1.234,6 €");
        let pc = format("AFPercent_Format(1, 0);").unwrap();
        assert_eq!(show(&pc, "0.125"), "12.5%");
        let date = format("AFDate_FormatEx(\"dd/mm/yyyy\");").unwrap();
        assert_eq!(show(&date, "2026-10-19"), "19/10/2026");
        assert_eq!(show(&date, "5/1/26"), "05/01/2026");
        assert_eq!(show(&date, "31/02/2026"), "31/02/2026");
        let long = Format::Date("dddd, d mmmm yyyy".into());
        assert_eq!(show(&long, "19/10/2026"), "19/10/2026");
        assert_eq!(show(&long, "2026-10-19"), "Monday, 19 October 2026");
        assert_eq!(
            format("AFDate_Format(14);"),
            Some(Format::Date("dd/mm/yyyy".into()))
        );
        let phone = format("AFSpecial_Format(2);").unwrap();
        assert_eq!(show(&phone, "2025551234"), "(202) 555-1234");
    }

    #[test]
    fn scripts_written_are_read_back() {
        let calc = Calculation::Simple {
            op: CalcOp::Sum,
            fields: vec!["A".into(), "B \"x\"".into()],
        };
        let rupees = Format::Number {
            decimals: 2,
            separators: 0,
            negative: 0,
            currency: "₹".into(),
            currency_first: true,
        };
        let [c, f, k] = script_text(Some(&calc), Some(&rupees));
        assert_eq!(calculation(&c.unwrap().1), Some(calc));
        assert_eq!(format(&f.unwrap().1), Some(rupees));
        assert!(k.unwrap().1.starts_with("AFNumber_Keystroke("));
    }
}
