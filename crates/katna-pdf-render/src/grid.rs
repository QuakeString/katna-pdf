// SPDX-License-Identifier: GPL-3.0-or-later

//! A uniform grid over a page that finds the items whose boxes meet a
//! region: the index behind a [`Scene`](crate::scene::Scene).
//!
//! It is built in one pass over the items, far quicker than an R-tree's
//! bulk load, which matters because a drawing's first view waits for it.
//! Each cell lists the items that reach into it. An item reaching into
//! many cells, such as a sheet's border, is kept in one shared list
//! instead, which every query looks through.

/// About this many items to a cell, for a page whose items are spread
/// evenly.
const PER_CELL: usize = 64;
/// The most cells along a side.
const MAX_SIDE: usize = 512;
/// An item reaching into more cells than this goes in the shared list.
const MAX_CELLS: usize = 16;

pub struct Grid {
    /// The page's width and height in points.
    size: [f32; 2],
    /// Cells along x and y.
    cols: usize,
    rows: usize,
    /// Cell `c`'s items are `items[starts[c]..starts[c + 1]]`.
    starts: Vec<u32>,
    items: Vec<u32>,
    /// Items in too many cells to list in each.
    wide: Vec<u32>,
}

impl Grid {
    /// Indexes `boxes`, each `[x0, y0, x1, y1]` in points, on a page of
    /// `size` points. Item `i` is `boxes[i]`.
    pub fn new(size: [f32; 2], boxes: &[[f32; 4]]) -> Grid {
        let cells = (boxes.len() / PER_CELL).max(1);
        // Square cells, as near as the page's shape allows.
        let aspect = (size[0] / size[1].max(1.0)).max(1e-3) as f64;
        let cols = ((cells as f64 * aspect).sqrt().round() as usize).clamp(1, MAX_SIDE);
        let rows = ((cells as f64 / aspect).sqrt().round() as usize).clamp(1, MAX_SIDE);
        let mut grid = Grid {
            size,
            cols,
            rows,
            starts: vec![0; cols * rows + 1],
            items: Vec::new(),
            wide: Vec::new(),
        };
        // Count each cell's items, turn the counts into starts, then fill.
        for b in boxes {
            if let Some((c0, c1, r0, r1)) = grid.span(b)
                && (c1 - c0 + 1) * (r1 - r0 + 1) <= MAX_CELLS
            {
                for r in r0..=r1 {
                    for c in c0..=c1 {
                        grid.starts[r * cols + c + 1] += 1;
                    }
                }
            }
        }
        for i in 1..grid.starts.len() {
            grid.starts[i] += grid.starts[i - 1];
        }
        grid.items = vec![0; grid.starts[cols * rows] as usize];
        let mut next = grid.starts.clone();
        for (i, b) in boxes.iter().enumerate() {
            let Some((c0, c1, r0, r1)) = grid.span(b) else {
                continue;
            };
            if (c1 - c0 + 1) * (r1 - r0 + 1) > MAX_CELLS {
                grid.wide.push(i as u32);
                continue;
            }
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let at = &mut next[r * cols + c];
                    grid.items[*at as usize] = i as u32;
                    *at += 1;
                }
            }
        }
        grid
    }

    /// The items whose boxes may meet `[x0, y0, x1, y1]`, each once, in
    /// item order, appended to `out`; `boxes` as given to [`Grid::new`].
    /// Every item that meets the region is there; `boxes` weeds out the
    /// rest of each cell.
    pub fn find(&self, region: [f32; 4], boxes: &[[f32; 4]], out: &mut Vec<u32>) {
        let meets = |b: &[f32; 4]| {
            b[0] <= region[2] && b[2] >= region[0] && b[1] <= region[3] && b[3] >= region[1]
        };
        let start = out.len();
        if let Some((c0, c1, r0, r1)) = self.span(&region) {
            for r in r0..=r1 {
                let row = r * self.cols;
                let (from, to) = (self.starts[row + c0], self.starts[row + c1 + 1]);
                out.extend(
                    self.items[from as usize..to as usize]
                        .iter()
                        .filter(|&&i| meets(&boxes[i as usize])),
                );
            }
        }
        out.extend(self.wide.iter().filter(|&&i| meets(&boxes[i as usize])));
        let found = &mut out[start..];
        found.sort_unstable();
        let mut kept = start;
        for i in start..out.len() {
            if i == start || out[i] != out[kept - 1] {
                out[kept] = out[i];
                kept += 1;
            }
        }
        out.truncate(kept);
    }

    /// Roughly how much memory the grid holds, in bytes.
    pub fn memory(&self) -> usize {
        (self.starts.capacity() + self.items.capacity() + self.wide.capacity()) * 4
    }

    /// The first and last column and row that `b` reaches into, counting
    /// whatever lies off the page as in the nearest edge cells, or `None`
    /// when `b` is not a box.
    fn span(&self, b: &[f32; 4]) -> Option<(usize, usize, usize, usize)> {
        if !(b[0] <= b[2] && b[1] <= b[3]) {
            return None;
        }
        let [w, h] = self.size;
        let cell = |v: f32, side: f32, n: usize| {
            ((v / side.max(1e-3) * n as f32).max(0.0) as usize).min(n - 1)
        };
        Some((
            cell(b[0], w, self.cols),
            cell(b[2], w, self.cols),
            cell(b[1], h, self.rows),
            cell(b[3], h, self.rows),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a query should find: every box meeting the region, in order.
    /// An inside-out box is no box.
    fn brute(region: [f32; 4], boxes: &[[f32; 4]]) -> Vec<u32> {
        (0..boxes.len() as u32)
            .filter(|&i| {
                let b = boxes[i as usize];
                b[0] <= b[2]
                    && b[1] <= b[3]
                    && b[0] <= region[2]
                    && b[2] >= region[0]
                    && b[1] <= region[3]
                    && b[3] >= region[1]
            })
            .collect()
    }

    #[test]
    fn finds_what_a_full_scan_finds() {
        let mut seed = 1u32;
        let mut next = move |range: f32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 * range
        };
        let size = [800.0, 500.0];
        let mut boxes: Vec<[f32; 4]> = (0..20_000)
            .map(|_| {
                let (x, y) = (next(820.0) - 10.0, next(520.0) - 10.0);
                [x, y, x + next(12.0), y + next(12.0)]
            })
            .collect();
        // A border round the sheet, a box off the page and an empty box.
        boxes.push([0.0, 0.0, 800.0, 500.0]);
        boxes.push([900.0, 900.0, 950.0, 950.0]);
        boxes.push([5.0, 5.0, 4.0, 4.0]);
        let grid = Grid::new(size, &boxes);
        assert!(!grid.wide.is_empty());
        let mut out = Vec::new();
        for _ in 0..200 {
            let (x, y, s) = (next(800.0), next(500.0), next(200.0));
            let region = [x, y, x + s, y + s / 2.0];
            out.clear();
            grid.find(region, &boxes, &mut out);
            assert_eq!(out, brute(region, &boxes), "{region:?}");
        }
        out.clear();
        grid.find([-50.0, -50.0, 900.0, 600.0], &boxes, &mut out);
        assert_eq!(out, brute([-50.0, -50.0, 900.0, 600.0], &boxes));
    }

    #[test]
    fn an_empty_page_finds_nothing() {
        let grid = Grid::new([100.0, 100.0], &[]);
        let mut out = vec![7];
        grid.find([0.0, 0.0, 100.0, 100.0], &[], &mut out);
        assert_eq!(out, [7]);
    }
}
