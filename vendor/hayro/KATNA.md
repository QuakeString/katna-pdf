# hayro, patched for Katna PDF

This is `hayro` 0.8.0 from crates.io (Apache-2.0 OR MIT, see
`LICENSE-APACHE` and `LICENSE-MIT`), used through `[patch.crates-io]` in
the workspace `Cargo.toml`. Its examples are left out.

The first commit that added this directory holds the crate unchanged, so
`git diff` against it shows the whole patch. When hayro is upgraded, copy
the new version here and apply the same changes, or drop a patch once
upstream has it.

## Tiling-pattern cells (src/paint.rs)

A tiling pattern is drawn by painting one cell into a pixmap one `XStep`
wide and `YStep` high, then repeating it. The cell's scale was capped only
by the pattern's bbox, so a pattern whose step is far larger than its bbox
asked for a huge pixmap at deep zoom. In pdf.js's `bug1795263.pdf`, the
`XStep` is 12,108 for a 25-unit bbox, so ten cells of 11,600² pixels came to
5 GB and 22 s for one 1024² tile at 800%. Past 65,535 pixels the `as u16`
cast saturated, and `tiling-pattern-large-steps.pdf` crashed.

The patch lowers the cell's scale so a cell is at most 2,048 pixels on a
side, and keeps it at least 1 pixel. Results on pdf.js's 994 test files at
150% and in an 800% tile (`bakeoff/`):

- `bug1795263.pdf`: tile 22,448 → 162 ms, peak memory 5 GB → 177 MB.
- `colorkeymask.pdf`: tile 2,018 → 5 ms, 939 → 37 MB.
- `tiling-pattern-large-steps.pdf`: crashed → draws in 58 ms.
- `tiling-pattern-box.pdf`: hung past 30 s → draws in 1.6 s.
- No other page changed. The three pages that moved differ by under
  0.05 grey levels from before.

Upstream issue text, for hayro's tracker: "Tiling patterns with an XStep or
YStep much larger than their BBox allocate step-sized cell pixmaps at the
device scale (11,600² per cell for bug1795263.pdf at 8×), and the `as u16`
cast in `paint.rs` saturates past 65,535. Capping the cell's scale so
`step × scale ≤ 2048` fixes the memory and the crash with no visible change
on pdf.js's corpus."
