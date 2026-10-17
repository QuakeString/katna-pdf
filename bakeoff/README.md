# Engine bake-off

Draws the same PDFs with hayro, PDFium and MuPDF and compares speed,
failures and looks (docs/PLAN.md §3.1). Results so far:
[docs/bakeoff/2026-10-engines.md](../docs/bakeoff/2026-10-engines.md).

This crate is kept out of the workspace, so PDFium and MuPDF never enter
Katna PDF's own build, CI or `cargo deny` checks. Building it compiles
MuPDF from source, which needs a C compiler and clang.

```sh
# PDFium: any libpdfium.so, e.g. from the pypdfium2 wheel
python3 -m pip download --no-deps pypdfium2 && unzip pypdfium2-*.whl 'pypdfium2_raw/*'
export PDFIUM_LIB=$PWD/pypdfium2_raw/libpdfium.so

# A corpus: pdf.js's test files
git clone --depth 1 --filter=blob:none --sparse https://github.com/mozilla/pdf.js
git -C pdf.js sparse-checkout set test/pdfs

cargo build --release
target/release/katna-pdf-bakeoff sweep pdf.js/test/pdfs sweep.jsonl --timeout 30
target/release/katna-pdf-bakeoff report sweep.jsonl > report.md

# Vector-heavy CAD-style sheets
mkdir cad && (cd cad && python3 ../make_cad.py)
target/release/katna-pdf-bakeoff sweep cad cad.jsonl --timeout 120
```

Run sweeps on an otherwise idle machine; each file runs alone so the
times compare.
