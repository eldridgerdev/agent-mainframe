#!/usr/bin/env python3
"""Assemble a sequence of same-size PNG frames into an animated GIF.

No ffmpeg dependency: Pillow can write animated GIFs natively via
Image.save(save_all=True, append_images=...).
"""

import argparse

from PIL import Image


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("frames", nargs="+", help="PNG frame paths, in display order")
    ap.add_argument("--out", required=True, help="output GIF path")
    ap.add_argument("--duration-ms", type=int, default=800, help="per-frame duration in ms")
    args = ap.parse_args()

    images = [Image.open(p).convert("RGB") for p in args.frames]
    images[0].save(
        args.out,
        save_all=True,
        append_images=images[1:],
        loop=0,
        duration=args.duration_ms,
    )
    print(f"wrote {args.out} ({len(images)} frames)")


if __name__ == "__main__":
    main()
