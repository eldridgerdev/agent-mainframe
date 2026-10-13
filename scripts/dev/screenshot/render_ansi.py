#!/usr/bin/env python3
"""Render a `tmux capture-pane -e -p` ANSI dump to a PNG.

capture-pane emits an already laid-out grid (fixed rows/cols, no cursor
movement escapes) so this only needs an SGR state machine, not a full
terminal emulator: walk the text left-to-right/top-to-bottom, tracking
current SGR attributes per cell, then rasterize the grid.
"""

import argparse
import subprocess
import sys
import unicodedata
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Optional

from PIL import Image, ImageDraw, ImageFont

# v1 limitation: double-width CJK/emoji cells are drawn single-width (each
# codepoint gets one cell), so wide glyphs will visually overlap/clip.

# Box-drawing and block-element characters are drawn as geometry instead of
# taken from the font, the way terminal emulators do it. A font's glyph is
# sized to the font's own fractional advance and line height, so once the
# cell is rounded to whole pixels neighbouring glyphs stop short of each
# other and a border reads as a row of dashes.
LIGHT, HEAVY, DOUBLE = 1, 2, 3
BOX_WEIGHTS = {"LIGHT": LIGHT, "SINGLE": LIGHT, "HEAVY": HEAVY, "DOUBLE": DOUBLE}
BOX_DIRECTIONS = {
    "UP": "u", "DOWN": "d", "LEFT": "l", "RIGHT": "r",
    "VERTICAL": "ud", "HORIZONTAL": "lr",
}
DASH_COUNTS = {"DOUBLE": 2, "TRIPLE": 3, "QUADRUPLE": 4}
# Arms on either side of an arm, keyed by the arm: (negative-offset side,
# positive-offset side), then the arm straight across the cell.
BOX_SIDES = {"u": "lr", "d": "lr", "l": "ud", "r": "ud"}
BOX_OPPOSITE = {"u": "d", "d": "u", "l": "r", "r": "l"}
ARC_SUPERSAMPLE = 4
# PIL arc start angle (clockwise from 3 o'clock) for the quarter turn joining
# a horizontal arm on side sx to a vertical arm on side sy.
ARC_START = {(1, 1): 180, (-1, 1): 270, (-1, -1): 0, (1, -1): 90}
SHADE_ALPHA = {"░": 64, "▒": 128, "▓": 191}
# Quadrant characters U+2596..U+259F as (upper-left, upper-right, lower-left,
# lower-right) fill flags.
QUADRANTS = [
    (0, 0, 1, 0), (0, 0, 0, 1), (1, 0, 0, 0), (1, 0, 1, 1), (1, 0, 0, 1),
    (1, 1, 1, 0), (1, 1, 0, 1), (0, 1, 0, 0), (0, 1, 1, 0), (0, 1, 1, 1),
]

ANSI_16 = [
    (0x00, 0x00, 0x00), (0xaa, 0x00, 0x00), (0x00, 0xaa, 0x00), (0xaa, 0x55, 0x00),
    (0x00, 0x00, 0xaa), (0xaa, 0x00, 0xaa), (0x00, 0xaa, 0xaa), (0xaa, 0xaa, 0xaa),
    (0x55, 0x55, 0x55), (0xff, 0x55, 0x55), (0x55, 0xff, 0x55), (0xff, 0xff, 0x55),
    (0x55, 0x55, 0xff), (0xff, 0x55, 0xff), (0x55, 0xff, 0xff), (0xff, 0xff, 0xff),
]

CUBE_LEVELS = [0, 95, 135, 175, 215, 255]

THEMES = {
    "dark": {"bg": (0x1e, 0x1e, 0x1e), "fg": (0xd4, 0xd4, 0xd4)},
    "light": {"bg": (0xff, 0xff, 0xff), "fg": (0x1e, 0x1e, 0x1e)},
}

FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
]
BOLD_FONT_CANDIDATES = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
]


def color_256(n: int) -> tuple:
    if n < 16:
        return ANSI_16[n]
    if n < 232:
        n -= 16
        r, g, b = n // 36, (n // 6) % 6, n % 6
        return (CUBE_LEVELS[r], CUBE_LEVELS[g], CUBE_LEVELS[b])
    level = 8 + 10 * (n - 232)
    return (level, level, level)


@dataclass
class SgrState:
    fg: Optional[tuple] = None
    bg: Optional[tuple] = None
    bold: bool = False
    reverse: bool = False
    underline: bool = False

    def clone(self):
        return SgrState(self.fg, self.bg, self.bold, self.reverse, self.underline)


def apply_sgr(state: SgrState, params: str) -> None:
    parts = [p for p in params.split(";")]
    codes = [int(p) if p else 0 for p in parts] if parts != [""] else [0]
    i = 0
    while i < len(codes):
        code = codes[i]
        if code == 0:
            state.fg = None
            state.bg = None
            state.bold = False
            state.reverse = False
            state.underline = False
        elif code == 1:
            state.bold = True
        elif code == 22:
            state.bold = False
        elif code == 4:
            state.underline = True
        elif code == 24:
            state.underline = False
        elif code == 7:
            state.reverse = True
        elif code == 27:
            state.reverse = False
        elif 30 <= code <= 37:
            state.fg = ANSI_16[code - 30]
        elif code == 38:
            if i + 1 < len(codes) and codes[i + 1] == 5 and i + 2 < len(codes):
                state.fg = color_256(codes[i + 2])
                i += 2
            elif i + 1 < len(codes) and codes[i + 1] == 2 and i + 4 < len(codes):
                state.fg = (codes[i + 2], codes[i + 3], codes[i + 4])
                i += 4
        elif code == 39:
            state.fg = None
        elif 40 <= code <= 47:
            state.bg = ANSI_16[code - 40]
        elif code == 48:
            if i + 1 < len(codes) and codes[i + 1] == 5 and i + 2 < len(codes):
                state.bg = color_256(codes[i + 2])
                i += 2
            elif i + 1 < len(codes) and codes[i + 1] == 2 and i + 4 < len(codes):
                state.bg = (codes[i + 2], codes[i + 3], codes[i + 4])
                i += 4
        elif code == 49:
            state.bg = None
        elif 90 <= code <= 97:
            state.fg = ANSI_16[8 + (code - 90)]
        elif 100 <= code <= 107:
            state.bg = ANSI_16[8 + (code - 100)]
        i += 1


def parse_grid(text: str):
    """Returns (rows_dict, max_row, max_col). rows_dict[r][c] = (char, SgrState)."""
    grid = {}
    state = SgrState()
    row, col = 0, 0
    max_row, max_col = 0, 0
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch == "\x1b":
            nxt = text[i + 1] if i + 1 < n else ""
            if nxt == "[":
                j = i + 2
                while j < n and not (0x40 <= ord(text[j]) <= 0x7e):
                    j += 1
                final = text[j] if j < n else ""
                params = text[i + 2:j]
                if final == "m":
                    apply_sgr(state, params)
                i = j + 1
                continue
            if nxt == "]":
                j = i + 2
                while j < n and text[j] != "\x07":
                    if text[j] == "\x1b" and j + 1 < n and text[j + 1] == "\\":
                        j += 1
                        break
                    j += 1
                i = j + 1
                continue
            if nxt in "()":
                i += 3
                continue
            i += 2
            continue
        if ch == "\n":
            row += 1
            col = 0
            i += 1
            continue
        if ch == "\r":
            col = 0
            i += 1
            continue
        if ch == "\t":
            col = (col // 8 + 1) * 8
            i += 1
            continue
        grid.setdefault(row, {})[col] = (ch, state.clone())
        max_row = max(max_row, row)
        max_col = max(max_col, col)
        col += 1
        i += 1
    return grid, max_row, max_col


def load_font(path_candidates, size):
    for path in path_candidates:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    try:
        out = subprocess.run(
            ["fc-list", "DejaVu Sans Mono"], capture_output=True, text=True, timeout=5
        )
        for line in out.stdout.splitlines():
            fp = line.split(":")[0].strip()
            if fp and Path(fp).exists():
                return ImageFont.truetype(fp, size)
    except (OSError, subprocess.SubprocessError):
        pass
    return None


@lru_cache(maxsize=None)
def box_spec(char: str):
    """Decode a box-drawing character from its Unicode name.

    Returns ("lines" | "arc", {arm: weight}) or ("dash", weight, count,
    vertical), or None for anything left to the font (diagonals).
    """
    if not "─" <= char <= "╿":
        return None
    words = unicodedata.name(char, "").replace("BOX DRAWINGS ", "").split()
    if not words or "DIAGONAL" in words:
        return None
    if "DASH" in words:
        return ("dash", BOX_WEIGHTS[words[0]], DASH_COUNTS[words[1]], words[-1] == "VERTICAL")
    # "LIGHT DOWN AND RIGHT", "DOWN LIGHT AND RIGHT HEAVY", "UP HEAVY AND
    # RIGHT DOWN LIGHT": a weight covers the directions in its own group, and
    # a group that names none inherits the one before it.
    arms = dict.fromkeys("urdl", 0)
    weight = 0
    for group in " ".join(words).split(" AND "):
        tokens = group.split()
        weight = next((BOX_WEIGHTS[t] for t in tokens if t in BOX_WEIGHTS), weight)
        for token in tokens:
            for arm in BOX_DIRECTIONS.get(token, ""):
                arms[arm] = weight
    return ("arc" if "ARC" in words else "lines", arms)


def stroke_span(centre: int, thickness: int):
    lo = centre - thickness // 2
    return lo, lo + thickness - 1


def draw_box_lines(draw, arms, w, h, light):
    """Fill a cell-sized mask with straight box-drawing arms.

    Every arm runs from its cell edge to a stopping point near the centre.
    A double line is two light strokes one stroke-width either side of the
    centre line; where each stops is what makes corners and tees join.
    """
    cx, cy = w // 2, h // 2
    gap = light

    def thickness(arm):
        return light * 2 if arms[arm] == HEAVY else light if arms[arm] else 0

    def fill(arm, lateral, thick, forward, perp_thick):
        # `forward` is how far past the centre (toward the opposite edge) the
        # perpendicular stroke this one meets sits; `perp_thick` is that
        # stroke's thickness, or 0 to stop on the centre pixel itself.
        vertical = arm in "ud"
        sign = 1 if arm in "ul" else -1
        centre = (cy if vertical else cx) + sign * forward
        lo, hi = stroke_span(centre, perp_thick) if perp_thick else (centre, centre)
        length = h if vertical else w
        a0, a1 = (0, hi) if sign > 0 else (lo, length - 1)
        b0, b1 = stroke_span((cx if vertical else cy) + lateral, thick)
        draw.rectangle([b0, a0, b1, a1] if vertical else [a0, b0, a1, b1], fill=255)

    for arm, weight in arms.items():
        if not weight:
            continue
        near_side, far_side = BOX_SIDES[arm]
        opposite = arms[BOX_OPPOSITE[arm]]
        if weight == DOUBLE:
            for lateral, side, other in ((-gap, near_side, far_side), (gap, far_side, near_side)):
                if arms[side] == DOUBLE:
                    fill(arm, lateral, light, -gap, light)
                elif arms[side]:
                    fill(arm, lateral, light, 0, thickness(side))
                elif opposite:
                    fill(arm, lateral, light, 0, 0)
                elif arms[other] == DOUBLE:
                    fill(arm, lateral, light, gap, light)
                elif arms[other]:
                    fill(arm, lateral, light, 0, thickness(other))
                else:
                    fill(arm, lateral, light, 0, 0)
            continue
        sides = (arms[near_side], arms[far_side])
        if DOUBLE in sides:
            if opposite:
                fill(arm, 0, thickness(arm), 0, 0)
            else:
                fill(arm, 0, thickness(arm), -gap if all(sides) else gap, light)
        else:
            fill(arm, 0, thickness(arm), 0, max(thickness(near_side), thickness(far_side)))


def draw_box_arc(draw, arms, w, h, light):
    """Rounded corner, drawn on a canvas ARC_SUPERSAMPLE times the cell."""
    k = ARC_SUPERSAMPLE
    t = light * k
    xc = stroke_span(w // 2, light)[0] * k + t / 2
    yc = stroke_span(h // 2, light)[0] * k + t / 2
    sx = 1 if arms["r"] else -1
    sy = 1 if arms["d"] else -1
    radius = min(xc, w * k - xc, yc, h * k - yc)
    ax, ay = xc + sx * radius, yc + sy * radius
    outer = radius + t / 2
    start = ARC_START[(sx, sy)]
    draw.arc([ax - outer, ay - outer, ax + outer, ay + outer], start, start + 90, fill=255, width=t)
    draw.rectangle([min(ax, (w * k if sx > 0 else 0)), yc - t / 2,
                    max(ax, (w * k if sx > 0 else 0)), yc + t / 2 - 1], fill=255)
    draw.rectangle([xc - t / 2, min(ay, (h * k if sy > 0 else 0)),
                    xc + t / 2 - 1, max(ay, (h * k if sy > 0 else 0))], fill=255)


def draw_block_element(draw, char, w, h):
    code = ord(char)
    if char in SHADE_ALPHA:
        draw.rectangle([0, 0, w - 1, h - 1], fill=SHADE_ALPHA[char])
    elif code == 0x2580:  # upper half
        draw.rectangle([0, 0, w - 1, round(h / 2) - 1], fill=255)
    elif 0x2581 <= code <= 0x2588:  # lower 1/8 .. full
        draw.rectangle([0, h - round(h * (code - 0x2580) / 8), w - 1, h - 1], fill=255)
    elif 0x2589 <= code <= 0x258f:  # left 7/8 .. 1/8
        draw.rectangle([0, 0, max(1, round(w * (0x2590 - code) / 8)) - 1, h - 1], fill=255)
    elif code == 0x2590:  # right half
        draw.rectangle([w - round(w / 2), 0, w - 1, h - 1], fill=255)
    elif code == 0x2594:  # upper 1/8
        draw.rectangle([0, 0, w - 1, max(1, round(h / 8)) - 1], fill=255)
    elif code == 0x2595:  # right 1/8
        draw.rectangle([w - max(1, round(w / 8)), 0, w - 1, h - 1], fill=255)
    else:
        mx, my = round(w / 2), round(h / 2)
        quads = [(0, 0, mx - 1, my - 1), (mx, 0, w - 1, my - 1),
                 (0, my, mx - 1, h - 1), (mx, my, w - 1, h - 1)]
        for filled, rect in zip(QUADRANTS[code - 0x2596], quads):
            if filled:
                draw.rectangle(rect, fill=255)


@lru_cache(maxsize=None)
def cell_mask(char: str, w: int, h: int, light: int):
    """Coverage mask for a character drawn as geometry, or None to use the font."""
    spec = box_spec(char)
    if spec is None and not "▀" <= char <= "▟":
        return None
    scale = ARC_SUPERSAMPLE if spec and spec[0] == "arc" else 1
    mask = Image.new("L", (w * scale, h * scale), 0)
    draw = ImageDraw.Draw(mask)
    if spec is None:
        draw_block_element(draw, char, w, h)
    elif spec[0] == "arc":
        draw_box_arc(draw, spec[1], w, h, light)
        mask = mask.resize((w, h), Image.BOX)
    elif spec[0] == "dash":
        _, weight, count, vertical = spec
        thick = light * 2 if weight == HEAVY else light
        length = h if vertical else w
        b0, b1 = stroke_span((w if vertical else h) // 2, thick)
        gap = max(1, round(length / count / 4))
        for i in range(count):
            a0, a1 = round(i * length / count), round((i + 1) * length / count) - gap - 1
            draw.rectangle([b0, a0, b1, a1] if vertical else [a0, b0, a1, b1], fill=255)
    else:
        draw_box_lines(draw, spec[1], w, h, light)
    return mask


def render(grid, rows, cols, font_size, theme_name, out_path, scale=2):
    theme = THEMES[theme_name]
    default_bg, default_fg = theme["bg"], theme["fg"]

    font_size *= scale
    font = load_font(FONT_CANDIDATES, font_size)
    bold_font = load_font(BOLD_FONT_CANDIDATES, font_size)
    # DejaVu Sans Mono has no Braille glyphs. Terminal emulators fall back
    # to another font for these Mermaid connectors; do the same per cell.
    braille_font = next(
        (
            ImageFont.truetype(path, font_size)
            for path in (
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
            )
            if Path(path).exists()
        ),
        None,
    )
    if font is None:
        print(
            "warning: DejaVuSansMono.ttf not found, falling back to Pillow default "
            "bitmap font (size/legibility will suffer)",
            file=sys.stderr,
        )
        font = ImageFont.load_default()
        bold_font = None

    # The font's own advance and line height, with no padding: any extra
    # spreads the text out and opens a seam between adjacent cells.
    cell_w = max(1, round(font.getlength("M")))
    ascent, descent = font.getmetrics()
    cell_h = ascent + descent
    light = max(1, round(cell_h / 16))

    img = Image.new("RGB", (cols * cell_w, rows * cell_h), default_bg)
    draw = ImageDraw.Draw(img)

    for r in range(rows):
        row_cells = grid.get(r, {})
        for c in range(cols):
            cell = row_cells.get(c)
            char, state = cell if cell else (" ", SgrState())
            fg = state.fg or default_fg
            bg = state.bg or default_bg
            if state.reverse:
                fg, bg = bg, fg
            x0, y0 = c * cell_w, r * cell_h
            if bg != default_bg:
                draw.rectangle([x0, y0, x0 + cell_w - 1, y0 + cell_h - 1], fill=bg)
            mask = cell_mask(char, cell_w, cell_h, light) if char != " " else None
            if mask is not None:
                img.paste(fg, (x0, y0), mask)
            elif char != " ":
                use_font = bold_font if (state.bold and bold_font) else font
                if "\u2800" <= char <= "\u28ff" and braille_font:
                    use_font = braille_font
                draw.text((x0, y0), char, font=use_font, fill=fg)
                if state.bold and not bold_font:
                    draw.text((x0 + 1, y0), char, font=use_font, fill=fg)
            if state.underline:
                uy = y0 + cell_h - 1 - light
                draw.rectangle([x0, uy, x0 + cell_w - 1, uy + light - 1], fill=fg)

    img.save(out_path)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("input", nargs="?", help="ANSI dump file (default: stdin)")
    ap.add_argument("--out", default="screenshot.png", help="output PNG path")
    ap.add_argument("--cols", type=int, help="override grid column count")
    ap.add_argument("--rows", type=int, help="override grid row count")
    ap.add_argument("--font-size", type=int, default=14)
    ap.add_argument(
        "--scale", type=int, default=2,
        help="pixel density multiplier; 2 stays sharp on a HiDPI display (default 2)",
    )
    ap.add_argument("--theme", choices=["dark", "light"], default="dark")
    args = ap.parse_args()
    if args.scale < 1:
        ap.error("--scale must be at least 1")

    if args.input:
        text = Path(args.input).read_text(errors="replace")
    else:
        text = sys.stdin.read()

    grid, max_row, max_col = parse_grid(text)
    rows = args.rows or (max_row + 1)
    cols = args.cols or (max_col + 1)

    render(grid, rows, cols, args.font_size, args.theme, args.out, args.scale)
    print(f"wrote {args.out} ({cols}x{rows} cells)")


if __name__ == "__main__":
    main()
