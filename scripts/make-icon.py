#!/usr/bin/env python3
"""Generate assets/AppIcon.icns for Relay using only the Python standard library.

Renders anti-aliased RGBA PNGs at 128, 256, 512, and 1024 px and packs them into
an Apple ICNS container (ic07, ic08, ic09, ic10) using Relay's design tokens
from docs/design.md.
"""

from __future__ import annotations

import math
import pathlib
import struct
import zlib

# Relay dark palette tokens (docs/design.md)
BG_TOP = (0x1B, 0x1C, 0x1F)      # #1b1c1f background
BG_BOT = (0x23, 0x24, 0x28)      # #232428 surface
RIM_COL = (0x34, 0x36, 0x3B)     # #34363b border
ACCENT = (0x5B, 0x8C, 0xFF)      # #5b8cff accent
AGENT_DOT = (0xB0, 0x7C, 0xFF)   # #b07cff agent purple

# Secondary (trailing) chevron is slightly softened accent for directional motion
CHEVRON_BACK = (
    int(BG_BOT[0] * 0.28 + ACCENT[0] * 0.72),
    int(BG_BOT[1] * 0.28 + ACCENT[1] * 0.72),
    int(BG_BOT[2] * 0.28 + ACCENT[2] * 0.72),
)

# Geometry in normalized canvas coordinates [-0.5, 0.5]
SQUIRCLE_HALF = 0.402   # ~80.4% of canvas (standard macOS icon margin)
SQUIRCLE_EXP = 5.5      # macOS continuous-corner superellipse exponent
RIM_WIDTH = 0.012       # subtle inner border

# Geometric Relay double-chevron + signal dot
CHEVRON_STROKE_R = 0.036
CHEVRON_HALF_H = 0.155
CHEVRON1_X0, CHEVRON1_X1 = -0.215, -0.070
CHEVRON2_X0, CHEVRON2_X1 = -0.055, 0.090
DOT_CX, DOT_CY, DOT_R = 0.212, 0.0, 0.052

ICNS_CHUNKS = (
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
)


def _clamp01(v: float) -> float:
    if v <= 0.0:
        return 0.0
    if v >= 1.0:
        return 1.0
    return v


def _sd_squircle(px: float, py: float, half: float, exp: float) -> float:
    """First-order Euclidean signed distance to a centered superellipse."""
    ax = abs(px) / half
    ay = abs(py) / half
    if ax == 0.0 and ay == 0.0:
        return -half
    u = ax**exp + ay**exp
    p = u ** (1.0 / exp)
    gx = (ax / p) ** (exp - 1.0) if ax > 0.0 else 0.0
    gy = (ay / p) ** (exp - 1.0) if ay > 0.0 else 0.0
    grad = math.hypot(gx, gy)
    return (p - 1.0) * half / grad


def _sd_segment(
    px: float, py: float, ax: float, ay: float, bx: float, by: float
) -> float:
    pax = px - ax
    pay = py - ay
    bax = bx - ax
    bay = by - ay
    t = (pax * bax + pay * bay) / (bax * bax + bay * bay)
    if t < 0.0:
        t = 0.0
    elif t > 1.0:
        t = 1.0
    return math.hypot(pax - bax * t, pay - bay * t)


def _sd_chevron(px: float, py: float, x0: float, x1: float) -> float:
    # Vertical symmetry across y = 0 means testing only the upper arm with |py|
    # gives the exact distance to both arms of the symmetric chevron.
    return (
        _sd_segment(px, abs(py), x0, CHEVRON_HALF_H, x1, 0.0)
        - CHEVRON_STROKE_R
    )


def _blend_over(
    dst_r: float, dst_g: float, dst_b: float, col: tuple[int, int, int], alpha: float
) -> tuple[float, float, float]:
    inv = 1.0 - alpha
    return (
        dst_r * inv + col[0] * alpha,
        dst_g * inv + col[1] * alpha,
        dst_b * inv + col[2] * alpha,
    )


def render_rgba(size: int) -> bytearray:
    """Render a crisp anti-aliased RGBA image of size x size."""
    buf = bytearray(size * size * 4)
    inv_size = 1.0 / size
    aa = 1.15 * inv_size
    inv_aa = 1.0 / aa
    half = SQUIRCLE_HALF
    exp = SQUIRCLE_EXP
    rim_w = RIM_WIDTH
    shadow_dy = 0.014
    shadow_blur = 0.026
    inv_shadow_blur = 1.0 / shadow_blur

    # Precompute vertical gradient colors per row
    bg_top_r, bg_top_g, bg_top_b = BG_TOP
    bg_dr = BG_BOT[0] - bg_top_r
    bg_dg = BG_BOT[1] - bg_top_g
    bg_db = BG_BOT[2] - bg_top_b
    grad_span = 1.0 / (2.0 * half)

    # Bounding boxes for fast skipping of glyph SDFs
    c1_min_x = CHEVRON1_X0 - CHEVRON_STROKE_R - aa
    c1_max_x = CHEVRON1_X1 + CHEVRON_STROKE_R + aa
    c2_min_x = CHEVRON2_X0 - CHEVRON_STROKE_R - aa
    c2_max_x = CHEVRON2_X1 + CHEVRON_STROKE_R + aa
    c_max_y = CHEVRON_HALF_H + CHEVRON_STROKE_R + aa

    dot_min_x = DOT_CX - DOT_R - aa
    dot_max_x = DOT_CX + DOT_R + aa
    dot_max_y = DOT_R + aa

    outer_bound = half + shadow_blur + shadow_dy + aa
    inner_safe = half - rim_w - aa

    idx = 0
    for y in range(size):
        py = (y + 0.5) * inv_size - 0.5
        ay = abs(py)

        if ay > outer_bound:
            idx += size * 4
            continue

        # Subtle vertical gradient inside squircle: #1b1c1f (top) -> #232428 (bottom)
        t = _clamp01((py + half) * grad_span)
        row_r = bg_top_r + bg_dr * t
        row_g = bg_top_g + bg_dg * t
        row_b = bg_top_b + bg_db * t

        # Subtle top edge sheen inside the squircle for crisp native depth
        in_chev_y = ay <= c_max_y
        in_dot_y = ay <= dot_max_y
        ay_safe = ay < inner_safe * 0.72

        for x in range(size):
            px = (x + 0.5) * inv_size - 0.5
            ax = abs(px)

            if ax > outer_bound:
                idx += 4
                continue

            # Fast path: well inside the squircle interior (not near rim)
            if ay_safe and ax < inner_safe * 0.72:
                r, g, b = row_r, row_g, row_b
                if in_chev_y:
                    if c1_min_x <= px <= c1_max_x:
                        d1 = _sd_chevron(px, py, CHEVRON1_X0, CHEVRON1_X1)
                        if d1 < aa:
                            a1 = _clamp01(0.5 - d1 * inv_aa)
                            r, g, b = _blend_over(r, g, b, CHEVRON_BACK, a1)
                    if c2_min_x <= px <= c2_max_x:
                        d2 = _sd_chevron(px, py, CHEVRON2_X0, CHEVRON2_X1)
                        if d2 < aa:
                            a2 = _clamp01(0.5 - d2 * inv_aa)
                            r, g, b = _blend_over(r, g, b, ACCENT, a2)
                if in_dot_y and dot_min_x <= px <= dot_max_x:
                    dd = math.hypot(px - DOT_CX, py - DOT_CY) - DOT_R
                    if dd < aa:
                        ad = _clamp01(0.5 - dd * inv_aa)
                        r, g, b = _blend_over(r, g, b, AGENT_DOT, ad)

                buf[idx] = int(r + 0.5)
                buf[idx + 1] = int(g + 0.5)
                buf[idx + 2] = int(b + 0.5)
                buf[idx + 3] = 255
                idx += 4
                continue

            d_sq = _sd_squircle(px, py, half, exp)

            if d_sq >= aa:
                # Outside squircle: render subtle macOS ambient drop shadow
                d_sh = _sd_squircle(px, py - shadow_dy, half, exp)
                if d_sh < shadow_blur:
                    sh_t = _clamp01(1.0 - max(0.0, d_sh) * inv_shadow_blur)
                    sh_a = int(sh_t * sh_t * 64.0 + 0.5)
                    if sh_a > 0:
                        buf[idx + 3] = sh_a
                idx += 4
                continue

            # Inside or on squircle boundary
            cov = _clamp01(0.5 - d_sq * inv_aa)
            r, g, b = row_r, row_g, row_b

            # Subtle inner rim (#34363b)
            if d_sq > -rim_w - aa:
                rim_cov = _clamp01((d_sq + rim_w) * inv_aa + 0.5)
                r, g, b = _blend_over(r, g, b, RIM_COL, rim_cov * 0.92)

            # Foreground marks (clipped naturally by squircle cov)
            if in_chev_y:
                if c1_min_x <= px <= c1_max_x:
                    d1 = _sd_chevron(px, py, CHEVRON1_X0, CHEVRON1_X1)
                    if d1 < aa:
                        a1 = _clamp01(0.5 - d1 * inv_aa)
                        r, g, b = _blend_over(r, g, b, CHEVRON_BACK, a1)
                if c2_min_x <= px <= c2_max_x:
                    d2 = _sd_chevron(px, py, CHEVRON2_X0, CHEVRON2_X1)
                    if d2 < aa:
                        a2 = _clamp01(0.5 - d2 * inv_aa)
                        r, g, b = _blend_over(r, g, b, ACCENT, a2)
            if in_dot_y and dot_min_x <= px <= dot_max_x:
                dd = math.hypot(px - DOT_CX, py - DOT_CY) - DOT_R
                if dd < aa:
                    ad = _clamp01(0.5 - dd * inv_aa)
                    r, g, b = _blend_over(r, g, b, AGENT_DOT, ad)

            if cov < 1.0:
                # Composite squircle edge over drop shadow for clean anti-aliased edge
                d_sh = _sd_squircle(px, py - shadow_dy, half, exp)
                sh_a = 0.0
                if d_sh < shadow_blur:
                    sh_t = _clamp01(1.0 - max(0.0, d_sh) * inv_shadow_blur)
                    sh_a = sh_t * sh_t * (64.0 / 255.0)
                out_a = cov + sh_a * (1.0 - cov)
                if out_a > 0.0:
                    scale = cov / out_a
                    r *= scale
                    g *= scale
                    b *= scale
                alpha_byte = int(out_a * 255.0 + 0.5)
            else:
                alpha_byte = 255

            buf[idx] = int(r + 0.5)
            buf[idx + 1] = int(g + 0.5)
            buf[idx + 2] = int(b + 0.5)
            buf[idx + 3] = alpha_byte
            idx += 4

    return buf


def downsample_2x(src: bytearray, src_size: int) -> bytearray:
    """Box-downsample an RGBA buffer by 2x using premultiplied alpha."""
    dst_size = src_size // 2
    dst = bytearray(dst_size * dst_size * 4)
    row_stride = src_size * 4
    d_idx = 0
    for y in range(dst_size):
        row0 = (y * 2) * row_stride
        row1 = row0 + row_stride
        for x in range(dst_size):
            x4 = x * 8
            i00 = row0 + x4
            i01 = i00 + 4
            i10 = row1 + x4
            i11 = i10 + 4

            a00 = src[i00 + 3]
            a01 = src[i01 + 3]
            a10 = src[i10 + 3]
            a11 = src[i11 + 3]
            a_sum = a00 + a01 + a10 + a11
            if a_sum == 0:
                d_idx += 4
                continue

            if a_sum == 1020:
                dst[d_idx] = (src[i00] + src[i01] + src[i10] + src[i11] + 2) >> 2
                dst[d_idx + 1] = (src[i00 + 1] + src[i01 + 1] + src[i10 + 1] + src[i11 + 1] + 2) >> 2
                dst[d_idx + 2] = (src[i00 + 2] + src[i01 + 2] + src[i10 + 2] + src[i11 + 2] + 2) >> 2
                dst[d_idx + 3] = 255
            else:
                r = (
                    src[i00] * a00
                    + src[i01] * a01
                    + src[i10] * a10
                    + src[i11] * a11
                    + (a_sum >> 1)
                ) // a_sum
                g = (
                    src[i00 + 1] * a00
                    + src[i01 + 1] * a01
                    + src[i10 + 1] * a10
                    + src[i11 + 1] * a11
                    + (a_sum >> 1)
                ) // a_sum
                b = (
                    src[i00 + 2] * a00
                    + src[i01 + 2] * a01
                    + src[i10 + 2] * a10
                    + src[i11 + 2] * a11
                    + (a_sum >> 1)
                ) // a_sum
                dst[d_idx] = r
                dst[d_idx + 1] = g
                dst[d_idx + 2] = b
                dst[d_idx + 3] = (a_sum + 2) >> 2
            d_idx += 4
    return dst


def _png_chunk(chunk_type: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(chunk_type)
    crc = zlib.crc32(data, crc) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + chunk_type + data + struct.pack(">I", crc)


def encode_png(rgba: bytearray, width: int, height: int) -> bytes:
    """Encode an 8-bit RGBA buffer to a deterministic PNG using Sub filtering."""
    stride = width * 4
    filtered = bytearray((stride + 1) * height)
    out = 0
    for y in range(height):
        filtered[out] = 1  # Filter type 1: Sub
        out += 1
        row_start = y * stride
        # First pixel (4 bytes) has no left neighbor
        filtered[out : out + 4] = rgba[row_start : row_start + 4]
        out += 4
        # Remaining bytes subtract byte 4 positions to the left
        for i in range(4, stride):
            filtered[out] = (rgba[row_start + i] - rgba[row_start + i - 4]) & 0xFF
            out += 1

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(filtered), level=9)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", idat)
        + _png_chunk(b"IEND", b"")
    )


def build_icns(out_path: pathlib.Path) -> int:
    """Render all sizes and write an Apple ICNS file to out_path."""
    rgba_1024 = render_rgba(1024)
    rgba_512 = downsample_2x(rgba_1024, 1024)
    rgba_256 = downsample_2x(rgba_512, 512)
    rgba_128 = downsample_2x(rgba_256, 256)

    buffers = {
        128: rgba_128,
        256: rgba_256,
        512: rgba_512,
        1024: rgba_1024,
    }

    chunks = bytearray()
    for ostype, size in ICNS_CHUNKS:
        png = encode_png(buffers[size], size, size)
        chunks += ostype + struct.pack(">I", 8 + len(png)) + png

    total_len = 8 + len(chunks)
    icns = b"icns" + struct.pack(">I", total_len) + bytes(chunks)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(icns)
    return total_len


def main() -> None:
    root = pathlib.Path(__file__).resolve().parent.parent
    out_path = root / "assets" / "AppIcon.icns"
    total = build_icns(out_path)
    print(f"wrote {out_path.relative_to(root)} ({total} bytes)")


if __name__ == "__main__":
    main()
