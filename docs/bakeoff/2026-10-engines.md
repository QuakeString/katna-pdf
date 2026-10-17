# Engine bake-off, October 2026

Phase 0 asks which engine draws Katna PDF's pages (docs/PLAN.md §3.1):
PDFium, MuPDF or hayro. This is the first measurement. The final choice
was made on 2026-10-17: **hayro only** (PLAN.md §3.1).

## Recommendation

**Keep hayro, the pure-Rust engine, as Katna PDF's only renderer. Use PDFium
and MuPDF as yardsticks in the bake-off tool, not in the app.**

- **As reliable as the C engines.** On 994 real-world PDFs, hayro drew
  97.6% of the files, PDFium 97.4% and MuPDF 96.4%.
- **Looked right most often.** On page 1, hayro was the odd one out on
  3 files, against 39 for PDFium and 41 for MuPDF.
- **As fast in everyday reading.** Its median page at 150% took 5.0 ms,
  against 6.8 ms for PDFium and 3.3 ms for MuPDF. It was the fastest to
  open a file and show page 1. All three met both speed budgets on about
  99% of files.
- **Fits the project.** It keeps Katna's "Rust only" rule. It adds no
  AGPL question (MuPDF) and no C++ binary to ship (PDFium).

hayro's weak spots are real, and they are the work this choice brings:

1. **Deep zoom on pages with images.** One 1024² tile at 800% took
   22 s and 5 GB of memory on `bug1795263.pdf`. On `issue2948.pdf` it
   took 3 s and 1 GB. The C engines took a few milliseconds on both.
   hayro seems to build the whole page at full size before cutting out
   the tile. This needs fixing (in hayro upstream, or by drawing images
   at screen size ourselves) before the CAD and zoom work in Phase 1.
2. **Real CAD drawings.** On the one real architectural plan in the set
   (`22060_A1_01_Plans.pdf`, A1, 6 MB), a page took 462 ms in hayro,
   2,015 ms in PDFium and 68 ms in MuPDF. On synthetic sheets with
   100k–1M strokes, hayro was the fastest of the three, yet all three
   missed the budget by seconds. Either way, the §2.5 vector pipeline
   (scene, R-tree, tile pyramid) is needed whichever engine draws normal
   pages.
3. **Three files fail only in hayro.** Two tiling patterns hang it and
   one panics. These go upstream as bug reports with the files. The app
   already draws pages off the UI thread; Phase 2 moves drawing into
   worker processes, so a crash costs one page, not the app.

The corpus favours hayro somewhat: it is pdf.js's test set, which hayro's
author also tests against. The next round should add our own files: Indian
government forms, scanned receipts, DSC-signed PDFs and real AutoCAD
exports (still wanted from you).

## How it was measured

- **Corpus.** Mozilla pdf.js's `test/pdfs`: 994 files, many of them
  broken on purpose. There are also three synthetic CAD-style sheets with
  100k, 400k and 1M strokes (`bakeoff/make_cad.py`).
- **Engines.**
  - hayro 0.8.0 through `katna-pdf-render`.
  - PDFium 156.0.8076 (pypdfium2's build) through `pdfium-render` 0.9.4.
  - MuPDF through `mupdf` 0.8.0, built from source, base-14 fonts only.
- **Machine.** 4-core Intel Xeon at 2.1 GHz, Linux. One file at a time,
  each in its own process, with a 30 s limit, so crashes and hangs are
  counted rather than stopping the run.
- **Times.**
  - Open plus page 1 at 150%: the "file to first page" budget of 300 ms.
  - The mean of up to 20 pages spread through the file: the "sharp zoom"
    budget of 150 ms per page.
  - One 1024² tile from the middle of page 1 at 800%.
- **Looks.** Page 1 of each engine is shrunk to a 32×32 grey picture. An
  engine is the "odd one out" when the other two agree within 3 grey
  levels and it differs from both by more than 6. This catches blank,
  missing or badly wrong pages, not fine anti-aliasing.

Run it again with `bakeoff/README.md`.

## Results

### Opens and draws

| Engine | Drew page 1 | Error | Crash | Hang (timeout) | Sampled pages failed | 8× tile failed |
|---|---|---|---|---|---|---|
| hayro | 970 (97.6%) | 19 | 1 | 4 | 0 files | 0 |
| pdfium | 968 (97.4%) | 23 | 1 | 2 | 2 files | 0 |
| mupdf | 958 (96.4%) | 31 | 1 | 4 | 1 files | 0 |

### Speed on the 951 files all engines drew (ms; median / 95th percentile / worst)

| Engine | Open + page 1 at 150% | Within 300 ms | Page at 150% | Within 150 ms | 1024² tile at 800% | Peak memory (MB, median) |
|---|---|---|---|---|---|---|
| hayro | 5.9 / 41.6 / 7704 | 99.4% | 5.0 / 31.7 / 7490 | 98.5% | 5.9 / 32.4 / 22448 | 20 |
| pdfium | 8.5 / 43.1 / 1960 | 99.5% | 6.8 / 28.0 / 2015 | 98.8% | 1.8 / 24.1 / 781 | 23 |
| mupdf | 9.4 / 59.2 / 3808 | 99.6% | 3.3 / 20.6 / 531 | 99.4% | 2.6 / 24.2 / 856 | 18 |

hayro's time as a multiple of the others' (median over the same files; below 1 is faster):

- vs pdfium: open + page 1 ×0.62, page ×0.86, deep tile ×3.04
- vs mupdf: open + page 1 ×0.54, page ×1.63, deep tile ×2.46

### Odd one out on page 1 (the other two engines agree, this one differs by more than 6 grey levels on average)

- **hayro**: 3 files
- **pdfium**: 39 files
- **mupdf**: 41 files

hayro, worst first: recursiveCompositGlyf.pdf (179), issue2537r.pdf (9), annotation-square-circle-without-appearance.pdf (7)

pdfium, worst first: knockout_inner_backdrop.pdf (57), filled-background.pdf (47), issue18694.pdf (47), red_stamp.pdf (47), tiling-pattern-fold-order.pdf (41), issue8565.pdf (34), bug946506.pdf (30), postscript_type4_many_outputs.pdf (27), smask_luminosity_oob_transfer.pdf (26), colors.pdf (25), issue6298.pdf (23), bug920426.pdf (23)

mupdf, worst first: axial_shading_many_inputs.pdf (255), jbig2_file_header.pdf (255), bitmap-composite-and-xnor-refine.pdf (237), bitmap-composite-or-xor-replace-refine.pdf (237), bitmap-halftone-refine.pdf (237), bitmap-refine-customat-tpgron.pdf (237), bitmap-refine-customat.pdf (237), bitmap-refine-lossless.pdf (237), bitmap-refine-refine.pdf (237), bitmap-refine-template1-tpgron.pdf (237), bitmap-refine-template1.pdf (237), bitmap-refine-tpgron.pdf (237)

### Files only this engine fails on

- **hayro**: 3 files. bug1978317.pdf (timeout); tiling-pattern-box.pdf (timeout); tiling-pattern-large-steps.pdf (crash: exit status: 101)
- **pdfium**: 4 files. bug1250079.pdf (error: FormatError); bug1606566.pdf (error: FormatError); bug1980958.pdf (error: FormatError); jpx_numcomps.pdf (crash: signal: 5 (SIGTRAP))
- **mupdf**: 9 files. GHOSTSCRIPT-698804-1-fuzzed.pdf (error: no pages); bug_jpx.pdf (crash: signal: 6 (SIGABRT)); issue18986.pdf (error: no pages); issue21436.pdf (error: no pages); issue9418.pdf (error: no pages); poppler-67295-0.pdf (error: no pages); poppler-91414-0-53.pdf (error: no pages); poppler-91414-0-54.pdf (error: no pages); type3_glyph_error.pdf (error: MuPDF error, code: 4, message: Illegal dimensions for)

### CAD stress sheets (ms; peak memory in MB)

| File | hayro page 1 | PDFium page 1 | MuPDF page 1 | hayro tile | PDFium tile | MuPDF tile | Memory h / P / M |
|---|---|---|---|---|---|---|---|
| A1, 100k strokes | 672 | 1,249 | 2,351 | 193 | 165 | 567 | 166 / 186 / 88 |
| A0, 400k strokes | 2,343 | 5,104 | 8,708 | 757 | 552 | 2,081 | 430 / 441 / 174 |
| A0, 1M strokes | 6,085 | 12,444 | 21,170 | 1,819 | 1,492 | 5,262 | 732 / 676 / 189 |

hayro's slowest files in the set, page at 150% / tile at 800% (ms),
compared with PDFium and MuPDF:

| File | hayro | PDFium | MuPDF |
|---|---|---|---|
| issue16263.pdf | 7,490 / 6,665 | 1,565 / 156 | 10 / 184 |
| 22060_A1_01_Plans.pdf (real CAD) | 462 / 776 | 2,015 / 781 | 68 / 145 |
| bug1795263.pdf | fast / 22,448 | fast / 2 | fast / 4 |
| issue2948.pdf | 273 / 3,176 | 11 / 5 | 8 / 8 |

MuPDF's odd-one-out list is mostly pdf.js's JBIG2 refinement tests, drawn
blank. PDFium's list is mostly transparency groups, soft masks and shadings.
