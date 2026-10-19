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
curl -LO https://github.com/QuakeString/katna-pdf/releases/download/arch-latest/katna-pdf-git-x86_64.pkg.tar.zst
sudo pacman -U katna-pdf-git-x86_64.pkg.tar.zst
```

(`pacman -U` straight from the URL fails, because it then also asks for
a signature file, which this package does not have.)

For updates with `pacman -Syu`, add this to `/etc/pacman.conf`:

```ini
[katna-pdf]
SigLevel = Optional TrustAll
Server = https://github.com/QuakeString/katna-pdf/releases/download/arch-latest
```

To build the package yourself: `cd packaging/arch && makepkg -si`.

## Install on other Linux systems

CI also builds a `.deb`, an `.rpm`, an AppImage and a plain tarball on
every push to `main`, tries each on its own system, and puts them on the
[`linux-latest`](https://github.com/QuakeString/katna-pdf/releases/tag/linux-latest)
pre-release. They are x86_64 only and need glibc 2.35 or later.

- Ubuntu 22.04+ and Debian 12+: download `katna-pdf-x86_64.deb`, then
  `sudo apt install ./katna-pdf-x86_64.deb`.
- Fedora: download `katna-pdf-x86_64.rpm`, then
  `sudo dnf install ./katna-pdf-x86_64.rpm`.
- Anywhere else: `chmod +x Katna-PDF-x86_64.AppImage` and run it, or
  unpack `katna-pdf-linux-x86_64.tar.gz` and run its `install.sh`.

## Install on Windows

[`windows-latest`](https://github.com/QuakeString/katna-pdf/releases/tag/windows-latest)
holds `KatnaPDF-windows-x86_64.zip`, built from `main`: unpack it anywhere
and run `katna-pdf.exe`. It is not signed yet, so SmartScreen asks the
first time (More info, then Run anyway).

## Build

On Linux, install `libxkbcommon-dev libxkbcommon-x11-dev libfontconfig-dev libfreetype-dev libwayland-dev libx11-xcb-dev` (Debian/Ubuntu names), then:

```sh
cargo run -p katna-pdf -- file.pdf
cargo run --release -p katna-pdf-bench -- file.pdf   # open and draw times
```

## Credits

This software is based in part on the work of the Independent JPEG Group
(through the `jpeg-encoder` crate, used when pictures are made smaller).
