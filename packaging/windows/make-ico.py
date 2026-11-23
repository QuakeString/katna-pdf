#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes packaging/windows/katna-pdf.ico, the Windows icon of Katna PDF
and its Setup, and katna-pdf-128.png, the logo on Setup's window, from
packaging/icons/in.invenia.katna.Pdf.svg. Each size is drawn by
rsvg-convert (librsvg: `pacman -S librsvg`, `apt install librsvg2-bin`)
and packed with Pillow. Run again after the icon changes."""
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

here = Path(__file__).resolve().parent
svg = here.parent / "icons" / "in.invenia.katna.Pdf.svg"


def draw(size, folder):
    out = Path(folder) / f"{size}.png"
    subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(out), str(svg)],
        check=True,
    )
    return Image.open(out).convert("RGBA")


sizes = [16, 24, 32, 48, 64, 128, 256]
with tempfile.TemporaryDirectory() as folder:
    images = [draw(s, folder) for s in sizes]
    images[-1].save(
        here / "katna-pdf.ico",
        sizes=[(s, s) for s in sizes],
        append_images=images[:-1],
    )
    images[sizes.index(128)].save(here / "katna-pdf-128.png", optimize=True)
