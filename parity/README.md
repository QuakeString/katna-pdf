# Feature parity

`features.tsv` tracks every feature of Adobe Acrobat (Reader, Standard, Pro and
the paid AI plans) and Master PDF Editor 5.9.99 next to what Katna PDF ships, plus
Katna PDF's own features that neither has ([UNIQUE.md](UNIQUE.md)). It is the one
list progress is measured from: change a row's `katna` column in the PR that ships
the feature.

| Column | Meaning |
|---|---|
| `id` | `area.name`, never renamed |
| `area` | `core`, `view`, `edit`, `organize`, `comment`, `form`, `protect`, `sign`, `ocr`, `create`, `optimize`, `print`, `a11y`, `misc`, `library`, `idea` |
| `feature` | what it does |
| `adobe` | the cheapest Adobe plan that has it: `reader` (free), `standard`, `pro`, `studio` (Acrobat Studio or the AI Assistant add-on), `-` (Adobe has none) |
| `mpe` | Master PDF Editor has it: `y`, `p` (partly), `n` |
| `katna` | `shipped` (on main), `partial` (some of it on main; `note` says what is missing), `todo` (in scope, not built), `deferred` (the owner said later), `skip` (out of scope) |
| `tier` | our priority: P0 first release, P1 v1.0, P2 later |
| `where` | the PR, module or CLI command that has it |
| `note` | what is missing, or why |
| `source` | where the row came from: `acrobat` (the Acrobat Pro checklist below), `mpe` (Master PDF Editor's manual), `adobe` (Adobe's own pages, 2024–2026 additions), `katna` (ours) |

## Where we stand (2026-11-23, with form scripts)

| Area | Features | Shipped | Partial | To do | Deferred | Skip |
|---|---:|---:|---:|---:|---:|---:|
| Files and saving (`core`) | 61 | 35 | 12 | 11 | 1 | 2 |
| Viewing and navigation (`view`) | 108 | 64 | 13 | 26 | 0 | 5 |
| Editing content (`edit`) | 83 | 52 | 12 | 16 | 0 | 3 |
| Pages and document (`organize`) | 83 | 30 | 19 | 28 | 1 | 5 |
| Comments and review (`comment`) | 77 | 40 | 11 | 18 | 0 | 8 |
| Forms (`form`) | 89 | 50 | 16 | 11 | 2 | 10 |
| Security and redaction (`protect`) | 46 | 20 | 9 | 10 | 2 | 5 |
| Digital signatures (`sign`) | 59 | 40 | 2 | 14 | 0 | 3 |
| Scanning and OCR (`ocr`) | 31 | 0 | 0 | 3 | 25 | 3 |
| Create, convert and export (`create`) | 39 | 7 | 4 | 18 | 2 | 8 |
| Optimise and standards (`optimize`) | 61 | 10 | 9 | 22 | 0 | 20 |
| Printing (`print`) | 41 | 10 | 6 | 6 | 0 | 19 |
| Accessibility (`a11y`) | 62 | 3 | 4 | 49 | 0 | 6 |
| Compare, automation, settings, cloud (`misc`) | 101 | 15 | 20 | 34 | 2 | 30 |
| Library (`library`, ours) | 9 | 9 | 0 | 0 | 0 | 0 |
| Ideas (`idea`, ours, proposed) | 11 | 0 | 0 | 10 | 1 | 0 |
| **All** | **961** | **385** | **137** | **276** | **36** | **127** |

Against each product, counting only what is in scope (not `skip`):

| Against | In scope | Shipped | Partial | To do | Deferred |
|---|---:|---:|---:|---:|---:|
| Acrobat Reader (free) | 333 | 191 (57 %) | 65 | 75 | 2 |
| Acrobat Standard (Reader + Standard) | 553 | 299 (54 %) | 102 | 146 | 6 |
| Acrobat Pro (everything below Studio) | 737 | 337 (45 %) | 128 | 239 | 33 |
| Master PDF Editor | 437 | 284 (64 %) | 68 | 66 | 19 |
| Only in Katna PDF | 61 | 32 | 5 | 22 | 2 |

Many of Acrobat Pro's open rows are accessibility tagging (49 rows, nothing built
yet) and OCR (25 rows, deferred by the owner on 2026-10-17). Without those two
areas, Pro parity is 335 of 656 shipped (51 %), 69 % with partial rows counted.

Out of scope (`skip`): Adobe's cloud services and accounts, shared reviews, Acrobat
Sign requests, PDF Spaces and the generative AI features, prepress (separations,
trapping, PDF/X, preflight fix-ups), 3D, Flash, dynamic XFA and the web app. Static XFA is deferred (owner, 2026-11-23). A
private on-device assistant is proposed instead (`idea.local-ai`).

To recount after editing the file:

```sh
awk -F'\t' 'NR>1{print $6}' parity/features.tsv | sort | uniq -c
awk -F'\t' 'NR>1 && $4=="reader" && $6!="shipped" && $6!="skip"{print $6"\t"$1"\t"$3}' parity/features.tsv
```

## What to build next for parity

**Acrobat Reader gaps people meet every day** (all `reader`, `todo`): find in
comments; rotate the view; the loupe; search comments, copy comment text,
eraser, area highlight, insert/replace-text proposals; autosave and revert; close
all; a keyboard shortcuts card and About; print in grayscale and from thumbnails;
fonts list in Properties; signing with the Windows certificate store and macOS
Keychain; self-signed digital IDs; the preferences window (identity, documents,
commenting, forms, full screen, search); screen-reader support (AccessKit).

**Master PDF Editor gaps** (`mpe` y, `katna` todo, 70 rows): autosave and backup
on save; status bar; rotate view; spell check; object tree
pane; edit inside form XObjects; links made from URLs in the text; a text box with
several fonts; split by size; bookmarks for combined or inserted files; drag pages
between files; scanner input (SANE / WIA); TIFF in and out, XPS, SVG; grayscale
conversion; mono (JBIG2/CCITT) compression; choose the PDF version; manual duplex
and print presets; custom shortcuts. Everything else MPE does, Katna PDF already
does, and it adds timestamps, LTV, certification, FieldMDP and the library, which
MPE lacks.

## Sources

- The 838 `acrobat` rows: ids and titles from PdfCraft's Acrobat Pro parity
  checklist (`parity/acrobat-features.toml` in
  [storytold/pdfcraft](https://github.com/storytold/pdfcraft), © the PdfCraft
  contributors, MIT OR Apache-2.0). Only the list was used, no code. The tiers,
  statuses and notes are ours.
- Adobe: [compare versions](https://www.adobe.com/acrobat/pricing/compare-versions.html),
  [Acrobat Reader](https://www.adobe.com/acrobat/pdf-reader.html),
  [what's new on desktop](https://helpx.adobe.com/acrobat/desktop/whats-new/whats-new-acrobat-desktop.html),
  [release notes](https://helpx.adobe.com/acrobat/desktop/whats-new/release-notes.html),
  [Acrobat blog, January 2026](https://blog.adobe.com/en/publish/2026/01/21/work-smarter-acrobat-turn-docs-presentations-podcasts-edit-pdfs-ai),
  [PDF Spaces sharing, May 2026](https://9to5mac.com/2026/05/06/adobe-acrobat-now-lets-users-turn-pdfs-into-shareable-ai-powered-workspaces/).
  Adobe does not publish a per-feature tier table in text, so `adobe` is our
  reading of those pages; Reader's commenting, filling, Fill & Sign, signing with
  a digital ID, checking signatures, read aloud and printing are free, editing
  and page tools start at Standard, and redaction, measuring, Bates, OCR,
  compare, PDF/A and accessibility tools are Pro.
- Master PDF Editor 5.9.99 (August 2026): its
  [online manual](https://code-industry.net/masterpdfeditor-help/), summarised in
  the project's `plan/master-pdf-editor-features.md`.
- Katna PDF: `docs/PLAN.md` and the merged PRs named in `where`.
