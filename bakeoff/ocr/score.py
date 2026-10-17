# SPDX-License-Identifier: GPL-3.0-or-later
"""Runs each OCR engine over the test pages and scores what it read.

    python3 -I score.py PAGES_DIR MODELS_DIR > results.json

PAGES_DIR comes from make_pages.py. MODELS_DIR holds ocrs's
text-detection.rten and text-recognition.rten, and PaddleOCR's det.onnx,
rec.onnx and keys.txt (the recogniser's dictionary). Tesseract must be on
PATH with eng, hin and ben installed. Prints a table to stderr.

Character error rate (CER) is the edit distance between what an engine
read and the page's text, over the length of the text, with all
whitespace collapsed to single spaces and both sides in Unicode NFC; it
also counts lines read out of order. "Words" is the share of the page's
words read exactly, in any order.
"""

import json
import os
import subprocess
import sys
import time
import unicodedata
from collections import Counter
from pathlib import Path

from rapidfuzz.distance import Levenshtein

HERE = Path(__file__).resolve().parent
HARNESS = HERE / "target" / "release" / "katna-pdf-ocr-bakeoff"
TESSERACT_LANG = {"eng": "eng", "hin": "hin", "ben": "ben"}


def norm(text):
    return " ".join(unicodedata.normalize("NFC", text).split())


def cer(read, truth):
    truth = norm(truth)
    return Levenshtein.distance(norm(read), truth) / max(1, len(truth))


def word_accuracy(read, truth):
    """Words of the truth that were read, in any order: unlike CER, not
    hurt by lines read in the wrong order."""
    truth = Counter(norm(truth).split())
    read = Counter(norm(read).split())
    return sum((truth & read).values()) / max(1, sum(truth.values()))


def harness(args):
    out = subprocess.run([str(HARNESS), *args], check=True, capture_output=True, text=True)
    return [json.loads(line) for line in out.stdout.splitlines()]


def tesseract(image, lang):
    started = time.perf_counter()
    out = subprocess.run(
        ["tesseract", str(image), "stdout", "-l", TESSERACT_LANG[lang], "--psm", "3"],
        check=True,
        capture_output=True,
        text=True,
    )
    return {"file": str(image), "ms": (time.perf_counter() - started) * 1000, "text": out.stdout}


def main():
    pages, models = Path(sys.argv[1]), Path(sys.argv[2])
    images = sorted(p for p in pages.iterdir() if p.suffix in (".png", ".jpg"))
    english = [str(p) for p in images if p.name.startswith("eng-")]
    runs = {"tesseract": [tesseract(p, p.name[:3]) for p in images]}
    runs["ocrs"] = harness(
        ["ocrs", str(models / "text-detection.rten"), str(models / "text-recognition.rten"), *english]
    )
    runs["pp-ocr"] = harness(
        ["ppocr", str(models / "det.onnx"), str(models / "rec.onnx"), str(models / "keys.txt"), *english]
    )
    results = []
    for engine, rows in runs.items():
        for row in rows:
            name = Path(row["file"]).name
            lang, n, kind = name.split(".")[0].split("-")
            truth = (pages / f"{lang}-{n}.txt").read_text()
            results.append(
                {
                    "engine": engine,
                    "lang": lang,
                    "page": int(n),
                    "kind": kind,
                    "ms": round(row["ms"]),
                    "cer": round(cer(row["text"], truth), 4),
                    "words": round(word_accuracy(row["text"], truth), 4),
                    "text": row["text"],
                }
            )
    json.dump(results, sys.stdout, ensure_ascii=False, indent=1)
    table(results)


def table(results):
    groups = {}
    for r in results:
        groups.setdefault((r["lang"], r["kind"], r["engine"]), []).append(r)
    print(f"{'lang':5} {'kind':6} {'engine':10} {'CER %':>7} {'words %':>8} {'ms/page':>8}", file=sys.stderr)
    for (lang, kind, engine), rows in sorted(groups.items()):
        c = 100 * sum(r["cer"] for r in rows) / len(rows)
        words = 100 * sum(r["words"] for r in rows) / len(rows)
        ms = sum(r["ms"] for r in rows) / len(rows)
        print(f"{lang:5} {kind:6} {engine:10} {c:7.2f} {words:8.1f} {ms:8.0f}", file=sys.stderr)


if __name__ == "__main__":
    os.environ.setdefault("OMP_THREAD_LIMIT", str(os.cpu_count()))
    main()
