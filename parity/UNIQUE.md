# What only Katna PDF does

Features neither Adobe Acrobat (any plan) nor Master PDF Editor offers, or offers
only in a much weaker form. Each has a row in [features.tsv](features.tsv); the
`where` column there names the PR. "Proposed" items are ideas, not yet agreed.

## Shipped

**Signing for India, in pure Rust**
- India's CCA roots and e-Mudhra intermediates built in, so Indian DSC signatures
  check as trusted out of the box (`sign.india-cca-roots`, #48). Acrobat needs the
  roots imported by hand; MPE leans on the system store or NSS.
- USB DSC tokens (ePass2003 and other makers' PKCS#11 drivers, OpenSC) on Linux
  and Windows, with a "Find out why" check of the service, device and driver when a
  token is not found (`sign.token-diagnose`, #56). Acrobat has no Linux build.
- PAdES B-T, B-LT and B-LTA with timestamps, OCSP and CRL on Linux
  (`sign.ltv-everywhere`, #45, #51). MPE has no timestamps or LTV at all.
- A signature check written out as a PDF report (`sign.verify-report-pdf`).
- Signing and checking in pure Rust: no NSS, no OpenSSL, no external key store
  (`sign.pure-rust-crypto`).

**A library, not just a viewer**
- Watched folders catalogued with covers, title, author, pages, signed, scanned
  and locked; every page of every file searchable at once (`library.catalog`,
  `library.full-text`, #76, #78).
- Smart collections, stars, tags and notes; copies found by bytes or by words;
  many files renamed from their properties, merged, signed, checked or made smaller
  in one go (`library.*`, #79).
- PDFs attached to mail in Katna Mail, and Google Drive and OneDrive through
  Katna's accounts, with Ctrl+S saving back to the drive (#80, #81, #89).

**Drawings and reading**
- Huge CAD drawings: a 1M-stroke sheet shows in 0.56 s and zooms from a grid
  index (`view.cad-fast`, #30); zoom to 10,000 %, the part on screen always drawn
  sharp (`view.zoom-10000`, #120; Acrobat stops at 6,400 %).
- Okular-style page colours: night that keeps photos, invert lightness, sepia,
  black and white (`view.night-keeps-photos`, #29). Acrobat has one
  replace-colours setting.
- Pages drawn in worker processes, so a hostile or broken file never takes the
  app down (`view.worker-isolation`, #38).
- Read aloud on Linux through Speech Dispatcher or espeak-ng (#85); KDE global
  menu (#43).

**Safer by design**
- Form totals and number, date and percent formats done in Rust from Acrobat's
  own calls, with no script engine (`form.calc-without-js`, #67).
- Every script and action a file carries is listed and none is ever run
  (`protect.scripts-never-run`, #109).
- PAN, Aadhaar and Indian phone numbers found for redaction
  (`protect.redact-india-ids`, #64).
- Smaller files are checked to draw the same before they are kept
  (`optimize.checked-same`, #82).

**For people who script**
- A command line for every feature with `--json` output for scripts and AI
  agents (`misc.cli-json`, #35). MPE's command line only merges.
- Free and open source (GPL-3.0-or-later), Linux first, in-app updates including
  the Arch package (#92), one family with Katna Mail and Calendar.

## Proposed

These are ideas for the owner to accept or drop. Each is a `todo` row with
`source` katna and `note` proposed.

- **Signed QR checks** on GST e-invoices and e-way bills (`idea.gst-einvoice-qr`)
  and on e-Aadhaar, e-PAN and DigiLocker papers (`idea.signed-qr-ids`): say at a
  glance whether a paper is genuine.
- **Aadhaar eSign** (`sign.aadhaar-esign`, already planned; needs an ASP).
- **DigiLocker**: open and check issued documents (`idea.digilocker`).
- **Indic text editing** with proper shaping for Hindi and other scripts
  (`idea.indic-editing`), where Acrobat is weak.
- **Version timeline**: every saved version of a file kept in the library with
  its changes drawn, like a history of the document (`idea.version-timeline`).
- **Sign and reply** to a mailed PDF in Katna Mail in one step (`idea.mail-sign-reply`).
- **Folder actions**: watched folders that sign, stamp, compress or convert new
  files (`idea.folder-actions`).
- **Private AI**: summaries and questions on the computer itself, nothing sent
  out, off by default (`idea.local-ai`).
- **CAD helpers**: the scale read from the title block, layer-aware snapping, a
  sheet index (`idea.cad-tools`).
- **D-Bus interface** so Katna apps and scripts can drive the open window
  (`idea.dbus-api`).
- **Live Text**: select and search words inside any picture (`idea.live-text`;
  with OCR, deferred).
