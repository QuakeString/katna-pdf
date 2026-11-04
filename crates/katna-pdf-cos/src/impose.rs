// SPDX-License-Identifier: GPL-3.0-or-later

//! Pages laid out on sheets for printing: several pages to a sheet, a
//! booklet (sheets folded in the middle, printed on both sides), or a
//! poster (one page spread over several sheets). Each page becomes a
//! form XObject (its content and resources, its crop box the form's
//! box, its turn in the form's matrix), drawn scaled and centred in its
//! cell. Annotations are not carried over: the sheets are for the
//! printer.

use std::collections::HashSet;

use crate::document::Document;
use crate::object::{Dict, Object, Ref};
use crate::pages::{ComposeError, Copier, page_dict};

/// How the pages go on the sheets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetLayout {
    /// `across` × `down` pages on each sheet, in reading order.
    NUp { across: u32, down: u32 },
    /// Two pages a side, ordered so the sheets, printed on both sides
    /// (turned on the short edge), stack and fold into a booklet.
    Booklet,
    /// Each page over `across` × `down` sheets, to be joined.
    Poster { across: u32, down: u32 },
}

/// A sheet's size in points, and the space left around each page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheet {
    pub width: f32,
    pub height: f32,
    /// Space between pages and from the edge, in points.
    pub gap: f32,
}

impl Sheet {
    pub const A4: (f32, f32) = (595.0, 842.0);
    pub const LETTER: (f32, f32) = (612.0, 792.0);

    /// A sheet of `paper` (width, height), on its side when `landscape`.
    pub fn of(paper: (f32, f32), landscape: bool) -> Self {
        let (w, h) = if landscape == (paper.0 < paper.1) {
            (paper.1, paper.0)
        } else {
            paper
        };
        Sheet {
            width: w,
            height: h,
            gap: 18.0,
        }
    }
}

/// `pages` of `doc` (by index) laid out as `layout` says on sheets of
/// `sheet`, as a new file.
pub fn impose(
    doc: &Document,
    pages: &[usize],
    layout: SheetLayout,
    sheet: Sheet,
) -> Result<Vec<u8>, ComposeError> {
    if pages.is_empty() {
        return Err(ComposeError::Empty);
    }
    if doc.is_encrypted() {
        return Err(ComposeError::Encrypted);
    }
    let refs = doc.pages();
    let docs = [doc];
    let mut copier = Copier::default();
    let catalog = copier.reserve();
    let root = copier.reserve();
    // Pages are not copied as pages: only as forms.
    for &r in &refs {
        copier.map.insert((0, r), None);
    }
    // Each page used, once, as a form, with its size as shown.
    let mut forms: Vec<(usize, Ref, f32, f32)> = Vec::new();
    let mut seen = HashSet::new();
    for &page in pages {
        let r = *refs.get(page).ok_or(ComposeError::NoSuchPage(page))?;
        if !seen.insert(page) {
            continue;
        }
        let info = doc.page_info(page).ok_or(ComposeError::NoSuchPage(page))?;
        let (w, h) = info.size();
        let num = copier.reserve();
        let form = form_of(doc, r, &info, &mut copier, &docs);
        copier.set(num, form);
        forms.push((page, Ref::new(num, 0), w, h));
    }
    let form_of_page = |page: usize| forms.iter().find(|f| f.0 == page).copied();

    // Each sheet: the cells on it, each a page (or none) scaled into a
    // part of the sheet (left, bottom, width, height), with an offset
    // within the page for poster tiles.
    let mut sheets: Vec<Vec<Cell>> = Vec::new();
    match layout {
        SheetLayout::NUp { across, down } => {
            let (across, down) = (across.max(1), down.max(1));
            let per = (across * down) as usize;
            for chunk in pages.chunks(per) {
                let mut cells = Vec::new();
                for (i, &page) in chunk.iter().enumerate() {
                    let (col, row) = (i as u32 % across, i as u32 / across);
                    cells.push(Cell {
                        page,
                        area: grid_cell(sheet, across, down, col, row),
                        tile: None,
                    });
                }
                sheets.push(cells);
            }
        }
        SheetLayout::Booklet => {
            // Padded to a multiple of four with blank places.
            let n = pages.len().div_ceil(4) * 4;
            let at = |i: usize| pages.get(i).copied();
            for k in 0..n / 4 {
                let front = [at(n - 1 - 2 * k), at(2 * k)];
                let back = [at(2 * k + 1), at(n - 2 - 2 * k)];
                for side in [front, back] {
                    let cells = side
                        .into_iter()
                        .enumerate()
                        .filter_map(|(col, page)| {
                            page.map(|page| Cell {
                                page,
                                area: grid_cell(sheet, 2, 1, col as u32, 0),
                                tile: None,
                            })
                        })
                        .collect();
                    sheets.push(cells);
                }
            }
        }
        SheetLayout::Poster { across, down } => {
            let (across, down) = (across.max(1), down.max(1));
            for &page in pages {
                for row in 0..down {
                    for col in 0..across {
                        sheets.push(vec![Cell {
                            page,
                            area: grid_cell(sheet, 1, 1, 0, 0),
                            tile: Some((col, row, across, down)),
                        }]);
                    }
                }
            }
        }
    }

    let mut kids = Vec::new();
    for cells in sheets {
        let mut content = String::new();
        let mut xobjects = Dict::new();
        for (i, cell) in cells.iter().enumerate() {
            let Some((_, form, w, h)) = form_of_page(cell.page) else {
                continue;
            };
            let name = format!("P{}", i + 1);
            xobjects.set(&name, form);
            let [left, bottom, cw, ch] = cell.area;
            let (scale, dx, dy) = match cell.tile {
                None => {
                    // As big as fits the cell, in its middle.
                    let s = (cw / w).min(ch / h);
                    (s, left + (cw - w * s) / 2.0, bottom + (ch - h * s) / 2.0)
                }
                Some((col, row, across, down)) => {
                    // The page spread over all the sheets; this sheet shows
                    // its tile, rows counted from the top.
                    let s = (cw * across as f32 / w).min(ch * down as f32 / h);
                    let total_w = w * s;
                    let total_h = h * s;
                    let x0 = left - col as f32 * cw + (cw * across as f32 - total_w) / 2.0;
                    let y0 =
                        bottom - (down - 1 - row) as f32 * ch + (ch * down as f32 - total_h) / 2.0;
                    (s, x0, y0)
                }
            };
            // Clipped to its cell: a poster tile shows only its part.
            content.push_str(&format!(
                "q {} {} {} {} re W n {} 0 0 {} {} {} cm /{name} Do Q\n",
                real(left),
                real(bottom),
                real(cw),
                real(ch),
                real(scale),
                real(scale),
                real(dx),
                real(dy)
            ));
        }
        let stream = copier.reserve();
        copier.set(
            stream,
            Object::Stream(crate::dss::compressed(content.as_bytes())),
        );
        let mut resources = Dict::new();
        resources.set("XObject", Object::Dict(xobjects));
        let mut page = Dict::new();
        page.set("Type", Object::name("Page"));
        page.set("Parent", Ref::new(root, 0));
        page.set(
            "MediaBox",
            vec![
                Object::Int(0),
                Object::Int(0),
                Object::Real(f64::from(sheet.width)),
                Object::Real(f64::from(sheet.height)),
            ],
        );
        page.set("Resources", Object::Dict(resources));
        page.set("Contents", Ref::new(stream, 0));
        let num = copier.reserve();
        copier.set(num, Object::Dict(page));
        kids.push(Object::Ref(Ref::new(num, 0)));
    }
    let mut tree = Dict::new();
    tree.set("Type", Object::name("Pages"));
    tree.set("Count", kids.len() as i64);
    tree.set("Kids", kids);
    copier.set(root, Object::Dict(tree));
    let mut cat = Dict::new();
    cat.set("Type", Object::name("Catalog"));
    cat.set("Pages", Ref::new(root, 0));
    copier.set(catalog, Object::Dict(cat));
    let mut trailer = Dict::new();
    trailer.set("Root", Ref::new(catalog, 0));
    if let Some(info) = doc.trailer().get("Info") {
        let info = copier.copy(&docs, 0, info);
        trailer.set("Info", info);
    }
    copier.drain(&docs);
    Ok(copier.write(trailer))
}

/// One page's place on a sheet.
struct Cell {
    page: usize,
    /// Left, bottom, width, height on the sheet.
    area: [f32; 4],
    /// For a poster: this sheet's column and row of the page, and how
    /// many across and down.
    tile: Option<(u32, u32, u32, u32)>,
}

/// Cell (`col`, `row`) of an `across` × `down` grid on `sheet`, rows
/// from the top, with the gap around it.
fn grid_cell(sheet: Sheet, across: u32, down: u32, col: u32, row: u32) -> [f32; 4] {
    let cw = (sheet.width - sheet.gap * (across + 1) as f32) / across as f32;
    let ch = (sheet.height - sheet.gap * (down + 1) as f32) / down as f32;
    let left = sheet.gap + col as f32 * (cw + sheet.gap);
    let bottom = sheet.height - sheet.gap - (row + 1) as f32 * ch - row as f32 * sheet.gap;
    [left, bottom, cw.max(1.0), ch.max(1.0)]
}

/// Page `r` of `doc` as a form XObject: its content, resources, crop
/// box, and a matrix that turns it as `/Rotate` says, so it stands
/// upright with its bottom left at the origin.
fn form_of(
    doc: &Document,
    r: Ref,
    info: &crate::pages::PageInfo,
    copier: &mut Copier,
    docs: &[&Document],
) -> Object {
    let dict = page_dict(doc, r);
    let page = doc.pages().iter().position(|p| *p == r).unwrap_or(0);
    let content = doc.page_content_bytes(page).unwrap_or_default();
    let [x0, y0, x1, y1] = info.crop;
    let matrix: [f32; 6] = match info.rotate.rem_euclid(360) {
        90 => [0.0, -1.0, 1.0, 0.0, -y0, x1],
        180 => [-1.0, 0.0, 0.0, -1.0, x1, y1],
        270 => [0.0, 1.0, -1.0, 0.0, y1, -x0],
        _ => [1.0, 0.0, 0.0, 1.0, -x0, -y0],
    };
    let mut form = Dict::new();
    form.set("Type", Object::name("XObject"));
    form.set("Subtype", Object::name("Form"));
    form.set("BBox", crate::annots::reals(&info.crop));
    form.set(
        "Matrix",
        matrix
            .iter()
            .map(|v| Object::Real(f64::from(*v)))
            .collect::<Vec<_>>(),
    );
    if let Some(resources) = dict.get("Resources") {
        form.set("Resources", copier.copy(docs, 0, resources));
    }
    if let Some(group) = dict.get("Group") {
        form.set("Group", copier.copy(docs, 0, group));
    }
    Object::Stream(crate::dss::compressed_with(form, &content))
}

fn real(v: f32) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_cells_fill_the_sheet_from_the_top() {
        let sheet = Sheet {
            width: 100.0,
            height: 200.0,
            gap: 10.0,
        };
        let [l, b, w, h] = grid_cell(sheet, 2, 2, 0, 0);
        assert_eq!((l, w, h), (10.0, 35.0, 85.0));
        assert_eq!(b, 200.0 - 10.0 - 85.0);
        let [l2, b2, _, _] = grid_cell(sheet, 2, 2, 1, 1);
        assert_eq!((l2, b2), (55.0, 10.0));
    }

    #[test]
    fn sheets_turn_as_asked() {
        let s = Sheet::of(Sheet::A4, true);
        assert_eq!((s.width, s.height), (842.0, 595.0));
        let s = Sheet::of((842.0, 595.0), false);
        assert_eq!((s.width, s.height), (595.0, 842.0));
        assert_eq!(real(0.5), "0.5");
        assert_eq!(real(2.0), "2");
    }
}
