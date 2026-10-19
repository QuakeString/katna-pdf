#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Writes packaging/windows/katna-pdf.ico, the Windows icon of Katna PDF
and its Setup, and katna-pdf-128.png, the logo on Setup's window. Draws
packaging/icons/in.invenia.katna.Pdf.svg's shapes again with Pillow, each
size from eight times as large. Needs Pillow; run again after the icon
changes."""
from pathlib import Path

from PIL import Image, ImageDraw

here = Path(__file__).resolve().parent
TEAL = (0, 128, 128, 255)
PAPER = (249, 249, 249, 255)
FOLD = (179, 217, 217, 255)
OVER = 8


def draw(size):
    big = size * OVER
    k = big / 128
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(image)
    p = lambda *xy: [v * k for v in xy]
    d.ellipse(p(2, 2, 126, 126), fill=TEAL)
    # The page, its top right corner cut off for the fold.
    d.rounded_rectangle(p(36, 24, 96, 104), radius=4 * k, fill=PAPER)
    d.polygon(p(74, 23, 97, 23, 97, 46), fill=TEAL)
    d.polygon(p(74, 24, 96, 46, 78, 46, 74, 42), fill=FOLD)
    d.ellipse(p(74, 38, 82, 46), fill=FOLD)
    # Three lines of text, with round ends.
    w = 5 * k
    for x0, x1, y in [(48, 80, 62), (48, 80, 74), (48, 68, 86)]:
        d.line(p(x0, y, x1, y), fill=TEAL, width=round(w))
        for x in (x0, x1):
            d.ellipse(p(x - 2.5, y - 2.5, x + 2.5, y + 2.5), fill=TEAL)
    return image.resize((size, size), Image.LANCZOS)


sizes = [16, 24, 32, 48, 64, 128, 256]
images = [draw(s) for s in sizes]
images[-1].save(
    here / "katna-pdf.ico",
    sizes=[(s, s) for s in sizes],
    append_images=images[:-1],
)
images[sizes.index(128)].save(here / "katna-pdf-128.png", optimize=True)
