# Feature checklist

`features.tsv` is katna-pdf's feature checklist: 838 features of a full PDF workbench. Each one is mapped against [docs/PLAN.md](../docs/PLAN.md).

| Column | Meaning |
|---|---|
| `id`, `title` | the feature |
| `pdfcraft_tier` | priority in the source list |
| `verdict` | `covered` (in our plan), `partial` (touched, not fully), `missing` (fits our goals, not yet in the plan), `skip` (Adobe cloud, prepress, 3D or otherwise outside our goals) |
| `our_tier` | our priority: P0 MVP, P1 v1.0, P2 later |
| `where` | plan section or phase |
| `note` | why, when useful |

Totals (2026-10-16): 272 covered, 214 partial, 238 missing, 114 skip. Plan §4.8 adds the missing and partial P0/P1 items to the plan.

As features ship, add a `status` (planned / partial / shipped) and the test that proves it, so progress is measured from this file.

**Source and licence.** The feature ids and titles come from PdfCraft's Acrobat Pro parity checklist (`parity/acrobat-features.toml` in [storytold/pdfcraft](https://github.com/storytold/pdfcraft), © the PdfCraft contributors, MIT OR Apache-2.0). Only the list was used, no code. The verdicts, tiers and notes are katna-pdf's own.
