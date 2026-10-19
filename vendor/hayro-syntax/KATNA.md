# hayro-syntax, patched for Katna PDF

This is `hayro-syntax` 0.8.0 from crates.io (Apache-2.0 OR MIT, see
`LICENSE-APACHE` and `LICENSE-MIT`). It is used through `[patch.crates-io]`
in the workspace `Cargo.toml`, beside our patched `vendor/hayro` and
`vendor/hayro-interpret` (their `KATNA.md`). Its examples are left out.

The first commit that added this directory holds the crate unchanged, so
`git diff` against that commit shows the whole patch. When hayro is
upgraded, copy the new version here and apply the same changes, or drop a
patch once upstream has it.

## Object streams' tables read once (src/xref.rs, src/data.rs)

An object stream starts with a table of its objects' numbers and offsets.
Each object taken from the stream read the whole table again, so taking
every object out of a stream of N cost N². The decoded stream was already
cached in `Data`; now its table is too (`Data::offsets_with`,
`ObjectStream::cached`). The repair paths, which read each stream once,
are unchanged.

`pdf.js`'s `bug1978317.pdf` (a Firefox print) has 7,736 link annotations
in object streams. Drawing, reading the text of or finding the pictures on
its one page took about 65 s each. It now takes 0.3 s.

Upstream issue text: "`ObjectStream::new` parses the offset table on every
`XRef::get_with` of an object in that stream, which is quadratic for pages
with many annotations in one object stream (pdf.js bug1978317.pdf: ~65 s
per page). Caching the parsed table per stream id next to the decoded data
in `Data` makes it 0.3 s."

## Page content in picture filters skipped (src/page.rs)

A content stream whose filters include `CCITTFaxDecode`, `JBIG2Decode`,
`DCTDecode` or `JPXDecode` is not content, and is now left out of
`Page::page_stream`. `pdf.js`'s `bomb_giant.pdf` puts a 100,000 ×
100,000 JBIG2 picture in a page's `/Contents`. Decoding it as content
filled 10 GB with white and took about 30 s before the page's text could
be read. It now takes well under a millisecond, with the same text.

## Checked

The first two pages of all 994 files in the pdf.js test corpus are drawn
at half size and their text read, before and after: every result is the
same, except that `bug1978317.pdf` now finishes.
