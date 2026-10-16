# katna-pdf: Product and Engineering Plan

*Draft 4, 2026-10-16 (adds the SumatraPDF speed study, the pure-Rust signing stack and the CAD drawing pipeline). No repository exists yet; this is the starting plan.*

katna-pdf is a cross-platform PDF **viewer, editor, signer and library manager** written in Rust with a GPUI interface. The targets are:

- **Viewing** on par with Acrobat Reader
- **Editing** on par with Master PDF Editor
- **Signing**: Indian DSC USB tokens plus standards-based PAdES digital signatures, with validation
- **Reading comfort**: day and night themes, and Okular-style page recoloring for night reading
- **Library**: a manager for the user's whole PDF collection

Platform priority: **Linux and Windows first**, with macOS supported from the same codebase.

---

## 1. Guiding principles

1. **Never corrupt a user's file.** Every save is either an incremental update or an atomic write: write to a temporary file, fsync it, then rename it into place. Signed documents are saved incrementally only, so existing signatures stay valid.
2. **Fast first paint.** A page should be on screen in under 150 ms on a typical document. Rendering runs in tiles, progressively, away from the UI thread.
3. **Hostile input is normal.** PDFs are untrusted. The renderer runs isolated so a crash cannot take down the app, and the parsers are fuzzed.
4. **One engine behind a trait.** The renderer is abstracted so it can be swapped later (PDFium today, possibly pure-Rust hayro tomorrow).
5. **Beautiful by default.** The design system (tokens, motion, typography) is built in Phase 0, not bolted on at the end.
6. **Keep the licensing permissive.** Every core dependency must be MIT, Apache-2.0, BSD or similar, so the project keeps its options open (see §3.4).

---

## 2. Architecture

### 2.1 Process model

```
┌──────────────────────────── katna (GPUI app process) ────────────────────────────┐
│  UI (GPUI views) ── App state / Commands / Undo ── Document model (katna-core)    │
│         │                         │                                              │
│         │                   katna-cos (object layer, incremental writer)          │
│         ▼                                                                        │
│  Tile cache (GPU textures) ◄── render client ──IPC──► katna-render-worker (xN)   │
│                                                        PDFium, sandboxed,        │
│  Signing service (pure Rust; token via APDU or vendor PKCS#11) restartable       │
│  Library service (SQLite + Tantivy + file watcher)                               │
└──────────────────────────────────────────────────────────────────────────────────┘
```

- **Render workers** are separate processes, each holding one PDFium instance. PDFium is not thread-safe, and it can crash on malformed files. A worker pool gives true parallelism and crash isolation. The MVP can start with one in-process render thread behind the same trait, then move to workers in Phase 2.
- **The object layer (`katna-cos`) is our own Rust code.** We parse and write PDF objects ourselves for anything that touches file structure: incremental saves, signing byte ranges, page operations, metadata, forms data and redaction. PDFium is used for rasterizing, text extraction, form rendering and high-level page-object editing.

### 2.2 Workspace and crate layout

```
katna-pdf/
├─ crates/
│  ├─ katna-app/          # binary: GPUI window, menus, routing, settings, single-instance
│  ├─ katna-ui/           # design system: theme tokens, components (wraps gpui-component), icons
│  ├─ katna-core/         # Document model, command bus, undo/redo, selection, autosave
│  ├─ katna-cos/          # PDF object layer: lexer/parser, xref/xref-streams, object streams,
│  │                      #   incremental writer, full rewrite, encryption (RC4/AES-128/AES-256 R6)
│  ├─ katna-content/      # content stream parser/writer, graphics state, text runs, path ops
│  ├─ katna-render/       # RenderBackend trait, PDFium backend, tile scheduler, recolor filters
│  ├─ katna-render-worker/# worker binary (IPC: length-prefixed postcard/bincode over pipes)
│  ├─ katna-text/         # extraction, search (incl. regex), reading order, font/shaping
│  │                      #   (rustybuzz/harfrust, skrifa, subsetter), Indic script support
│  ├─ katna-edit/         # text/image/vector editing, page ops, redaction, watermark, Bates
│  ├─ katna-annot/        # all annotation types: model, appearance-stream generation
│  ├─ katna-forms/        # AcroForm fill, field designer, calculations (JS subset)
│  ├─ katna-sign/         # CMS/PAdES builder, signature appearances, DocMDP/FieldMDP, TSA client
│  │  ├─ token-apdu       # pure-Rust card drivers (ePass2003, PIV, OpenPGP) over PC/SC or USB CCID
│  │  ├─ token-pkcs11     # fallback: load the token vendor's PKCS#11 module directly (cryptoki)
│  │  ├─ keystore-file    # pure-Rust PKCS#12 (.pfx/.p12)
│  │  ├─ trust-store      # katna's own trust store (bundled CCA India roots)
│  │  └─ keystore-os      # optional, off by default: Windows CNG store, macOS Keychain
│  ├─ katna-validate/     # signature validation: chain, OCSP/CRL, LTV/DSS, trust lists
│  ├─ katna-ocr/          # OCR backend trait: Tesseract (leptess) + ocrs (pure Rust)
│  ├─ katna-convert/      # images ⇄ PDF, PDF → text/HTML/Markdown, Office via LibreOffice (optional)
│  ├─ katna-library/      # SQLite catalog, Tantivy full-text index, watcher, thumbnails
│  ├─ katna-cli/          # headless `katna` CLI: merge/split/sign/verify/ocr/optimize (CI + power users)
│  └─ katna-testkit/      # corpus runners, golden-image diffs, fuzz targets
├─ assets/                # icons, fonts (Inter / Noto families), themes (*.toml)
├─ xtask/                 # build, fetch pdfium binaries, packaging, release
└─ docs/
```

### 2.3 Core internals

- **Document model.** `Document` owns a `CosDocument` (the parsed objects plus a revision chain) and a render handle. Edits are **Commands** (`apply`/`revert`) recorded on an undo stack, which also drives the "modified" state and autosave.
- **Saving.** The default is an incremental save, which appends a revision. "Save As optimized" does a full rewrite: garbage-collect unused objects, compress object streams and optionally linearize.
- **Tiles.** Pages are rendered as 256 or 512 px tiles at device scale and grouped into zoom buckets. An LRU cache keeps tiles as GPU textures under a memory budget. While the user zooms, the view shows the scaled lower-resolution tiles until crisp ones arrive.
- **Async work.** GPUI's executor runs async tasks. CPU-heavy jobs (OCR, compression, indexing) run on a rayon pool and report progress through a job center UI that can cancel them.

### 2.4 Speed design (lessons from SumatraPDF)

SumatraPDF is the benchmark for "opens instantly, big file or small". We studied its source (commit `9edfa44`, Oct 2026). Its speed comes from a set of deliberate habits, not from one trick. katna adopts each of them. *SumatraPDF is GPLv3, so we copy ideas, not code.*

**Startup**
- One small native binary with everything statically linked. No runtime to boot, no plugins scanned, no fonts enumerated at launch.
- The window appears first. Everything else is deferred to background tasks: update checks, recent-file thumbnails, folder scans, printer lists.
- **Single instance.** A second launch (for example double-clicking another PDF) hands the path to the already-running process and exits immediately, so a new tab opens with no process start cost. katna does the same over a local socket or named pipe.

**Opening a document**
- Files under 32 MB are read into memory in one go. Larger files are streamed from disk on demand, so a 2 GB scan opens as fast as a 2 MB one.
- At open time, only the cross-reference table and the page tree are read, plus each page's size from its page dictionary. **No page content is parsed until that page is about to be shown.** Outline, links, text and annotations are also loaded lazily.
- Opening runs off the UI thread. The tab appears immediately with page placeholders at their correct sizes, and the password prompt is asynchronous.

**Rendering**
- **Parse once, draw many times.** Each visible page's parsed content is cached (MuPDF "display lists"). Zooming, tiling and re-rendering then replay that cache instead of re-parsing the content stream. In PDFium the equivalent is keeping `FPDF_PAGE` objects loaded for visible and nearby pages in an LRU.
- **A small LIFO render queue.** At most 8 pending requests; the newest request (what is on screen now) is rendered first and the oldest is dropped when the queue is full. Visible pages are queued both before and after the predictive ones, so they always win.
- **Cancel aggressively.** Each in-flight render carries an abort flag. When the user scrolls or zooms away, the render stops mid-page. PDFium supports this through progressive rendering with a pause callback (`FPDF_RenderPageBitmap_Start`).
- **Predictive pre-render:** next and previous page, plus one more each way in two-page modes, at most 4 pages, anchored to what is visible.
- **Never show a blank page.** While the crisp tile renders, the cache paints the nearest bitmap it has at another zoom, stretched. Zoom feels instant and sharpens a moment later.
- Tiles are sized relative to the screen. If memory runs short, tile size is halved and rendering retried, instead of failing.
- A bounded bitmap cache (Sumatra uses 128 entries) evicts off-screen tiles first.
- **Parallelism:** up to 8 render threads, each with its own cloned engine context sharing the parsed document. PDFium cannot do this inside one process (see §3.1), which is why katna uses a pool of worker processes.

**Night mode**
- Sumatra's "smart" dark mode classifies each embedded image before deciding whether to recolor it. Page-sized images that look like scans (mostly bright paper) are recolored with the page; artwork and photos keep their colors. This confirms the smart night mode in §4.5 and gives us a tested heuristic: check border uniformity, brightness and bounding-box size.

**Performance budgets for katna** (measured in CI on a reference corpus, on a mid-range laptop):

| Metric | Budget |
|---|---|
| Cold start to empty window | < 250 ms |
| Open a 1,000-page PDF to first page visible | < 300 ms |
| Open a 1 GB scanned PDF to first page visible | < 500 ms |
| Second file via single-instance handoff | < 100 ms |
| Zoom change: stretched preview | next frame |
| Zoom change: sharp | < 150 ms per visible page |
| Idle memory with a 1,000-page PDF open | < 150 MB |

**Also notable:** SumatraPDF's new cross-platform edition ("ng", targeting Linux, macOS and WebAssembly) is built on **GPUI**, through a C++ port of it. A mature, speed-obsessed PDF reader picking GPUI is a strong signal that GPUI suits this kind of app.

### 2.5 CAD drawings and other vector-heavy pages

AutoCAD, Revit, SolidWorks and similar exports are small files that hold hundreds of thousands to millions of tiny line segments. In these files zooming in and out is the main thing users do, so it must feel instant.

**Why Okular struggles.** Okular (through Poppler) re-renders the whole page from scratch at every new zoom level. Each time, it re-parses the content stream and rasterizes every path, even the ones far off-screen, on a single thread. The cost grows with the number of paths, not with what is visible, so each zoom step on a dense drawing takes seconds.

**katna's approach: a dedicated drawing pipeline**

1. **Detect it automatically.** While a page loads, katna counts path operators. Above a threshold (for example 50,000 segments, or a high path-to-text ratio), the page is flagged "vector-heavy" and switches to the drawing pipeline. Ordinary documents never pay for it.
2. **Parse once into a scene.** The content stream is interpreted once into katna's own in-memory scene: paths (flattened to segments), fills, strokes, glyphs and images, each with its bounding box and layer (OCG). The scene is built in a background thread. It is pure Rust, using `hayro-interpret` (or our own content parser) as the interpreter.
3. **Spatial index.** Every scene item goes into an R-tree (the `rstar` crate). Drawing a tile at 3200 % then visits only the few thousand items inside that tile, not the million on the page. Deep zoom becomes faster, not slower.
4. **Level of detail.** For zoomed-out views, katna precomputes simplified versions of the scene:
   - Segments shorter than a pixel at that zoom are merged or dropped.
   - Dense hatching collapses to a filled tone.
   - Polylines are simplified with the Douglas–Peucker algorithm.

   An overview of a full A0 sheet draws from a few thousand items instead of millions, and looks the same.
5. **Map-style tile pyramid.** Tiles are rendered in parallel on all CPU cores, as in an online map. During pinch or wheel zoom, katna shows the nearest ready level immediately, scaled, then sharpens only the visible tiles. Tiles at the current zoom level and at one level above and below are pre-rendered in the background.
6. **A fast rasterizer.** Tiles are drawn with Linebender's `vello_cpu` (pure Rust, multithreaded, built for many small paths). `vello_cpu` is also what hayro renders with. Two later steps are planned:
   - `vello_hybrid` or GPU `vello` rendering straight to the screen. Zoom then becomes a pure GPU transform, as in real CAD viewers.
   - GPU-tessellated hairlines through `lyon`.
7. **Interaction quality vs final quality.** While a zoom gesture is in motion, tiles render without anti-aliasing and with simplified line caps. When motion stops, they re-render at full quality, typically within the next 100–200 ms.
8. **Drawing controls:**
   - Toggle layers (most CAD PDFs carry OCG layers per CAD layer). Hiding a layer also skips it in the index.
   - "Line weights" on/off, like Acrobat: thin hairlines draw faster and are often easier to read.
   - Black-on-white, white-on-black and monochrome modes for plotting styles.

**Features this unlocks for drawings** (P1):
- Measuring tools that **snap** to line endpoints, midpoints and intersections, found through the R-tree.
- Scale calibration from the drawing's title block.
- Area and perimeter from a clicked closed shape.
- Coordinates readout.
- Compare two revisions of a drawing as a colored overlay.

**Budgets for vector-heavy pages** (reference: a 1-million-segment A0 drawing):

| Metric | Budget |
|---|---|
| Open to first full-sheet view | < 600 ms |
| Any zoom step: scaled preview | next frame |
| Any zoom step: sharp visible tiles | < 250 ms |
| Pan at 3200 % | 60 fps, no blank tiles |
| Scene and index memory | < 300 MB |

---

## 3. Engine choices

### 3.1 Rendering engine

| Option | License | Quality / coverage | Editing support | Verdict |
|---|---|---|---|---|
| **PDFium** via `pdfium-render` | BSD-3 / Apache-2.0 (binding MIT/Apache) | Chrome-grade; AcroForm, annotations, text extraction, page objects | Page-object API: add, move or delete text, image and path objects; save | **Primary choice** |
| MuPDF (`mupdf` crate) | **AGPL-3.0** or paid commercial license | Excellent and fast; good redaction and OCR hooks | Good | Strong engine, but AGPL forces the whole app to be AGPL. Keep as an optional backend only if the app goes AGPL |
| hayro (pure Rust) | Apache/MIT | Young but improving fast; no forms | None | Watch it. It is the long-term option for a pure-Rust renderer behind the same trait |
| pdf.js / Poppler | Apache / GPL | — | — | Poppler is GPL and C++; not a good fit |

**Speed note.** MuPDF is the engine behind SumatraPDF's speed. It is faster and lighter than PDFium, its parsed-page cache can be replayed from many threads in one process, and its editing, redaction and signing APIs are richer. PDFium is single-threaded per process, so katna needs a worker-process pool to match. The price of MuPDF is the AGPL license (§3.4).

**Decision:** use PDFium by default, and settle it with data. Phase 0 benchmarks both engines behind the `RenderBackend` trait against the §2.4 budgets. If MuPDF wins clearly and you are happy for katna to be AGPL, we switch to it. Prebuilt binaries come from `bblanchon/pdfium-binaries`, are fetched by `xtask`, and ship next to the executable. Build without V8 or XFA at first (smaller, safer). A `RenderBackend` trait keeps hayro and MuPDF possible later.

### 3.2 Object-level and writing libraries

- **`katna-cos` (our own code)**, informed by `lopdf`. We want precise control over incremental updates and byte-exact signature ranges, plus repair of broken xref tables. `lopdf` is a good reference and a fallback for early prototypes.
- **`pdf-writer`** (Typst) for generating new objects and **`krilla`** for building new PDFs from scratch (forms, conversions, PDF/A output).
- **`subsetter`, `skrifa`, `ttf-parser`, `rustybuzz`/`harfrust`** for font subsetting, embedding and shaping. These are critical for text editing and for Indic scripts (Devanagari, Bengali, Tamil and others).
- **RustCrypto**: `aes`, `cbc`, `sha2`, `rsa`, `p256`/`p384`, `cms`, `x509-cert`, `der`, `spki`, `x509-ocsp`, `x509-tsp`.

### 3.3 Other key dependencies

| Need | Choice |
|---|---|
| GUI | `gpui` (Zed, Apache-2.0), pinned to a git revision; **`gpui-component`** (Longbridge, Apache-2.0) for inputs, tables, docks, menus and color pickers |
| Signing crypto (all pure Rust) | RustCrypto `cms`, `x509-cert`, `der`, `pkcs12`, `rsa`, `p256`/`p384`, `sha2`; no OpenSSL, no NSS |
| Smart-card transport | `pcsc` (talks to the OS smart-card service) and `nusb` (pure-Rust USB, for a built-in CCID reader driver on Linux) |
| Vendor-token fallback | `cryptoki` (loads the token vendor's PKCS#11 module directly; no p11-kit, no NSS) |
| Optional OS stores | `windows` crate (CNG), `security-framework` (macOS); off by default |
| HTTP (TSA/OCSP/CRL) | `reqwest` with rustls (pure Rust TLS); katna's own root store, no system crypto library |
| OCR | `leptess` (Tesseract 5, with `hin`, `ben`, `tam` and other Indic traineddata) with `ocrs` as a pure-Rust fallback |
| Catalog | `rusqlite` (bundled), `tantivy` (full-text search), `notify` (folder watcher), `blake3` (deduplication) |
| Text-to-speech | `tts` crate (SAPI / Speech Dispatcher / AVSpeech) |
| Settings | `serde` + TOML in XDG / AppData / Application Support |
| Logging and crash reports | `tracing`; minidumps via `crash-handler` + `minidumper` (opt-in) |

### 3.4 Licensing recommendation

The license and the engine are now one decision:

- **Apache-2.0 OR MIT with PDFium:** anyone can reuse katna's code, including in closed products. Speed comes from the §2.4 techniques plus worker processes.
- **AGPL-3.0 with MuPDF** (the SumatraPDF route): katna stays fully open source and anyone distributing a modified version must publish it. It gets the fastest engine and the richest editing API.

Without benchmark data, the default is to license the app itself as **Apache-2.0 OR MIT**, or **GPL-3.0** if you want copyleft. Both work with PDFium. Choosing MuPDF would force AGPL or a paid license. That is the single biggest licensing fork in the plan, and this plan avoids it.

---

## 4. Feature scope

Priority tags: **P0** = MVP, **P1** = v1.0, **P2** = after v1.0.

### 4.1 Viewer (Acrobat Reader parity)

- **P0:** Open, tabs and multiple windows. Zoom: fit page, fit width, actual size, marquee and pinch zoom. Rotate the view. Page display modes: single, continuous, two-up and two-up with cover page. Smooth kinetic scrolling.
- **P0:** Sidebar panels for thumbnails, outline/bookmarks, attachments and layers (OCG).
- **P0:** Text selection and copy. Find in document with case, whole-word and regex options and a result list. Links and named destinations. History back and forward.
- **P0:** Fill and save AcroForm forms. Show annotations. Password-protected PDFs (open and print).
- **P0:** Print through the native dialog: Windows XPS/GDI, Linux GTK/CUPS, macOS NSPrint.
- **P1:** Full-screen presentation mode with page transitions. Reading mode that hides chrome. Auto-scroll.
- **P1:** Measuring tools (distance, perimeter, area, with scale calibration). Snapshot tool. Magnifier loupe.
- **P1:** Read aloud (TTS). Search across many files (through the library index).
- **P1:** Document properties: fonts, security, PDF/A and PDF/UA flags, producer and similar. Embedded files and portfolios (collections).
- **P1:** Compare two documents, side by side and as a diff overlay.
- **P1:** Accessibility: keyboard-only use, high-contrast themes, a tagged-PDF reading order view. Screen-reader support depends on GPUI's AccessKit work (see Risks).
- **P2:** A limited JavaScript subset (form calculations, formatting and validation) via `boa`. Multimedia and rich media. 3D (U3D/PRC) is **out of scope**, and so is dynamic XFA. Static XFA is rendered through its AcroForm fallback.

### 4.2 Annotation and review

- **P0:** Highlight, underline, strikeout, squiggly. Sticky notes. Free text. Ink/pen. Rectangle, ellipse, line, arrow, polygon, polyline. Stamps (built-in and custom).
- **P1:** A comments panel with replies, statuses, filters, and export/import as FDF/XFDF. Callouts. File-attachment annotations. Measurement annotations.
- **P1:** Flatten annotations. Summarize comments into a PDF.
- **Requirement:** every annotation has a generated appearance stream, so it looks correct in every other viewer.

### 4.3 Editor (Master PDF Editor parity)

**Page operations (P0):** insert, delete, extract, reorder by drag in the thumbnail grid, rotate, crop with a crop/trim/bleed box editor, split by range, size or bookmarks, merge, duplicate, insert a blank page or a page from another file or from an image.

**Content editing (P1, the hardest area):**
- **Edit text in place.** Parse content streams into text runs, map glyphs back to Unicode (ToUnicode and encodings), re-shape the edited run, then re-encode with the original font when its glyphs cover the change. Otherwise fall back to a matching system font, subset and embed it. Line, paragraph and reflow editing works within a text block.
- **Add text** with font, size, color, alignment and character spacing.
- **Images:** insert, replace, move, resize, crop, rotate, delete and export.
- **Vector objects:** select, move, recolor, change stroke and fill, and draw shapes.
- **Object inspector:** z-order, opacity, blend mode, alignment and distribution guides, snapping.
- **Edit links, bookmarks and named destinations.**

**Document-level editing:**
- **P1:** Headers and footers; watermark; background; Bates numbering; page labels.
- **P1:** Metadata editor (Info and XMP). Document properties and initial view settings.
- **P1:** Security: password-protect (AES-256), change permissions, remove protection when the password is known.
- **P1: True redaction.** Search-and-redact and pattern redaction (PAN, Aadhaar, phone numbers, email). It removes the underlying text, image pixels and vector content, not just draws boxes on top. It also sanitizes metadata, hidden layers, JavaScript and attachments.
- **P1:** Forms designer: text, checkbox, radio, combo, list, button and signature fields. Tab order. Calculation scripts. Import and export form data (FDF/XFDF/JSON/CSV).
- **P1:** OCR scanned documents into an invisible searchable text layer. Deskew and despeckle.
- **P1:** Optimize and compress: downsample images, recompress with JPEG/JPEG2000/JBIG2, subset fonts, remove unused objects, linearize for fast web view.
- **P2:** Layer (OCG) editing. Edit embedded files. Document JavaScript editor.
- **P2:** PDF/A-1b/2b/3b conversion and validation. Validation can call veraPDF when it is installed. PDF/UA checks.
- **P2:** Convert PDF to images, text, HTML or Markdown, and images to PDF. Office ⇄ PDF goes through an installed LibreOffice. Batch processing ("Actions") over many files.

### 4.4 Signing and validation

This is a first-class feature, built for India and international use alike.

**Everything is built in and pure Rust**

katna depends on no external key store or crypto library. That means no Firefox/NSS database (which Okular uses), no OpenSSL, no Java signer and no browser extension. Everything below is katna's own Rust code:

- The CMS/PAdES signature builder, hashing and RSA/ECDSA verification
- PKCS#12 (`.pfx`/`.p12`) parsing and software-key signing
- katna's own trust store, shipped with the CCA India roots and managed inside the app
- TSA, OCSP and CRL clients over rustls
- All validation logic, and the PIN entry dialog

**Talking to the USB token: the one unavoidable boundary**

A DSC token's private key never leaves the token. The token signs a hash itself, so katna must speak to it. katna does this in three tiers, tried in order:

1. **Native token drivers, pure Rust (preferred).** katna talks to the token directly with smart-card commands (APDUs). This needs no vendor software at all. It works for tokens whose card protocol is known:
   - **ePass2003** (Feitian): the protocol, including its secure-messaging layer, is publicly documented through the OpenSC project. We reimplement it in Rust from the documentation, not by copying OpenSC's LGPL code.
   - **Standard PIV and OpenPGP cards**, such as YubiKey and Nitrokey.

   The transport is the operating system's own smart-card service. That is WinSCard, part of Windows; pcsc-lite on Linux; and the built-in smart-card framework on macOS. These are OS components, not apps. On Linux, katna can also drive the USB reader directly through a built-in pure-Rust CCID driver, for systems without `pcscd`.

2. **The token vendor's PKCS#11 driver (fallback).** ProxKey, mToken, HyperPKI and SafeNet eToken use closed, undocumented card protocols. For them, katna loads the vendor's driver library directly in-process. This is the same driver that ships on the token and that every signing app needs. katna uses no p11-kit, no NSS and no middleware in between. Known modules:

   | Token | Windows | Linux |
   |---|---|---|
   | ePass2003 (Feitian) | `eps2003csp11.dll` | `libcastle.so.1.0.0` |
   | ProxKey (Watchdata) | `SignatureP11.dll` / `wdpkcs.dll` | `libwdpkcs_*.so` |
   | mToken (Longmai) | `CryptoIDA_pkcs11.dll` | `libcryptoid_pkcs11.so` |
   | HYP2003 / HyperPKI | `HypersecurP11.dll` | `libhyp2003*.so` |
   | SafeNet eToken 5110 | `eTPKCS11.dll` | `libeToken.so` |

   *Filenames vary by driver version, so they live in an easily updated data file, not in code. Users can also point katna at a custom module.* Each token moved into tier 1 removes one vendor dependency. Adding native drivers for more Indian tokens is ongoing work after v1.0 and depends on how open each vendor's protocol is.

3. **The OS certificate store (optional, off by default).** Windows CNG and the macOS Keychain, for users who prefer them. katna never requires them.

**UX:** an auto-detected token list that shows which tier each token uses, a certificate picker (subject, issuer, validity, key usage, class), secure PIN entry, and clear errors for "token locked", "wrong PIN, N tries left" and "no driver for this token: built-in support planned / install the vendor driver".

**Creating signatures**
- **P0:** Approval signatures, **PAdES B-B**: `ETSI.CAdES.detached` CMS with the signing-certificate-v2 attribute, SHA-256/384/512, and RSA PKCS#1 v1.5, RSA-PSS or ECDSA keys. Draw a new field or sign an existing one. Saved incrementally.
- **P0:** Visible signature appearances: name, date, reason, location, a "Digitally signed by…" text block, an optional image or handwritten-signature image, and saved appearance templates. A configurable layout follows the familiar Indian government and e-filing style.
- **P1:** **PAdES B-T** with an RFC 3161 timestamp. The timestamp server list is configurable, with free TSAs preset and an option for paid ones (eMudhra, Capricorn and others).
- **P1:** **PAdES B-LT / B-LTA**: embed the DSS dictionary (certificates, OCSP responses, CRLs, VRI) and add document timestamps.
- **P1:** **Certification signatures** (DocMDP with permission levels 1, 2 and 3). Field locking (FieldMDP). Signature-field "lock after signing".
- **P1:** **Batch signing** of many files with a single PIN entry, from the GUI and from `katna sign` in the CLI.
- **P2:** Aadhaar **eSign** through a licensed ESP's API (needs a commercial ESP agreement, so it is an optional plugin). A local signing bridge for web portals (like emSigner) is a later candidate.

**Validation (`katna-validate`)**
- **P0:** Check integrity (byte range and digest). Detect changes made after signing by comparing revisions. Show the signature panel with a green, yellow or red status and a human-readable explanation.
- **P1:** Build the certificate chain from embedded, DSS and AIA sources. Check revocation through OCSP, CRL and embedded responses. Evaluate at signing time or timestamp time. Check DocMDP permission violations. "View signed version" of any revision.
- **Trust:** bundle the **CCA India root certificates** (Root CA of India 2011, 2014 and 2022), with optional system trust and optional AATL/EUTL import. The user can manage trusted roots. Never trust a self-signed certificate implicitly.
- **P1:** Export a validation report as PDF and JSON.

### 4.5 Themes and night reading

**App themes**
- Light, Dark, and "Follow system". The OS appearance is detected live on Windows (registry/UISettings), on Linux (freedesktop portal `color-scheme`), and on macOS.
- Accent color picker. Optional translucency on platforms that support it (Mica on Windows 11, blur on KDE and macOS).
- Themes are TOML token files (`surface`, `surface-raised`, `text-primary`, `accent`, …), so the community can share themes. Ship polished presets: *Katna Light*, *Katna Dark*, *Sepia Paper*, *Nord*, *Solarized*, *High Contrast*.

**Page recoloring (Okular parity and beyond)**

Each mode below is a per-pixel filter applied to rendered tiles, either on the CPU with SIMD and rayon or as a GPU pass if GPUI exposes custom shaders. Filters are cached per tile, so switching modes is instant.

| Mode | Description |
|---|---|
| Invert colors | Plain RGB inversion |
| Invert lightness | Inverts L in OKLab and keeps hue, so red stays red. This is the best default for dark mode |
| Invert luma | sRGB-correct and gamma-corrected variants, as in Okular |
| Paper color | Tint white to a chosen paper color (sepia, gray) |
| Dark & light colors | Map black to color A and white to color B, e.g. light-gray text on charcoal |
| Black & white | Threshold with a contrast slider |
| Hue shift | Rotate hues |

On top of the filters:
- **Smart night mode** (beyond Okular): recolor text and vector content but **keep photos in natural color**. Image-object bounding boxes from PDFium's page-object API become a mask, and the original pixels are restored inside it.
- **Auto mode:** recolor when the app is in dark theme. Per-document overrides are remembered in the library.
- Brightness and contrast sliders and a warm "blue-light" filter, with a quick toggle in the toolbar and a keyboard shortcut.

### 4.6 Library / manager

- **P0:** Recent files with thumbnails. Reading progress (last page and zoom per document). Pinned and favorite files.
- **P1:** **Library view.** Watched folders are indexed automatically. Grid and list layouts with cover thumbnails. Sort and filter by title, author, date, size, page count, tags, "signed", "has forms" or "scanned".
- **P1:** Tags, collections and **smart collections** (saved queries). Star ratings. Notes per document.
- **P1:** Full-text search across the whole library (Tantivy, with OCR text included) and jump to the hit.
- **P1:** Duplicate detection (content hash plus a fuzzy text hash). Missing-file relinking when files move.
- **P1:** Bulk operations from the library: merge, compress, OCR, sign, verify, rename by metadata pattern, export.
- **P2:** Import citation metadata (DOI lookup), BibTeX export, simple reading statistics. Optional sync of the catalog file through any cloud folder; no account system.

### 4.7 Look and feel

The aim is Okular's functionality with a cleaner, modern design:

- A unified title bar with tabs (custom-drawn client-side decorations on Wayland and Windows), a collapsible left sidebar (thumbnails, outline, annotations, signatures, layers), and a right inspector that appears only while editing.
- **Mode switcher:** *Read · Comment · Edit · Sign · Organize*. Each mode shows only its own tools, which avoids Acrobat's toolbar overload.
- A **command palette** (Ctrl/Cmd+K) that reaches every action, and fully remappable shortcuts with Okular and Acrobat presets.
- Soft shadows under pages, smooth 120 Hz scrolling, and subtle 150–200 ms motion. Inter for UI text and Noto for scripts, including Indic. Lucide-style icon set.
- Drag-and-drop everywhere: files onto the window, pages between documents, images into pages.
- Optional localization from day one through Fluent (`fluent-rs`). Hindi and Bengali are good early additions.

---

## 5. Platform delivery

| Platform | Packages | Notes |
|---|---|---|
| **Linux** | Flatpak (Flathub), AppImage, `.deb`, `.rpm`, AUR | GPUI renders with Vulkan (Blade) on Wayland and X11. **Flatpak and USB tokens:** needs `--device=all` or pcscd socket access. Native-driver tokens then work inside the sandbox; vendor PKCS#11 tokens also need the host's driver library exposed. Document that native packages are the smoothest route for DSC users. |
| **Windows 10/11** | MSI (WiX) and MSIX, winget | GPUI's DirectX 11 backend. Code-sign the installer with an OV/EV certificate (SmartScreen). Register as a PDF handler and add a thumbnail provider (P2). |
| **macOS 12+** | Notarized `.dmg`, Homebrew cask | Metal backend. Universal binary. |

**CI:** GitHub Actions on a matrix of ubuntu, windows and macos. Run `cargo nextest`, golden-image render tests, `cargo deny` (licenses and advisories), `cargo fuzz` smoke runs, and nightly builds.

---

## 6. Testing strategy

- **Corpus:** CAD exports (AutoCAD, Revit, SolidWorks, QCAD, LibreCAD) with segment counts from 100k to 5M, pdf.js and PDFium test PDFs, the Isartor and veraPDF corpora, Arlington PDF Model checks, plus a private folder of real-world Indian PDFs (government forms, e-filing acknowledgements, signed GST and MCA documents).
- **Golden-image tests** for rendering and recolor filters, using a perceptual diff threshold.
- **Round-trip tests:** after each edit, the result must re-open in PDFium, `qpdf --check` and Acrobat. Signed output must validate in Adobe Acrobat, EU DSS and pyHanko.
- **Fuzzing:** `cargo-fuzz` on the `katna-cos` lexer and parser, content-stream parser, CMS parser and xref repair.
- **Hardware matrix for DSC:** at least ePass2003, ProxKey and mToken on Windows and Ubuntu, tested manually per release. SoftHSM2 runs in CI for automated PKCS#11 tests, and a virtual smart card (vsmartcard / jCardSim with a PIV applet) tests the native APDU path.

---

## 7. Phased roadmap

Durations below assume 1–2 full-time developers and are rough sizing, not commitments.

### Phase 0: Foundations (≈4–6 weeks)
- Workspace, CI, `xtask` fetching PDFium binaries, `cargo deny`.
- GPUI spike: window, tabs, a scrolling page list with tiled images on **Linux and Windows**. **This confirms the GUI choice early.**
- Engine bake-off: PDFium and MuPDF behind `RenderBackend`, measured against the §2.4 budgets on the same corpus.
- CAD prototype: render a real AutoCAD-exported drawing through the §2.5 pipeline (scene, R-tree, `vello_cpu` tiles) and compare its zoom times against PDFium, MuPDF and Okular.
- Single-instance handoff, lazy open (page sizes only), LIFO render queue with cancellation.
- Design tokens, light and dark themes, base components.
- `katna-cos` v0: parse, xref streams, incremental append writer.
- **Milestone M0:** open a PDF and scroll it smoothly on Linux and Windows, in light and dark themes, within the §2.4 budgets. Engine and license decided from the bake-off.

### Phase 1: MVP viewer (≈8–10 weeks)
- Tile renderer and cache, zoom modes, page display modes, thumbnails, outline, links, search, text selection and copy.
- Night-reading filters (all Okular modes) plus smart night mode.
- Vector-heavy drawing pipeline (§2.5): detection, scene with R-tree, level of detail, parallel tile pyramid.
- Form filling and save, basic printing, password-protected files.
- Recent files and reading progress.
- **Milestone M1, public alpha (v0.1):** "the prettiest PDF reader on Linux".

### Phase 2: Annotate and organize (≈8 weeks)
- All P0 annotation types with appearance streams, a comments panel and XFDF.
- Page operations: organizer grid, merge, split, insert, rotate, crop.
- Render workers moved to separate processes.
- `katna` CLI (merge, split, info).
- **Milestone M2 (v0.3):** a daily driver for reading and reviewing.

### Phase 3: Signing (≈8–10 weeks)
- Pure-Rust signing core, PKCS#12, native ePass2003 and PIV drivers, vendor PKCS#11 fallback for other Indian tokens.
- PAdES B-B and B-T, visible appearances, signature panel and validation (integrity, chain, revocation, CCA roots).
- Batch signing, and `katna sign` / `katna verify` in the CLI.
- Interop sign-off against Acrobat, EU DSS and pyHanko.
- **Milestone M3 (v0.5):** DSC signing works for real Indian workflows (GST, MCA, ITR attachments, tenders).

### Phase 4: Editing (≈12–16 weeks)
- Content-stream editor: text editing and adding, images, vectors, object inspector.
- Headers, footers, watermark, Bates numbering, metadata, security (AES-256).
- True redaction. Forms designer.
- **Milestone M4 (v0.8):** Master PDF Editor core parity.

### Phase 5: Library, OCR and polish to v1.0 (≈8–10 weeks)
- Library view, watched folders, tags, smart collections, full-text search, duplicate detection.
- OCR with Indic languages. Optimize and compress.
- PAdES B-LT/B-LTA, certification signatures.
- Compare documents, measuring tools with CAD snapping, read aloud, presentation mode.
- Packaging and installers on all three platforms, auto-update, crash reporting (opt-in).
- **Milestone M5: v1.0.**

### After v1.0
PDF/A and PDF/UA, conversions, batch "Actions", JavaScript subset, Aadhaar eSign plugin, a hayro backend experiment, plugin API, and mobile or tablet layouts if GPUI grows touch support.

---

## 8. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| **GPUI is pre-1.0**: API churn, thin docs, and Zed's needs come first | High | Pin a git revision and upgrade on a schedule. Keep GPUI code inside `katna-app`/`katna-ui`; the core crates stay GUI-agnostic. Lean on `gpui-component`. The M0 spike is the go/no-go gate. |
| **GPUI accessibility** (screen readers) is still immature | Medium | Track the upstream AccessKit work. Make keyboard-only use and high contrast solid now. Contribute upstream if needed. |
| **GPUI printing / IME / file dialogs** have gaps on some platforms | Medium | Use `rfd` for dialogs and platform-native print code paths. Test Indic IME input early. |
| **In-place text editing is hard**: subset fonts, odd encodings, missing glyphs | High | Scope it per text block. Fall back to font substitution with a visible warning. Build a large regression corpus. Ship it last (Phase 4). |
| **PDFium is slower than MuPDF and single-threaded per process** | Medium | §2.4 techniques plus a worker-process pool. The Phase 0 bake-off decides whether to switch engines. |
| **Our drawing pipeline renders differently from the main engine** (blend modes, patterns, shading edge cases) | Medium | Use it only on flagged vector-heavy pages, fall back to the main engine for any unsupported feature on that page, and compare golden images against PDFium in CI. |
| **PDFium crashes or is not thread-safe** | Medium | Out-of-process workers with automatic restart. Never share an instance across threads. |
| **DSC driver quirks** (vendor PKCS#11 bugs, 32/64-bit DLL mismatch, missing Linux drivers) | High for the Indian market | Native pure-Rust drivers where the protocol is known, a data-driven vendor module list otherwise, a "Diagnose token" tool, and a real-hardware test matrix. |
| **Closed token protocols** keep some tokens tied to the vendor driver | Medium | Tier 2 fallback always works. Native drivers are added token by token, only from public documentation or the vendor's cooperation. |
| **Flatpak sandbox blocks USB tokens** | Medium | Document the needed permissions. Recommend native `.deb`/`.rpm` for signers. Test p11-kit forwarding. |
| **Signature interop** (Acrobat shows "validity unknown" and similar) | High | Strict PAdES conformance and validation against Acrobat, DSS and pyHanko in CI. Embed the full chain and revocation data by default. |
| **Feature scope is enormous** | High | Strict P0/P1/P2 gates. Each phase ships something usable. The CLI allows early automated testing. |
| **Licensing drift** (one AGPL or GPL dependency slipping in) | Medium | `cargo deny` license policy in CI. |
| **Code-signing and notarization costs** | Low | Budget for a Windows OV/EV certificate and an Apple Developer account before v1.0. |

---

## 9. Open decisions for Mozammel

1. **App license and engine:** Apache-2.0/MIT with PDFium, or AGPL-3.0 with MuPDF? *Recommendation: let the Phase 0 bake-off decide. Default is Apache/MIT with PDFium; switch to AGPL with MuPDF only if it is clearly faster and you are fine with AGPL.*
2. ~~**Repository:**~~ Done: github.com/QuakeString/katna-pdf.
3. **DSC hardware on hand:** which token(s) do you own? That decides the first native driver we build (ePass2003 is the easiest) or the first vendor module we test against.
4. **Aadhaar eSign:** is it wanted at all? It needs a commercial ESP agreement.
