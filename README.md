# katna-pdf

A fast, beautiful PDF viewer, editor, signer and library manager in Rust and GPUI, part of the Katna family alongside [Katna Mail](https://github.com/QuakeString/katna).

- Acrobat Reader–class viewing and Master PDF Editor–class editing
- Indian DSC tokens, PAdES signing and Aadhaar eSign, all in pure Rust
- Day/night themes and Okular-style night reading, in Katna's look
- Fast zoom for CAD drawings, a PDF library and cloud drives through Katna

Linux and Windows first; macOS later. **Status:** Phase 0, a first window that opens, scrolls and zooms a PDF. See [docs/PLAN.md](docs/PLAN.md).

## Install on Arch Linux

CI builds `packaging/arch/PKGBUILD` on every push to `main` and puts the
package on the
[`arch-latest`](https://github.com/QuakeString/katna-pdf/releases/tag/arch-latest)
pre-release, which each build replaces. It is x86_64 only and not signed.

```sh
sudo pacman -U https://github.com/QuakeString/katna-pdf/releases/download/arch-latest/katna-pdf-git-x86_64.pkg.tar.zst
```

For updates with `pacman -Syu`, add this to `/etc/pacman.conf`:

```ini
[katna-pdf]
SigLevel = Optional TrustAll
Server = https://github.com/QuakeString/katna-pdf/releases/download/arch-latest
```

To build the package yourself: `cd packaging/arch && makepkg -si`.

## Build

On Linux, install `libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev libfreetype-dev libwayland-dev libx11-xcb-dev` (Debian/Ubuntu names), then:

```sh
cargo run -p katna-pdf -- file.pdf
cargo run --release -p katna-pdf-bench -- file.pdf   # open and draw times
```
