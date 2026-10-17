# SPDX-License-Identifier: GPL-3.0-or-later
# Synthetic vector-heavy "CAD" sheets: many short strokes, as AutoCAD exports.
import random, zlib, sys
def make(path, w, h, n, seed):
    r = random.Random(seed); out = ["0.2 w 1 J 1 j"]
    # a grid of rooms with walls, hatching and dimension ticks
    for i in range(n):
        x, y = r.uniform(20, w-20), r.uniform(20, h-20)
        k = r.random()
        if k < 0.6:
            dx, dy = r.uniform(-15, 15), r.uniform(-15, 15)
            out.append(f"{x:.2f} {y:.2f} m {x+dx:.2f} {y+dy:.2f} l S")
        elif k < 0.9:
            pts = " ".join(f"{x+r.uniform(-8,8):.2f} {y+r.uniform(-8,8):.2f} l" for _ in range(6))
            out.append(f"{x:.2f} {y:.2f} m {pts} h S")
        else:
            out.append(f"{x:.2f} {y:.2f} m {x+5:.2f} {y+9:.2f} {x+12:.2f} {y+3:.2f} {x+14:.2f} {y-6:.2f} c S")
    data = zlib.compress("\n".join(out).encode(), 6)
    objs = [b"<< /Type /Catalog /Pages 2 0 R >>",
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R >>".encode(),
            f"<< /Length {len(data)} /Filter /FlateDecode >>\nstream\n".encode() + data + b"\nendstream"]
    pdf = bytearray(b"%PDF-1.7\n"); offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(pdf)); pdf += f"{i} 0 obj\n".encode() + o + b"\nendobj\n"
    x = len(pdf); pdf += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n".encode()
    for o in offs: pdf += f"{o:010d} 00000 n \n".encode()
    pdf += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    open(path, "wb").write(pdf)
make("cad-a1-100k.pdf", 2384, 1684, 100_000, 1)
make("cad-a0-400k.pdf", 3370, 2384, 400_000, 2)
make("cad-a0-1m.pdf", 3370, 2384, 1_000_000, 3)
