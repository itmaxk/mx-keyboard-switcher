#!/usr/bin/env python3
"""Generate the app and tray icons without native deps.

The geometry below is the single source of truth: the app icon is written as
SVG (assets/icon.svg) and rasterized here too, so the two cannot drift.

The mark is two keycaps: the active layout's key in front, the other behind.
Shapes are signed-distance rounded rectangles and round-capped strokes (lines
and circular arcs), composited source-over in premultiplied alpha with 4x4
supersampling, then written as PNG (zlib) and ICO (PNG-compressed entries).

Outputs (crates/mxks-app/assets):
  icon.svg                 app icon (EN behind, RU in front)
  icon.ico                 256..16, Windows executable icon
  tray-{en,ru,none}.png    32x32 RGBA tray icons: active layout in front;
                           `none` is shown when the layout is neither EN nor RU
  tray-{en,ru,none}-template.png
                           32x32 macOS template variants: the menu bar uses
                           only their alpha, so letters are cut out
"""

import math
import struct
import sys
import zlib
from pathlib import Path

BASE = 512.0


def hex_rgb(s):
    return tuple(int(s[i : i + 2], 16) for i in (1, 3, 5))


def grad(stops, t):
    """stops: [(offset, (r,g,b)), ...] sorted by offset."""
    if t <= stops[0][0]:
        return stops[0][1]
    for (o0, c0), (o1, c1) in zip(stops, stops[1:]):
        if t <= o1:
            f = (t - o0) / (o1 - o0)
            return tuple(a + (b - a) * f for a, b in zip(c0, c1))
    return stops[-1][1]


class Shape:
    """Common paint: a flat color or a vertical gradient over the shape's own
    bounds, an alpha, and an op ("over" paints, "erase" cuts to transparent)."""

    def __init__(self, color=None, stops=None, alpha=1.0, op="over"):
        self.color, self.stops, self.alpha, self.op = color, stops, alpha, op

    def color_at(self, py):
        if self.stops:
            t = (py - self.y) / self.h
            return grad(self.stops, min(max(t, 0.0), 1.0))
        return self.color

    def coverage(self, px, py):
        x0, y0, x1, y1 = self.bbox
        if px < x0 or px > x1 or py < y0 or py > y1:
            return 0.0
        return min(max(0.5 - self.distance(px, py), 0.0), 1.0)


class RRect(Shape):
    def __init__(self, x, y, w, h, r, **paint):
        super().__init__(**paint)
        self.x, self.y, self.w, self.h, self.r = x, y, w, h, r
        self.bbox = (x - 1, y - 1, x + w + 1, y + h + 1)

    def distance(self, px, py):
        cx, cy = self.x + self.w / 2, self.y + self.h / 2
        hx, hy = self.w / 2 - self.r, self.h / 2 - self.r
        dx, dy = max(abs(px - cx) - hx, 0.0), max(abs(py - cy) - hy, 0.0)
        return math.hypot(dx, dy) - self.r

    def svg(self, fill):
        return (
            f'<rect x="{self.x:g}" y="{self.y:g}" width="{self.w:g}" height="{self.h:g}" '
            f'rx="{self.r:g}" fill="{fill}"{svg_alpha(self.alpha, "opacity")}/>'
        )


class Stroke(Shape):
    """Round-capped strokes of `width` along segments: ("L", x0, y0, x1, y1)
    or ("A", cx, cy, r, a0, a1), an arc from angle a0 to a1 (degrees, y down,
    increasing angle = clockwise on screen)."""

    def __init__(self, segments, width, **paint):
        super().__init__(**paint)
        self.segments, self.width = segments, width
        xs, ys = [], []
        for seg in segments:
            if seg[0] == "L":
                xs += [seg[1], seg[3]]
                ys += [seg[2], seg[4]]
            else:
                _, cx, cy, r, _, _ = seg
                xs += [cx - r, cx + r]
                ys += [cy - r, cy + r]
        pad = width / 2 + 1
        self.bbox = (min(xs) - pad, min(ys) - pad, max(xs) + pad, max(ys) + pad)

    def distance(self, px, py):
        return min(seg_distance(seg, px, py) for seg in self.segments) - self.width / 2

    def svg(self, fill):
        d = []
        for seg in self.segments:
            if seg[0] == "L":
                d.append(f"M{seg[1]:g} {seg[2]:g} L{seg[3]:g} {seg[4]:g}")
            else:
                _, cx, cy, r, a0, a1 = seg
                (sx, sy), (ex, ey) = arc_point(cx, cy, r, a0), arc_point(cx, cy, r, a1)
                large = 1 if a1 - a0 > 180 else 0
                d.append(f"M{sx:g} {sy:g} A{r:g} {r:g} 0 {large} 1 {ex:g} {ey:g}")
        return (
            f'<path d="{" ".join(d)}" fill="none" stroke="{fill}" stroke-width="{self.width:g}" '
            f'stroke-linecap="round"{svg_alpha(self.alpha, "stroke-opacity")}/>'
        )


def arc_point(cx, cy, r, angle):
    a = math.radians(angle)
    return round(cx + r * math.cos(a), 3), round(cy + r * math.sin(a), 3)


def seg_distance(seg, px, py):
    if seg[0] == "L":
        _, x0, y0, x1, y1 = seg
        vx, vy = x1 - x0, y1 - y0
        t = ((px - x0) * vx + (py - y0) * vy) / (vx * vx + vy * vy)
        t = min(max(t, 0.0), 1.0)
        return math.hypot(px - (x0 + t * vx), py - (y0 + t * vy))
    _, cx, cy, r, a0, a1 = seg
    angle = math.degrees(math.atan2(py - cy, px - cx))
    while angle < a0:
        angle += 360
    if angle <= a1:
        return abs(math.hypot(px - cx, py - cy) - r)
    return min(
        math.hypot(px - x, py - y) for x, y in (arc_point(cx, cy, r, a0), arc_point(cx, cy, r, a1))
    )


def svg_alpha(alpha, attr):
    return "" if alpha >= 1.0 else f' {attr}="{alpha:g}"'


# Glyphs drawn as strokes in a box (x, y, w, h), so they look the same
# everywhere and need no font.
def glyph(ch, x, y, w, h):
    if ch == "E":
        return [("L", x, y, x, y + h), ("L", x, y, x + w, y),
                ("L", x, y + h / 2, x + w * 0.8, y + h / 2), ("L", x, y + h, x + w, y + h)]
    if ch == "N":
        return [("L", x, y + h, x, y), ("L", x, y, x + w, y + h), ("L", x + w, y + h, x + w, y)]
    if ch == "R":
        r = h * 0.3
        bx = x + w - r
        return [("L", x, y + h, x, y), ("L", x, y, bx, y), ("A", bx, y + r, r, -90, 90),
                ("L", bx, y + 2 * r, x, y + 2 * r), ("L", x + w * 0.46, y + 2 * r, x + w, y + h)]
    if ch == "U":
        r = w / 2
        return [("L", x, y, x, y + h - r), ("A", x + r, y + h - r, r, 0, 180),
                ("L", x + w, y + h - r, x + w, y)]
    raise ValueError(ch)


def label(text, x, y, w, h, gap):
    """Segments for `text` in letter boxes w x h starting at (x, y)."""
    segments = []
    for i, ch in enumerate(text):
        segments += glyph(ch, x + i * (w + gap), y, w, h)
    return segments


LIME = [(0.0, hex_rgb("#ECFCCB")), (0.5, hex_rgb("#A3E635")), (1.0, hex_rgb("#65A30D"))]
INK = hex_rgb("#142008")
LABEL = {"en": "EN", "ru": "RU"}


def app_icon():
    """Rounded tile, EN keycap behind, RU keycap in front."""
    return [
        RRect(0, 0, 512, 512, 116, color=hex_rgb("#3F6212")),
        RRect(5, 5, 502, 502, 111, stops=[(0.0, hex_rgb("#2A3A0E")), (1.0, hex_rgb("#0C1203"))]),
        RRect(72, 108, 240, 212, 52, color=hex_rgb("#1F2E08")),
        RRect(72, 88, 240, 212, 52, color=hex_rgb("#4D7C0F")),
        Stroke(label("EN", 120, 125, 40, 65, 22), 16, color=hex_rgb("#A3E635"), alpha=0.75),
        # Dark gap that lifts the front key off the back one.
        RRect(188, 188, 264, 266, 62, color=INK),
        RRect(200, 220, 240, 220, 52, color=hex_rgb("#365314")),
        RRect(200, 200, 240, 212, 52, stops=LIME),
        Stroke(label("RU", 262, 266, 50, 80, 26), 20, color=INK),
    ]


# Tray geometry, drawn for 32 px (16 units per pixel) with no tile so the
# letters get the most room: the back key peeks out top-left, the front key
# holds the active layout. A transparent gap separates them on any panel.
BACK = (8, 8, 320, 272, 72)
GAP = (152, 144, 376, 368, 96)
FRONT_FACE = (176, 168, 328, 304, 72)
FRONT_SIDE = (176, 200, 328, 304, 72)


def front_label(text):
    return label(text, 228, 248, 96, 144, 32)


def back_label(text):
    return label(text, 40, 40, 54, 80, 20)


def tray_icon(front, back):
    shapes = [RRect(*BACK, color=hex_rgb("#4D7C0F"))]
    if back:
        shapes.append(Stroke(back_label(LABEL[back]), 22, color=hex_rgb("#D9F99D")))
    shapes += [
        RRect(*GAP, color=(0, 0, 0), op="erase"),
        RRect(*FRONT_SIDE, color=hex_rgb("#365314")),
        RRect(*FRONT_FACE, stops=LIME),
    ]
    if front:
        shapes.append(Stroke(front_label(LABEL[front]), 36, color=INK))
    return shapes


def tray_template(front, back):
    """macOS template: black with alpha; the inactive key is dimmer."""
    black = (0, 0, 0)
    shapes = [RRect(*BACK, color=black, alpha=0.5)]
    if back:
        shapes.append(Stroke(back_label(LABEL[back]), 22, color=black, op="erase"))
    shapes += [
        RRect(*GAP, color=black, op="erase"),
        RRect(176, 168, 328, 336, 72, color=black),
    ]
    if front:
        shapes.append(Stroke(front_label(LABEL[front]), 36, color=black, op="erase"))
    return shapes


def render(shapes, size, ss=4):
    """Render `shapes` at `size` px, ss x ss supersampled, -> RGBA bytes."""
    scale = BASE / size
    px = bytearray(size * size * 4)
    offs = [(i + 0.5) / ss for i in range(ss)]
    n = ss * ss
    for j in range(size):
        for i in range(size):
            r = g = b = a = 0.0
            for oy in offs:
                for ox in offs:
                    sx, sy = (i + ox) * scale, (j + oy) * scale
                    pr = pg = pb = pa = 0.0
                    for shape in shapes:
                        cov = shape.coverage(sx, sy) * shape.alpha
                        if cov <= 0.0:
                            continue
                        keep = 1.0 - cov
                        if shape.op == "erase":
                            pr, pg, pb, pa = pr * keep, pg * keep, pb * keep, pa * keep
                            continue
                        cr, cg, cb = shape.color_at(sy)
                        pr, pg, pb = cr * cov + pr * keep, cg * cov + pg * keep, cb * cov + pb * keep
                        pa = cov + pa * keep
                    r, g, b, a = r + pr, g + pg, b + pb, a + pa
            k = (j * size + i) * 4
            if a <= 0.0:
                px[k : k + 4] = b"\0\0\0\0"
            else:
                px[k : k + 4] = bytes(
                    min(255, round(v)) for v in (r / a, g / a, b / a, a / n * 255.0)
                )
    return bytes(px)


def to_svg(shapes):
    defs, body = [], []
    for shape in shapes:
        if shape.op != "over":
            raise ValueError("SVG output supports only painted shapes")
        if shape.stops:
            gid = f"g{len(defs)}"
            stops = "".join(
                f'<stop offset="{o:g}" stop-color="#{c[0]:02X}{c[1]:02X}{c[2]:02X}"/>'
                for o, c in shape.stops
            )
            defs.append(f'<linearGradient id="{gid}" x1="0" y1="0" x2="0" y2="1">{stops}</linearGradient>')
            fill = f"url(#{gid})"
        else:
            c = shape.color
            fill = f"#{c[0]:02X}{c[1]:02X}{c[2]:02X}"
        body.append("  " + shape.svg(fill))
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 512 512">\n'
        "  <!-- Generated by scripts/gen-icons.py; edit the geometry there. -->\n"
        f"  <defs>{''.join(defs)}</defs>\n" + "\n".join(body) + "\n</svg>\n"
    )


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
    app = app_icon()
    (assets / "icon.svg").write_text(to_svg(app))
    pngs = [(s, to_png(render(app, s), s)) for s in (256, 128, 96, 64, 48, 32, 24, 16)]
    (assets / "icon.ico").write_bytes(to_ico(pngs))
    for front, back in (("en", "ru"), ("ru", "en"), (None, None)):
        name = front or "none"
        (assets / f"tray-{name}.png").write_bytes(to_png(render(tray_icon(front, back), 32), 32))
        (assets / f"tray-{name}-template.png").write_bytes(
            to_png(render(tray_template(front, back), 32), 32)
        )
    print(f"wrote icon.svg, icon.ico and tray icons to {assets}")


if __name__ == "__main__":
    sys.exit(main())
