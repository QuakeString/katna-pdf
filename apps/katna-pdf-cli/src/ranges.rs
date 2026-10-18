// SPDX-License-Identifier: GPL-3.0-or-later

//! Page lists as people write them: `1-3,5,8-` (pages counted from 1;
//! an open end runs to the last page, an open start from the first),
//! `even`, `odd` and `all`.

/// The pages `spec` names in a document of `count` pages, from 0, in the
/// order written; a page named twice is there twice.
pub fn parse(spec: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in spec.split(',').map(str::trim) {
        match part {
            "" => continue,
            "all" => out.extend(0..count),
            "even" => out.extend((1..count).step_by(2)),
            "odd" => out.extend((0..count).step_by(2)),
            _ => {
                let (from, to) = match part.split_once('-') {
                    Some((a, b)) => (page(a, 1, count)?, page(b, count, count)?),
                    None => {
                        let p = page(part, 0, count)?;
                        (p, p)
                    }
                };
                if from == 0 || to == 0 || from > count || to > count {
                    return Err(format!("{part}: the file has pages 1 to {count}"));
                }
                if from <= to {
                    out.extend(from - 1..to);
                } else {
                    out.extend((to - 1..from).rev());
                }
            }
        }
    }
    if out.is_empty() {
        return Err(format!("{spec}: no pages"));
    }
    Ok(out)
}

/// A page number, or `open` when none is written.
fn page(s: &str, open: usize, count: usize) -> Result<usize, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(open);
    }
    if s == "last" || s == "z" {
        return Ok(count);
    }
    s.parse().map_err(|_| format!("{s}: not a page number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_lists() {
        assert_eq!(parse("1-3,5", 6).unwrap(), [0, 1, 2, 4]);
        assert_eq!(parse("5-", 6).unwrap(), [4, 5]);
        assert_eq!(parse("-2", 6).unwrap(), [0, 1]);
        assert_eq!(parse("3-1", 6).unwrap(), [2, 1, 0]);
        assert_eq!(parse("odd", 5).unwrap(), [0, 2, 4]);
        assert_eq!(parse("even", 5).unwrap(), [1, 3]);
        assert_eq!(parse("last,1", 3).unwrap(), [2, 0]);
        assert_eq!(parse("all", 2).unwrap(), [0, 1]);
        assert!(parse("7", 6).is_err());
        assert!(parse("0", 6).is_err());
        assert!(parse("x", 6).is_err());
        assert!(parse("", 6).is_err());
    }
}
