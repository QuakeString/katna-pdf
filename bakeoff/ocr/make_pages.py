# SPDX-License-Identifier: GPL-3.0-or-later
"""Makes the OCR bake-off's test pages: known text in English, Hindi and
Bengali, drawn as a clean page, a flatbed scan and a phone photo.

    python3 -I make_pages.py OUT_DIR

Writes OUT_DIR/<lang>-<n>-<kind>.png (or .jpg) with OUT_DIR/<lang>-<n>.txt
beside them, the text each page shows, one line per line. Needs Pillow with
raqm (for Indic shaping), numpy and the Noto fonts.
"""

import random
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = Path(__file__).resolve().parent
NOTO = Path("/usr/share/fonts/truetype/noto")
FONTS = {
    "eng": ["NotoSans-Regular.ttf", "NotoSerif-Regular.ttf"],
    "hin": ["NotoSansDevanagari-Regular.ttf", "NotoSerifDevanagari-Regular.ttf"],
    "ben": ["NotoSansBengali-Regular.ttf", "NotoSerifBengali-Regular.ttf"],
}
PAGES = 4  # per language
DPI = 200
WIDTH, HEIGHT = int(8.27 * DPI), int(11.69 * DPI)  # A4
MARGIN = int(0.9 * DPI)


def wrap(draw, words, font, width):
    lines, line = [], ""
    for word in words:
        trial = f"{line} {word}".strip()
        if line and draw.textlength(trial, font=font) > width:
            lines.append(line)
            line = word
        else:
            line = trial
    if line:
        lines.append(line)
    return lines


def page(lang, n, rng):
    sentences = (HERE / "text" / f"{lang}.txt").read_text().split("\n")
    sentences = [s for s in sentences if s]
    rng.shuffle(sentences)
    face = FONTS[lang][n % 2]
    points = [10, 11, 12, 14][n % 4]
    font = ImageFont.truetype(str(NOTO / face), int(points * DPI / 72))
    img = Image.new("L", (WIDTH, HEIGHT), 255)
    draw = ImageDraw.Draw(img)
    step = int(font.size * 1.6)
    y, shown = MARGIN, []
    for paragraph in [sentences[i : i + 3] for i in range(0, len(sentences), 3)]:
        for line in wrap(draw, " ".join(paragraph).split(), font, WIDTH - 2 * MARGIN):
            if y + step > HEIGHT - MARGIN:
                return img, shown
            draw.text((MARGIN, y), line, font=font, fill=0)
            shown.append(line)
            y += step
        y += step // 2
    return img, shown


def scan(img, rng):
    """A flatbed scan: a slight tilt, a little blur, grain, grey paper, JPEG."""
    img = img.rotate(rng.uniform(-1.2, 1.2), resample=Image.BICUBIC, fillcolor=255)
    img = img.filter(ImageFilter.GaussianBlur(0.6))
    a = np.asarray(img, dtype=np.float32) * 0.92 + 8
    a += np.random.default_rng(rng.randrange(1 << 30)).normal(0, 6, a.shape)
    return Image.fromarray(a.clip(0, 255).astype(np.uint8))


def photo(img, rng, width=1200, blur=0.5, dark=0.25):
    """A phone photo: keystoned, unevenly lit, smaller, softer, in colour.
    `width` is the page's width in the photo, in pixels."""
    w, h = img.size
    d = lambda: rng.uniform(0.02, 0.06)
    # Corners of the page inside the photo, as fractions.
    quad = [
        (d() * w, d() * h),
        (d() * w, (1 - d()) * h),
        ((1 - d()) * w, (1 - d()) * h),
        ((1 - d()) * w, d() * h),
    ]
    out = Image.new("L", (w, h), 90)
    # Map the photo back to the page with Pillow's quad transform.
    page_in_photo = img.transform(
        (w, h),
        Image.QUAD,
        data=sum(_invert_quad(quad, w, h), ()),
        resample=Image.BICUBIC,
        fillcolor=90,
    )
    out.paste(page_in_photo)
    a = np.asarray(out, dtype=np.float32)
    yy, xx = np.mgrid[0:h, 0:w]
    light = 1 - dark + dark * np.cos((xx / w - rng.uniform(0.2, 0.8)) * 2.2) * np.cos(
        (yy / h - rng.uniform(0.2, 0.8)) * 1.8
    )
    a = a * light
    a += np.random.default_rng(rng.randrange(1 << 30)).normal(0, 4, a.shape)
    out = Image.fromarray(a.clip(0, 255).astype(np.uint8))
    scale = width / w  # 1200: about 145 dpi across the page
    out = out.resize((width, int(h * scale)), Image.BILINEAR).filter(ImageFilter.GaussianBlur(blur))
    tint = Image.merge("RGB", [out.point(lambda v: min(255, v * f)) for f in (1.0, 0.97, 0.9)])
    return tint


def _invert_quad(quad, w, h):
    # Image.QUAD samples the source at the quad for each output corner; to
    # put the whole page inside `quad` we sample a source quad that is the
    # page grown by the inverse of each corner's inset.
    (x0, y0), (x1, y1), (x2, y2), (x3, y3) = quad
    def grow(x, y, cx, cy):
        # Corner (cx, cy) of the output maps from (x, y) in the photo; the
        # source point is that corner pushed outwards by the inset.
        return (cx - (x - cx) * 1.0, cy - (y - cy) * 1.0)
    return [grow(x0, y0, 0, 0), grow(x1, y1, 0, h), grow(x2, y2, w, h), grow(x3, y3, w, 0)]


def main():
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for lang in FONTS:
        for n in range(PAGES):
            rng = random.Random(f"{lang}-{n}")
            img, lines = page(lang, n, rng)
            (out / f"{lang}-{n}.txt").write_text("\n".join(lines) + "\n")
            img.save(out / f"{lang}-{n}-clean.png")
            scan(img, rng).save(out / f"{lang}-{n}-scan.jpg", quality=75)
            photo(img, rng).save(out / f"{lang}-{n}-photo.jpg", quality=70)
            # A quick snap from further away in a dim room: about 100 dpi.
            photo(img, rng, width=830, blur=0.8, dark=0.45).save(
                out / f"{lang}-{n}-snap.jpg", quality=55
            )
            print(lang, n, len(lines), "lines")


if __name__ == "__main__":
    main()
