"""Renders Tine's brand icon set into ``assets/brand/``.

The mark: a tuning fork (a *tine* is one prong of a fork) whose U-shaped yoke
holds a glowing mic capsule, like a broadcast mic in its mount. Palette: Ember.

Outputs (all committed; re-run after changing a constant below):

* ``tine-icon.svg``          master app icon (plate + mark)
* ``tine-mark.svg``          plate-less mark for docs / README
* ``tine-<px>.png``          app icon at 16..1024 px
* ``tine.ico``               Windows icon: BMP frames <= 128 px, PNG 256 px
                             (``crates/tine-app/build.rs`` links it as the exe icon)
* ``tray-<state>-32.rgba``   raw RGBA tray glyphs, one per tray state
                             (``include_bytes!`` in ``crates/tine-app/src/tray.rs``)

Usage: ``python -I scripts/brand/render_icons.py`` (needs Pillow + NumPy).
"""

from __future__ import annotations

import io
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "assets" / "brand"
SS = 4  # supersampling per axis

# --- Ember palette (keep in sync with crates/tine-ui/src/theme.rs) --------------------
PLATE_TOP = "#352753"  # plate gradient, top-left
PLATE_BOTTOM = "#120D1F"  # plate gradient, bottom-right
YOKE = "#F6EFE7"  # warm ivory fork
CAP_TOP = "#FFC35C"  # capsule gradient: amber...
CAP_BOTTOM = "#FF5A4E"  # ...to coral

TRAY_STATES = {
    # state: (capsule top, capsule bottom, yoke)
    "active": (CAP_TOP, CAP_BOTTOM, YOKE),
    "bypassed": ("#9A93A8", "#6E6880", "#B9B3C4"),
    "error": ("#FF5C5C", "#D92B2B", "#8E879A"),  # dimmed fork: reads as "off" next to active
}

# --- Geometry, 256-unit square --------------------------------------------------------
PLATE = (8.0, 8.0, 248.0, 248.0, 60.0)  # x0, y0, x1, y1, corner radius
PRONG_L, PRONG_R, PRONG_TOP, ARC_CY, ARC_R = 84.0, 172.0, 58.0, 128.0, 44.0
STEM_TOP, STEM_BOTTOM = ARC_CY + ARC_R, 214.0
CAP_X, CAP_Y0, CAP_Y1 = 128.0, 66.0, 106.0


def hexc(h: str) -> np.ndarray:
    h = h.lstrip("#")
    return np.array([int(h[i : i + 2], 16) / 255 for i in (0, 2, 4)])


def sd_round_rect(x, y, x0, y0, x1, y1, r):
    cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
    qx = np.abs(x - cx) - ((x1 - x0) / 2 - r)
    qy = np.abs(y - cy) - ((y1 - y0) / 2 - r)
    return np.hypot(np.maximum(qx, 0), np.maximum(qy, 0)) + np.minimum(np.maximum(qx, qy), 0) - r


def sd_seg(x, y, ax, ay, bx, by, r):
    px, py, dx, dy = x - ax, y - ay, bx - ax, by - ay
    h = np.clip((px * dx + py * dy) / (dx * dx + dy * dy), 0, 1)
    return np.hypot(px - dx * h, py - dy * h) - r


def sd_lower_arc(x, y, cx, cy, big_r, r):
    ring = np.abs(np.hypot(x - cx, y - cy) - big_r) - r
    ends = np.minimum(np.hypot(x - (cx - big_r), y - cy), np.hypot(x - (cx + big_r), y - cy)) - r
    return np.where(y >= cy, ring, ends)


def mark_sdf(x, y, small: bool):
    """Signed distances of the yoke and the capsule. Small sizes get heavier strokes."""
    hw = 16.0 if small else 14.0
    yoke = np.minimum.reduce(
        [
            sd_seg(x, y, PRONG_L, PRONG_TOP, PRONG_L, ARC_CY, hw),
            sd_seg(x, y, PRONG_R, PRONG_TOP, PRONG_R, ARC_CY, hw),
            sd_lower_arc(x, y, 128.0, ARC_CY, ARC_R, hw),
            sd_seg(x, y, 128.0, STEM_TOP, 128.0, STEM_BOTTOM, hw),
        ]
    )
    cap = sd_seg(x, y, CAP_X, CAP_Y0, CAP_X, CAP_Y1, 23.0 if small else 22.0)
    return yoke, cap


def over(img, color, alpha):
    a = alpha[..., None]
    img[..., :3] = img[..., :3] * (1 - a) + color * a
    img[..., 3] = img[..., 3] + alpha * (1 - img[..., 3])


def render(px: int, plate: bool = True, colors=(CAP_TOP, CAP_BOTTOM, YOKE)) -> Image.Image:
    n = px * SS
    c = (np.arange(n) + 0.5) / n * 256.0
    x, y = np.meshgrid(c, c)
    unit = 256.0 / n  # one supersample in 256-units
    cov = lambda d: np.clip(0.5 - d / unit, 0, 1)  # noqa: E731
    small = px <= 32
    img = np.zeros((n, n, 4))
    cap_top, cap_bottom, yoke_c = (hexc(v) for v in colors)

    if plate:
        x0, y0, x1, y1, r = PLATE
        d = sd_round_rect(x, y, x0, y0, x1, y1, r)
        t = np.clip((x + y) / 512, 0, 1)[..., None]
        img[..., :3] = hexc(PLATE_TOP) * (1 - t) + hexc(PLATE_BOTTOM) * t
        img[..., 3] = cov(d)
        # Hairline rim: faint all round (so the plate separates from dark taskbars and
        # title bars), brighter along the top edge at large sizes.
        rim_w = max(1.0, 256.0 / px * 0.6)
        ring = cov(np.abs(d + rim_w) - rim_w)
        top = np.clip((128 - y) / 128, 0, 1) * (0.14 if px >= 48 else 0.0)
        over(img, np.ones(3), ring * (0.10 + top) * img[..., 3])

    yoke, cap = mark_sdf(x, y, small)
    if plate and px >= 48:  # warm glow around the capsule
        glow = np.clip(1 - np.maximum(cap, 0) / 30, 0, 1) ** 2 * 0.34
        over(img, cap_top, glow * img[..., 3])
    over(img, yoke_c, cov(yoke))
    t = np.clip((y - (CAP_Y0 - 22)) / 84, 0, 1)[..., None]
    over(img, cap_top * (1 - t) + cap_bottom * t, cov(cap))
    if px >= 48:  # specular highlight on the capsule
        hl = sd_seg(x, y, CAP_X - 9, CAP_Y0 - 4, CAP_X - 9, CAP_Y0 + 14, 4.5)
        over(img, np.ones(3), cov(hl) * 0.35)

    img = img.reshape(px, SS, px, SS, 4)
    a = img[..., 3].mean(axis=(1, 3))
    rgb = (img[..., :3] * img[..., 3:]).mean(axis=(1, 3)) / np.maximum(a, 1e-6)[..., None]
    out = np.concatenate([rgb, a[..., None]], axis=-1)
    return Image.fromarray(np.round(np.clip(out, 0, 1) * 255).astype(np.uint8), "RGBA")


def svg(plate: bool) -> str:
    x0, y0, x1, y1, r = PLATE
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">',
        "<defs>",
        f'<linearGradient id="plate" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{PLATE_TOP}"/>'
        f'<stop offset="1" stop-color="{PLATE_BOTTOM}"/></linearGradient>',
        f'<linearGradient id="cap" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{CAP_TOP}"/>'
        f'<stop offset="1" stop-color="{CAP_BOTTOM}"/></linearGradient>',
        f'<radialGradient id="glow"><stop offset="0" stop-color="{CAP_TOP}" stop-opacity="0.45"/>'
        f'<stop offset="1" stop-color="{CAP_TOP}" stop-opacity="0"/></radialGradient>',
        "</defs>",
    ]
    if plate:
        parts.append(f'<rect x="{x0:g}" y="{y0:g}" width="{x1 - x0:g}" height="{y1 - y0:g}" rx="{r:g}" fill="url(#plate)"/>')
        parts.append(f'<ellipse cx="{CAP_X:g}" cy="{(CAP_Y0 + CAP_Y1) / 2:g}" rx="56" ry="76" fill="url(#glow)"/>')
    parts.append(
        f'<path d="M{PRONG_L:g} {PRONG_TOP:g}V{ARC_CY:g}A{ARC_R:g} {ARC_R:g} 0 0 0 {PRONG_R:g} {ARC_CY:g}V{PRONG_TOP:g}'
        f'M128 {STEM_TOP:g}V{STEM_BOTTOM:g}" fill="none" stroke="{YOKE}" stroke-width="28" stroke-linecap="round"/>'
    )
    parts.append(
        f'<rect x="{CAP_X - 22:g}" y="{CAP_Y0 - 22:g}" width="44" height="{CAP_Y1 - CAP_Y0 + 44:g}" rx="22" fill="url(#cap)"/>'
    )
    parts.append("</svg>\n")
    return "\n".join(parts)


def bmp_frame(im: Image.Image) -> bytes:
    """32-bit BGRA DIB with an AND mask, as an ICO frame expects."""
    w, h = im.size
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    bgra = np.asarray(im)[::-1][..., [2, 1, 0, 3]].tobytes()
    mask_row = ((w + 31) // 32) * 4
    return header + bgra + bytes(mask_row * h)


def write_ico(path: Path, sizes: list[int]) -> None:
    frames = []
    for s in sizes:
        im = render(s)
        if s >= 256:
            buf = io.BytesIO()
            im.save(buf, "PNG", optimize=True)
            frames.append((s, buf.getvalue()))
        else:
            frames.append((s, bmp_frame(im)))
    out = bytearray(struct.pack("<HHH", 0, 1, len(frames)))
    offset = 6 + 16 * len(frames)
    for s, data in frames:
        dim = 0 if s >= 256 else s
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in frames:
        out += data
    path.write_bytes(bytes(out))


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "tine-icon.svg").write_text(svg(plate=True), encoding="utf-8", newline="\n")
    (OUT / "tine-mark.svg").write_text(svg(plate=False), encoding="utf-8", newline="\n")
    for s in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
        render(s).save(OUT / f"tine-{s}.png", optimize=True)
    write_ico(OUT / "tine.ico", [16, 20, 24, 32, 40, 48, 64, 128, 256])
    for state, colors in TRAY_STATES.items():
        (OUT / f"tray-{state}-32.rgba").write_bytes(render(32, colors=colors).tobytes())
    print(f"wrote brand assets to {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
