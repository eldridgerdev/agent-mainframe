"""Box-drawing regressions for the Pillow renderer; no AMF instance or browser is started."""

import pathlib
import tempfile
import unittest

from PIL import Image

import render_ansi

BACKGROUND = render_ansi.THEMES["dark"]["bg"]
FOREGROUND = render_ansi.THEMES["dark"]["fg"]


def render(text, scale):
    grid, max_row, max_col = render_ansi.parse_grid(text)
    with tempfile.TemporaryDirectory() as tmp:
        out = pathlib.Path(tmp) / "frame.png"
        render_ansi.render(grid, max_row + 1, max_col + 1, 14, "dark", out, scale)
        with Image.open(out) as image:
            return image.convert("RGB")


class BoxDrawingTests(unittest.TestCase):
    def test_horizontal_border_is_one_unbroken_line(self):
        for scale in (1, 2):
            image = render("─" * 6, scale)
            inked = [y for y in range(image.height)
                     if any(image.getpixel((x, y)) != BACKGROUND for x in range(image.width))]
            self.assertTrue(inked, "the border drew nothing")
            # Full strength the whole way across: a seam between two cells is
            # a faded pixel, not necessarily a blank one.
            for y in inked:
                seams = [x for x in range(image.width) if image.getpixel((x, y)) != FOREGROUND]
                self.assertEqual(seams, [], f"scale {scale}: row {y} fades between cells")

    def test_vertical_border_is_one_unbroken_line(self):
        for scale in (1, 2):
            image = render("\n".join("│" * 6), scale)
            inked = [x for x in range(image.width)
                     if any(image.getpixel((x, y)) != BACKGROUND for y in range(image.height))]
            self.assertTrue(inked, "the border drew nothing")
            for x in inked:
                seams = [y for y in range(image.height) if image.getpixel((x, y)) != FOREGROUND]
                self.assertEqual(seams, [], f"scale {scale}: column {x} fades between cells")

    def test_every_arm_reaches_its_cell_edge_and_no_other(self):
        # A line, corner, tee or arc joins its neighbour only if each arm it
        # has touches that side of the cell, and looks wrong if any other does.
        width, height, light = 17, 33, 2
        edges = {
            "u": [(x, 0) for x in range(width)],
            "d": [(x, height - 1) for x in range(width)],
            "l": [(0, y) for y in range(height)],
            "r": [(width - 1, y) for y in range(height)],
        }
        checked = 0
        for code in range(0x2500, 0x2580):
            char = chr(code)
            spec = render_ansi.box_spec(char)
            if spec is None or spec[0] == "dash":
                continue
            mask = render_ansi.cell_mask(char, width, height, light)
            for arm, weight in spec[1].items():
                touches = any(mask.getpixel(point) for point in edges[arm])
                self.assertEqual(touches, bool(weight), f"U+{code:04X} {char} arm {arm!r}")
            checked += 1
        self.assertGreater(checked, 100)


if __name__ == "__main__":
    unittest.main()
