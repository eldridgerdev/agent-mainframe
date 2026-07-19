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
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

from PIL import Image, ImageDraw, ImageFont

# v1 limitation: double-width CJK/emoji cells are drawn single-width (each
# codepoint gets one cell), so wide glyphs will visually overlap/clip.

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


def render(grid, rows, cols, font_size, theme_name, out_path):
    theme = THEMES[theme_name]
    default_bg, default_fg = theme["bg"], theme["fg"]

    font = load_font(FONT_CANDIDATES, font_size)
    bold_font = load_font(BOLD_FONT_CANDIDATES, font_size)
    if font is None:
        print(
            "warning: DejaVuSansMono.ttf not found, falling back to Pillow default "
            "bitmap font (size/legibility will suffer)",
            file=sys.stderr,
        )
        font = ImageFont.load_default()
        bold_font = None

    probe = Image.new("RGB", (1, 1))
    probe_draw = ImageDraw.Draw(probe)
    bbox = probe_draw.textbbox((0, 0), "M", font=font)
    cell_w = bbox[2] - bbox[0] + 2
    ascent, descent = font.getmetrics()
    cell_h = ascent + descent + 2

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
            if char != " ":
                use_font = bold_font if (state.bold and bold_font) else font
                draw.text((x0 + 1, y0 + 1), char, font=use_font, fill=fg)
                if state.bold and not bold_font:
                    draw.text((x0 + 2, y0 + 1), char, font=use_font, fill=fg)
            if state.underline:
                uy = y0 + cell_h - 2
                draw.line([(x0, uy), (x0 + cell_w - 1, uy)], fill=fg)

    img.save(out_path)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("input", nargs="?", help="ANSI dump file (default: stdin)")
    ap.add_argument("--out", default="screenshot.png", help="output PNG path")
    ap.add_argument("--cols", type=int, help="override grid column count")
    ap.add_argument("--rows", type=int, help="override grid row count")
    ap.add_argument("--font-size", type=int, default=14)
    ap.add_argument("--theme", choices=["dark", "light"], default="dark")
    args = ap.parse_args()

    if args.input:
        text = Path(args.input).read_text(errors="replace")
    else:
        text = sys.stdin.read()

    grid, max_row, max_col = parse_grid(text)
    rows = args.rows or (max_row + 1)
    cols = args.cols or (max_col + 1)

    render(grid, rows, cols, args.font_size, args.theme, args.out)
    print(f"wrote {args.out} ({cols}x{rows} cells)")


if __name__ == "__main__":
    main()
