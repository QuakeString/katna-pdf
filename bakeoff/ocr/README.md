# OCR bake-off

Phase 0 compares pure-Rust OCR with Tesseract before Katna PDF picks an
engine for OCR and Live Text (`docs/PLAN.md` §4.9). It is not part of the
workspace and nothing here ships.

- `make_pages.py` draws test pages with known text in English, Hindi and
  Bengali (`text/`), in Noto Sans and Noto Serif from 10 to 14 pt, as:
  - **clean**: the page at 200 dpi.
  - **scan**: a flatbed scan, with a slight tilt, blur, grain, grey paper
    and JPEG.
  - **photo**: a phone photo, keystoned, unevenly lit, about 145 dpi.
  - **snap**: a quick photo from further away in a dim room, about 100 dpi
    with JPEG at quality 55.
- `src/main.rs` runs the two Rust engines on rten:
  - **ocrs** 0.13 as it ships.
  - **PP-OCR**: PaddleOCR's DB detector and CTC recogniser, with
    PaddleOCR's pre- and post-processing written out.
- `score.py` runs those engines and Tesseract, then scores each page.
  - **CER** is the character error rate. It also counts lines read out of
    order.
  - **Words** is the share of the page's words read exactly, in any order.

## Running it

```sh
cargo build --release
python3 -I make_pages.py PAGES
python3 -I score.py PAGES MODELS > results.json
```

`MODELS` needs these files:

- `text-detection.rten` and `text-recognition.rten` from ocrs-models.
- `det.onnx`, `rec.onnx` and `keys.txt` for PP-OCR. The models used here
  are PP-OCRv6 small, as bundled in the `rapidocr` wheel on PyPI.
  `keys.txt` is the rec model's `character` metadata, one per line.

## Results (2026-10-17)

These ran on 4 cores of a 2.1 GHz Xeon over 4 pages per language and kind.
Every engine used all 4 cores. Tesseract 5.3.4 used the stock Ubuntu
`eng`, `hin` and `ben` models.

| Language | Kind  | Engine    | CER %  | Words % | ms/page |
|----------|-------|-----------|-------:|--------:|--------:|
| English  | clean | PP-OCR    |   0.00 |   100.0 |    2868 |
| English  | clean | Tesseract |   0.00 |   100.0 |     896 |
| English  | clean | ocrs      |  20.45 |    97.0 |     974 |
| English  | scan  | PP-OCR    |   0.02 |    99.9 |    2770 |
| English  | scan  | Tesseract |   0.02 |    99.9 |     851 |
| English  | scan  | ocrs      |  17.87 |    97.2 |     813 |
| English  | photo | PP-OCR    |   0.00 |   100.0 |    2341 |
| English  | photo | Tesseract |   1.47 |    99.9 |     890 |
| English  | photo | ocrs      |   5.49 |    97.2 |     771 |
| English  | snap  | PP-OCR    |   0.03 |    99.8 |    1586 |
| English  | snap  | Tesseract |  13.98 |    93.3 |     658 |
| English  | snap  | ocrs      |  17.77 |    90.3 |     771 |
| Hindi    | clean | Tesseract |   0.73 |    97.5 |     972 |
| Hindi    | scan  | Tesseract |   0.71 |    97.6 |     916 |
| Hindi    | photo | Tesseract |   0.73 |    97.4 |     946 |
| Hindi    | snap  | Tesseract |   5.30 |    89.3 |     672 |
| Bengali  | clean | Tesseract |   0.46 |    98.0 |     682 |
| Bengali  | scan  | Tesseract |   0.36 |    98.2 |     658 |
| Bengali  | photo | Tesseract |   0.48 |    97.7 |     678 |
| Bengali  | snap  | Tesseract |  29.83 |    61.9 |     403 |

### What it says

- **PP-OCR on rten is the one to build on.** It read every English page
  almost perfectly. That includes the dim 100 dpi snaps, where Tesseract
  lost 7% of the words. It runs in pure Rust with no C library.
- **ocrs is not good enough.** It misreads characters ("closeo", "Was")
  on clean pages. Its line grouping also merges paragraph lines in the
  wrong order, which is what pushes its CER to 20% where the word score is
  97%. It reads only the Latin alphabet.
- **Tesseract is good on clean Indic print and poor on photos.** On
  Bengali snaps it loses 38% of the words, mostly conjuncts and vowel
  signs. Phone photos are what Live Text is for, so this matters most for
  Bengali.
- **PP-OCR's speed** here is that of a straightforward harness. Each
  pixel is indexed one at a time and every line is recognised alone. A
  real build would batch lines and keep the detector's input smaller.
  Even as it is, it fits OCR in the background, page by page.

### Not yet measured

- **PP-OCR's Hindi and Bengali recognisers.** PaddleOCR publishes a
  Devanagari model. Its download hosts (Hugging Face, ModelScope and
  Baidu's BOS) were blocked from the machine that ran this, so these rows
  are missing. Bengali needs a check that a PaddleOCR Bengali model
  exists at all. If none does, Bengali needs a model trained for it.
- **Real scans and phone photos.** These pages are drawn, so they are
  kinder than real paper. Real documents would make the comparison
  sharper, especially for Hindi and Bengali.
