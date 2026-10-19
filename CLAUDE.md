# Katna PDF — notes for AI-assisted development

Katna PDF is a PDF viewer, editor, signer and library manager in Rust and
GPUI, in the Katna family beside Katna Mail
(https://github.com/QuakeString/katna). Read `docs/PLAN.md` first.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo deny check licenses bans sources
cargo run --release -p katna-pdf-bench -- file.pdf
```

## Layout

- `crates/katna-pdf-render` — the page renderer behind an `Engine` trait
  (hayro today); `scene.rs` draws vector-heavy pages such as CAD
  drawings from a grid index (`docs/bakeoff/2026-10-cad.md`);
  `compare.rs` finds what changed between two versions (words lined up
  across pages, then pages drawn small for changed pictures and
  drawings) and draws the red and blue overlay;
  `measure.rs` keeps the lines a page draws for measuring tools to
  snap to (ends, middles, crossings);
  `worker.rs` draws in worker processes (the app runs itself with
  `--render-worker`; `KATNA_PDF_IN_PROCESS=1` draws in the app). No GPUI.
- `crates/katna-pdf-cos` — our own PDF object layer: parsing, xref and
  object streams, repair, incremental updates, plain-text reports
  as new PDFs (`report.rs`), editing the text pages draw, line by
  line or a paragraph at a time, re-wrapped to its width and lined up
  as it was (`edit.rs`, over `content.rs`'s content-stream operators and
  `fonts/`'s encodings and widths; letters a font lacks come from a
  standard font), and putting new text, pictures and shapes into a
  page's own content (`insert.rs`), and moving, resizing, turning, restacking,
  fading, recolouring, swapping and deleting the pictures and shapes
  already there (`objects.rs`), headers,
  footers, watermarks, Bates numbers and backgrounds on many pages (`stamp.rs`), the
  document's properties in `/Info` and XMP (`meta.rs`), the scale a
  drawing is measured in (`measure.rs`, from a page's `/VP`; measurement
  annotations carry theirs), opening
  password-locked files (`crypt.rs`: RC4, AES-128 and AES-256, an empty
  password tried first; updates to them stay locked) and putting an
  AES-256 password on or taking it off (`lock.rs`), true redaction
  (`redact.rs`: glyphs, pixels, shapes and annotations under an area
  taken out, PAN, Aadhaar, phone and email found, the file written
  again whole with only the objects it still uses),
  designing forms (`design.rs`: fields of every kind added with their
  own appearances, changed, moved, copied to other pages, taken out and
  put in Tab order), form data in and out as JSON, CSV, XFDF and FDF
  (`formdata.rs`), totals and number, date and percent formats done in
  Rust from the calls Acrobat's form tools write (`calc.rs`), the
  fields a flat form asks for found from its blanks, boxes, grid cells
  and squares to tick (`detect.rs`), the outline, links and named
  destinations changed (`nav.rs`), page labels read and written
  (`labels.rs`), how pages come on in a presentation (`present.rs`:
  `/Trans`, `/Dur`),
  page lists as people write them (`ranges.rs`), and files made smaller
  (`optimize.rs`: unused objects dropped, streams packed tighter, copies
  kept once, object streams, pictures brought down to a resolution, the
  result checked to draw the same). No GPUI, no C.
- `crates/katna-pdf-sign` — signing and checking signatures, pure
  Rust: certificate files (`p12.rs`), CMS / PAdES B-B (`cms.rs`),
  timestamps for B-T and document timestamps for B-LTA (`tsa.rs`, RFC
  3161 over ureq + rustls), USB
  tokens through PKCS#11 modules (`pkcs11.rs`; OpenSC, ePass2003 and
  other makers' drivers; `KATNA_PDF_PKCS11` adds one), and signers'
  certificates chained to trusted roots (`trust.rs`: the system's, plus
  India's CCA roots built in from `roots/india`, and a folder of the
  person's own), and
  revocation checked (`revocation.rs`: from the file's own validation
  data, then online by OCSP, then CRLs; missing issuers fetched from the
  certificate's AIA URL). The
  PDF side (fields, byte ranges) is `katna-pdf-cos`'s `sign.rs`, , certified
  files' allowed changes (DocMDP) and locked fields (FieldMDP) its `mdp.rs`, and long-term validation
  data (PAdES B-LT DSS) its `dss.rs`.
- `crates/katna-pdf-library` — the library: a SQLite catalog of the
  PDFs in watched folders (`scan.rs`: new, changed, moved and gone files;
  title, author, pages, signed, form, scanned, locked; a cover; each
  page's text in an FTS5 index), tags, stars, notes, smart collections
  as queries (`filter.rs`), copies by bytes or words, and many files
  merged, renamed from their properties, copied, tagged or made smaller (`bulk.rs`). Text and
  covers come through an `Engine`. No GPUI.
- `apps/katna-pdf` — the app window; `drives.rs` asks `katna-daemon` over
  D-Bus (`katna-dbus`) for the Google Drive and OneDrive of accounts signed
  in to Katna Mail, and `mail.rs` reads Katna Mail's store (`katna-store`,
  read-only) for PDFs attached to mail; the library browses and opens both
  (`library/drive.rs`, `library/mail.rs`). `katna-pdf --compare OLDER NEWER`
  opens a comparison (`viewer/compare.rs`). `speech.rs` reads aloud
  (Ctrl+Shift+Y, `viewer/speak.rs`) through Speech Dispatcher's socket,
  `espeak-ng`, or PowerShell's voice on Windows.
  F5 presents (`viewer/present.rs`): full screen, the pages' own
  transitions and timings from cos `present.rs`.
- `apps/katna-pdf-cli` — the command line: info, merge, split, pages,
  rotate, crop, sign (and certify), add-field, ltv, tokens, token-check, verify (with `--report` as a PDF), text and edit-text (`--blocks` / `--block` for paragraphs), insert, stamp, background, redact, compress, compare (with `--overlay` as a PNG), form (with `--detect`), form-data and fill, meta, lock and unlock, objects and object, bookmarks, links, dests and labels, annotations and XFDF, and library; `--json` for scripts and agents.
- `tools/katna-pdf-cos-check` — runs katna-pdf-cos over a folder of PDFs
  (loads, page counts against hayro, update round trips).
- `vendor/hayro`, `vendor/hayro-interpret`, `vendor/hayro-syntax` — hayro with our patches
  (`KATNA.md` in each).
- `bakeoff/` — the engine bake-off, outside the workspace; `bakeoff/ocr/`
  the OCR bake-off (results in its README).
- `tools/katna-pdf-bench` — open and draw timings against the budgets in
  `docs/PLAN.md` §2.4; `--cad` for zoom timings on drawings (§2.5).
- `i18n/en/katna-pdf.ftl` — English text people see.
- `packaging/` — the Arch package, desktop entry, icon and AppStream
  file; `linux/stage.sh` lays the files out for the .deb, .rpm, AppImage
  and tarball. CI publishes the Arch package on `arch-latest`, the others
  on `linux-latest` (each installed and opened by
  `ci/linux-package-test.sh` first) and a Windows zip on `windows-latest`.

## Rules

- License: GPL-3.0-or-later. Every source file starts with
  `// SPDX-License-Identifier: GPL-3.0-or-later`.
- GPUI, `katna-core`, `katna-ui`, `katna-platform`, `katna-dbus` and `katna-store` come from Katna's repository at one
  pinned rev (root `Cargo.toml`); move all of them together.
- GPUI only in `apps/katna-pdf` (enforced by `deny.toml`).
- Pure Rust where possible; TLS is `rustls` only. No `unsafe` code.
- Do not copy code from PdfCraft; its feature list is a reference only.
- Lengths use `katna_ui::px`/`unpx`, not `gpui::px`; radii, spacing and
  text sizes come from `katna_ui::tokens`.
- Text people see goes through `text::t("id", ..)` with the English in
  `i18n/en/katna-pdf.ftl` (moves to `katna_i18n::tr!` once katna-i18n
  accepts other repos' English).
- App IDs build on `katna_core::ids::PREFIX`; never hard-code it.
- Commit messages: `area: summary`.
