// SPDX-License-Identifier: GPL-3.0-or-later

//! The tables a page's text makes, found from where its words sit: a
//! table is rows of text one under another with two or more cells
//! across, the cells kept apart by gaps wider than a space, and its
//! columns are the stretches of the page's width that the cells cover.
//! They come out as CSV.

use std::fmt::Write as _;

use crate::document::Document;

/// A table found on a page, row by row, cell by cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The page it is on, from 0.
    pub page: usize,
    /// Its rows, top first, each with one entry a column (empty where
    /// the row has nothing in that column).
    pub rows: Vec<Vec<String>>,
}

/// A piece of a line kept apart from its neighbours by a wide gap.
#[derive(Debug, Clone)]
struct Cell {
    text: String,
    /// Left, bottom, right, top in the page's space.
    rect: [f64; 4],
}

impl Document {
    /// The tables page `page`'s text makes, top first.
    pub fn tables(&self, page: usize) -> Vec<Table> {
        let mut cells = Vec::new();
        for (text, boxes) in self.line_boxes(page) {
            cells.extend(split_cells(&text, &boxes));
        }
        tables_of(page, cells)
    }
}

/// The tables of `pages`, in page order.
pub fn tables(doc: &Document, pages: &[usize]) -> Vec<Table> {
    pages.iter().flat_map(|&p| doc.tables(p)).collect()
}

/// `tables` as CSV: one table after another with a blank line between,
/// cells quoted where they hold a comma, a quote or a line break.
pub fn csv(tables: &[Table]) -> String {
    let mut out = String::new();
    for (i, table) in tables.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for row in &table.rows {
            let line: Vec<String> = row.iter().map(|c| quote(c)).collect();
            let _ = writeln!(out, "{}", line.join(","));
        }
    }
    out
}

fn quote(cell: &str) -> String {
    if cell.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell.to_owned()
    }
}

/// A line's cells: its chars in order, a new cell wherever the gap to
/// the next char is well wider than the line's spaces (twice one, or
/// half the line's height when it has none).
fn split_cells(text: &str, boxes: &[[f64; 4]]) -> Vec<Cell> {
    let mut out = Vec::new();
    let height = boxes
        .iter()
        .filter(|b| b[1] <= b[3])
        .map(|b| b[3] - b[1])
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let space = text
        .chars()
        .zip(boxes)
        .filter(|(c, b)| *c == ' ' && b[0] <= b[2] && b[2] - b[0] > 0.0)
        .map(|(_, b)| b[2] - b[0])
        .fold(f64::MAX, f64::min);
    let wide = if space == f64::MAX {
        0.5 * height
    } else {
        (2.0 * space).max(0.4 * height)
    };
    let mut text_now = String::new();
    let mut rect: Option<[f64; 4]> = None;
    let mut last_right = f64::NAN;
    let mut split = |text_now: &mut String, rect: &mut Option<[f64; 4]>| {
        if let Some(r) = rect.take() {
            push_cell(&mut out, text_now, r);
        }
        text_now.clear();
    };
    for (ch, b) in text.chars().zip(boxes) {
        let empty = b[0] > b[2] || b[1] > b[3];
        if empty {
            // A space put between pieces: a gap, but no box of its own.
            if !text_now.is_empty() && !text_now.ends_with(' ') {
                text_now.push(' ');
            }
            continue;
        }
        if ch.is_whitespace() {
            // A space much wider than the others is a gap between
            // cells, not between words.
            if b[2] - b[0] > wide {
                split(&mut text_now, &mut rect);
            } else if rect.is_some() && !text_now.ends_with(' ') {
                text_now.push(' ');
            }
            last_right = b[2];
            continue;
        }
        if rect.is_some() && b[0] - last_right > wide {
            split(&mut text_now, &mut rect);
        }
        text_now.push(ch);
        rect = Some(match rect {
            Some(r) => [
                r[0].min(b[0]),
                r[1].min(b[1]),
                r[2].max(b[2]),
                r[3].max(b[3]),
            ],
            None => *b,
        });
        last_right = b[2];
    }
    split(&mut text_now, &mut rect);
    out
}

fn push_cell(out: &mut Vec<Cell>, text: &str, rect: [f64; 4]) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(Cell {
            text: text.to_owned(),
            rect,
        });
    }
}

/// Cells gathered into rows by their baselines, rows into tables.
fn tables_of(page: usize, mut cells: Vec<Cell>) -> Vec<Table> {
    // Rows: cells whose bottoms are within half a line of one another,
    // top of the page first.
    cells.sort_by(|a, b| b.rect[1].total_cmp(&a.rect[1]));
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for cell in cells {
        let height = cell.rect[3] - cell.rect[1];
        match rows.last_mut() {
            Some(row) if (row[0].rect[1] - cell.rect[1]).abs() < 0.5 * height.max(1.0) => {
                row.push(cell);
            }
            _ => rows.push(vec![cell]),
        }
    }
    for row in &mut rows {
        row.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    }
    // Tables: runs of rows close under one another, with two or more
    // cells, a row of one cell allowed inside when it sits in a column.
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if rows[i].len() < 2 {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        let mut last_full = i;
        while end < rows.len() {
            let above = &rows[end - 1];
            let row = &rows[end];
            let height = above
                .iter()
                .map(|c| c.rect[3] - c.rect[1])
                .fold(0.0_f64, f64::max);
            let gap = above[0].rect[1] - row[0].rect[3];
            if gap > 2.5 * height.max(1.0) {
                break;
            }
            if row.len() >= 2 {
                last_full = end;
            } else {
                // A row of one cell belongs when the cell keeps within
                // one of the table's columns so far.
                let columns = columns_of(&rows[i..end]);
                let cell = &row[0];
                let in_column = columns
                    .iter()
                    .any(|&(l, r)| cell.rect[0] >= l - 2.0 && cell.rect[2] <= r + 2.0);
                if !in_column {
                    break;
                }
            }
            end += 1;
        }
        let end = last_full + 1;
        if end - i >= 2 {
            let laid = lay_out(&rows[i..end]);
            if laid[0].len() >= 2 && !is_prose(&laid) {
                out.push(Table { page, rows: laid });
            }
        }
        i = end;
    }
    out
}

/// Whether `rows` are text set in columns rather than a table: every
/// column that is mostly filled holds five words or more a cell.
fn is_prose(rows: &[Vec<String>]) -> bool {
    let columns = rows[0].len();
    (0..columns).all(|k| {
        let cells: Vec<&String> = rows
            .iter()
            .map(|r| &r[k])
            .filter(|c| !c.is_empty())
            .collect();
        if cells.len() * 2 < rows.len() {
            return true;
        }
        let words: usize = cells.iter().map(|c| c.split_whitespace().count()).sum();
        words >= 5 * cells.len()
    })
}

/// Rows of cells as rows of columns: the columns are the stretches of
/// the width the cells cover, cells joined with a space where two
/// share one.
fn lay_out(rows: &[Vec<Cell>]) -> Vec<Vec<String>> {
    let columns = columns_of(rows);
    rows.iter()
        .map(|row| {
            let mut out = vec![String::new(); columns.len()];
            for cell in row {
                let x = (cell.rect[0] + cell.rect[2]) / 2.0;
                let k = columns
                    .iter()
                    .position(|&(l, r)| l <= x && x <= r)
                    .unwrap_or(0);
                if !out[k].is_empty() {
                    out[k].push(' ');
                }
                out[k].push_str(&cell.text);
            }
            out
        })
        .collect()
}

/// The stretches of the width `rows`' cells cover, left to right.
fn columns_of(rows: &[Vec<Cell>]) -> Vec<(f64, f64)> {
    let mut spans: Vec<(f64, f64)> = rows
        .iter()
        .flatten()
        .map(|c| (c.rect[0], c.rect[2]))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut columns: Vec<(f64, f64)> = Vec::new();
    for (left, right) in spans {
        match columns.last_mut() {
            Some(last) if left <= last.1 => last.1 = last.1.max(right),
            _ => columns.push((left, right)),
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, left: f64, bottom: f64, right: f64) -> Cell {
        Cell {
            text: text.to_owned(),
            rect: [left, bottom, right, bottom + 10.0],
        }
    }

    #[test]
    fn rows_under_one_another_make_a_table() {
        let cells = vec![
            cell("Title", 72.0, 700.0, 200.0),
            cell("Item", 72.0, 600.0, 100.0),
            cell("Qty", 300.0, 600.0, 320.0),
            cell("Bolts", 72.0, 586.0, 110.0),
            cell("12", 305.0, 586.0, 320.0),
            cell("Nuts", 72.0, 572.0, 105.0),
            cell("3", 310.0, 572.0, 320.0),
            cell("Footer text", 72.0, 500.0, 200.0),
        ];
        let tables = tables_of(0, cells);
        assert_eq!(tables.len(), 1);
        assert_eq!(
            tables[0].rows,
            vec![
                vec!["Item".to_owned(), "Qty".to_owned()],
                vec!["Bolts".to_owned(), "12".to_owned()],
                vec!["Nuts".to_owned(), "3".to_owned()],
            ]
        );
    }

    #[test]
    fn text_in_two_columns_is_not_a_table() {
        let left = "Dynamic languages such as JavaScript are harder to";
        let right = "applications such as mail, documents and the like";
        let cells = (0..6)
            .flat_map(|i| {
                let y = 700.0 - 14.0 * i as f64;
                [cell(left, 72.0, y, 290.0), cell(right, 310.0, y, 530.0)]
            })
            .collect();
        assert!(tables_of(0, cells).is_empty());
    }

    #[test]
    fn a_wide_gap_splits_a_line_into_cells() {
        let text = "ab cd";
        let boxes = [
            [0.0, 0.0, 5.0, 10.0],
            [5.0, 0.0, 10.0, 10.0],
            [10.0, 0.0, 13.0, 10.0],
            [40.0, 0.0, 45.0, 10.0],
            [45.0, 0.0, 50.0, 10.0],
        ];
        let cells = split_cells(text, &boxes);
        let texts: Vec<&str> = cells.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["ab", "cd"]);
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        let table = Table {
            page: 0,
            rows: vec![vec![
                "a,b".to_owned(),
                "say \"hi\"".to_owned(),
                "plain".to_owned(),
            ]],
        };
        assert_eq!(csv(&[table]), "\"a,b\",\"say \"\"hi\"\"\",plain\n");
    }
}
