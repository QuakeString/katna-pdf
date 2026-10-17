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
  (hayro today). No GPUI.
- `crates/katna-pdf-cos` — our own PDF object layer: parsing, xref and
  object streams, repair, incremental updates. No GPUI, no C.
- `apps/katna-pdf` — the app window.
- `tools/katna-pdf-cos-check` — runs katna-pdf-cos over a folder of PDFs
  (loads, page counts against hayro, update round trips).
- `vendor/hayro`, `vendor/hayro-interpret` — hayro with our patches
  (`KATNA.md` in each).
- `bakeoff/` — the engine bake-off, outside the workspace; `bakeoff/ocr/`
  the OCR bake-off (results in its README).
- `tools/katna-pdf-bench` — open and draw timings against the budgets in
  `docs/PLAN.md` §2.4.
- `i18n/en/katna-pdf.ftl` — English text people see.

## Rules

- License: GPL-3.0-or-later. Every source file starts with
  `// SPDX-License-Identifier: GPL-3.0-or-later`.
- GPUI, `katna-core` and `katna-ui` come from Katna's repository at one
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
