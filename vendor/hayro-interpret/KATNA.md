# hayro-interpret, patched for Katna PDF

This is `hayro-interpret` 0.8.0 from crates.io (Apache-2.0 OR MIT, see
`LICENSE-APACHE` and `LICENSE-MIT`). It is used through `[patch.crates-io]`
in the workspace `Cargo.toml`, beside our patched `vendor/hayro`
(`vendor/hayro/KATNA.md`). Its examples are left out.

The first commit that added this directory holds the crate unchanged, so
`git diff` against that commit shows the whole patch. When hayro is
upgraded, copy the new version here and apply the same changes, or drop a
patch once upstream has it.

## Huge images drawn small (src/x_object/decode/mod.rs)

`with_rgba` and `with_stencil` take a hint of the size the image will be
drawn at, but only JPEG 2000 used it. Every other image was expanded to a
byte per sample at full size, and the renderer then resampled it down.

`pdf.js`'s `issue16263.pdf` is a PowerPoint export. Its soft mask is a
34,862 × 4,332 one-bit scan (151 megapixels) shown 1,178 pixels wide.
Expanding and resampling it took about 160 ms each time, and the page draws
it 40 times.

Now an image of 4 megapixels or more, drawn at least twice smaller on both
axes, is averaged down by that whole factor straight from its packed samples
while decoding. It comes out as 8-bit samples, and the scale factors cover
the difference. One-bit images count set bits a byte at a time. Images at 1,
2, 4 and 8 bits are handled. Indexed images and images with a colour-key
`/Mask` are left alone, because averaging their samples would change their
meaning. Averaging is exact for the decode array, since decoding is linear.

`StencilImage` also gains `paint()` and `stream()` (src/types.rs), so the
renderer can keep a decoded stencil and draw it again
(`vendor/hayro/KATNA.md`).

Upstream issue text, for hayro's tracker: "The `target_dimension` hint is
only used for JPX. For other images drawn far below their size,
`decode_context` could average k×k blocks of the packed samples into 8-bit
samples before expansion (k = the whole factor on both axes). For
issue16263.pdf, a 151-megapixel one-bit soft mask shown at 1,178 px, this
avoids a 151 MB expansion and the resampling pass."
