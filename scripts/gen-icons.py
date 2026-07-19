#!/usr/bin/env python3
"""Rasterize the app icon (crates/mxks-app/assets/icon.svg) without native deps.

The geometry below mirrors icon.svg exactly — a premium lime keyboard mark:
dark rounded background with a lime rim, a lime-gradient keyboard deck with a
drop edge and glossy sheen, dark key caps, and one bright accent "switch" key.
Pure stdlib: shapes are rendered as signed-distance rounded rectangles with
4x4 supersampling, then written as PNG (zlib) and ICO (PNG-compressed entries).

Outputs:
  crates/mxks-app/assets/icon-tray.png  (32x32 RGBA, embedded in the tray)
  crates/mxks-app/assets/icon.ico       (256..16, Windows executable icon)
"""

import struct
import sys
import zlib
from pathlib import Path

BASE = 512.0

def hex_rgb(s):
    return tuple(int(s[i : i + 2], 16) for i in (1, 3, 5))

def lerp(a, b, t):
    return a + (b - a) * t

def grad(stops, t):
    """stops: [(offset, (r,g,b)), ...] sorted by offset."""
    if t <= stops[0][0]:
        return stops[0][1]
    for (o0, c0), (o1, c1) in zip(stops, stops[1:]):
        if t <= o1:
            f = (t - o0) / (o1 - o0)
            return tuple(lerp(a, b, f) for a, b in zip(c0, c1))
    return stops[-1][1]

class RRect:
    """Rounded rect with a flat color, a vertical gradient, and an alpha."""

    def __init__(self, x, y, w, h, r, color=None, stops=None, alpha=1.0):
        self.x, self.y, self.w, self.h, self.r = x, y, w, h, r
        self.color, self.stops, self.alpha = color, stops, alpha

    def coverage(self, px, py):
        cx, cy = self.x + self.w / 2, self.y + self.h / 2
        hx, hy = self.w / 2 - self.r, self.h / 2 - self.r
        dx, dy = max(abs(px - cx) - hx, 0.0), max(abs(py - cy) - hy, 0.0)
        d = (dx * dx + dy * dy) ** 0.5 - self.r
        return min(max(0.5 - d, 0.0), 1.0)

    def color_at(self, py):
        if self.stops:
            t = (py - self.y) / self.h
            return grad(self.stops, min(max(t, 0.0), 1.0))
        return self.color

# Geometry: keep in sync with assets/icon.svg.
SHAPES = [
    RRect(0, 0, 512, 512, 116, color=hex_rgb("#3F6212")),
    RRect(5, 5, 502, 502, 111, stops=[(0.0, hex_rgb("#2A3A0E")), (1.0, hex_rgb("#0C1203"))]),
    RRect(48, 142, 416, 248, 48, color=hex_rgb("#365314")),
    RRect(
        48, 132, 416, 248, 48,
        stops=[(0.0, hex_rgb("#ECFCCB")), (0.5, hex_rgb("#A3E635")), (1.0, hex_rgb("#65A30D"))],
    ),
    RRect(62, 144, 388, 38, 19, color=(255, 255, 255), alpha=0.16),
]
KEY = hex_rgb("#1C2A08")
for kx in (78, 153, 228, 303, 378):
    SHAPES.append(RRect(kx, 170, 56, 48, 12, color=KEY))
for kx in (78, 153, 303, 378):
    SHAPES.append(RRect(kx, 234, 56, 48, 12, color=KEY))
SHAPES.append(RRect(78, 298, 56, 44, 12, color=KEY))
SHAPES.append(RRect(153, 298, 206, 44, 12, color=KEY))
SHAPES.append(RRect(378, 298, 56, 44, 12, color=KEY))
SHAPES.append(RRect(228, 234, 56, 48, 12, color=hex_rgb("#F7FEE7")))

def render(size, ss=4):
    """Render the icon at `size` px, ss x ss supersampled, -> RGBA bytes."""
    scale = BASE / size
    px = bytearray(size * size * 4)
    offs = [(i + 0.5) / ss for i in range(ss)]
    for j in range(size):
        for i in range(size):
            r = g = b = a = 0.0
            for oy in offs:
                for ox in offs:
                    sx, sy = (i + ox) * scale, (j + oy) * scale
                    sr = sg = sb = sa = 0.0
                    for shape in SHAPES:
                        cov = shape.coverage(sx, sy) * shape.alpha
                        if cov <= 0.0:
                            continue
                        cr, cg, cb = shape.color_at(sy)
                        sr = lerp(sr, cr, cov)
                        sg = lerp(sg, cg, cov)
                        sb = lerp(sb, cb, cov)
                        sa = sa + (1.0 - sa) * cov
                    r += sr
                    g += sg
                    b += sb
                    a += sa
            n = ss * ss
            k = (j * size + i) * 4
            px[k : k + 4] = bytes(
                min(255, round(v / n)) for v in (r, g, b, a * 255.0 / 1.0)
            )
    return bytes(px)

def to_png(rgba, size):
    raw = b"".join(
        b"\x00" + rgba[y * size * 4 : (y + 1) * size * 4] for y in range(size)
    )
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )

def to_ico(pngs):
    """pngs: [(size, png_bytes)] -> ICO with PNG-compressed entries."""
    header = struct.pack("<HHH", 0, 1, len(pngs))
    entries, blobs = b"", b""
    offset = len(header) + 16 * len(pngs)
    for size, png in pngs:
        b = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", b, b, 0, 0, 1, 32, len(png), offset)
        blobs += png
        offset += len(png)
    return header + entries + blobs

def main():
    assets = Path(__file__).resolve().parent.parent / "crates" / "mxks-app" / "assets"
    tray = render(32)
    (assets / "icon-tray.png").write_bytes(to_png(tray, 32))
    pngs = [(s, to_png(render(s), s)) for s in (256, 128, 96, 64, 48, 32, 24, 16)]
    (assets / "icon.ico").write_bytes(to_ico(pngs))
    print(f"wrote {assets / 'icon-tray.png'} and {assets / 'icon.ico'}")

if __name__ == "__main__":
    sys.exit(main())
