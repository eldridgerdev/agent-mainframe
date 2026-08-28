#!/usr/bin/env python3
"""Replace the marked AMF screenshot section while preserving the PR body."""

import argparse
import re
from pathlib import Path


START = "<!-- amf:screenshots:start -->"
END = "<!-- amf:screenshots:end -->"


def update_body(body: str, fragment: str) -> str:
    pattern = re.compile(re.escape(START) + r".*?" + re.escape(END), re.DOTALL)
    matches = list(pattern.finditer(body))
    if len(matches) > 1:
        raise ValueError("PR body contains multiple AMF screenshot sections")
    if matches:
        return body[: matches[0].start()] + fragment.strip() + body[matches[0].end() :]

    if body and not body.endswith("\n"):
        body += "\n"
    return body + "\n" + fragment.strip() + "\n"


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--body-file", required=True, type=Path)
    ap.add_argument("--fragment-file", required=True, type=Path)
    ap.add_argument("--output-file", required=True, type=Path)
    args = ap.parse_args()

    body = args.body_file.read_text(encoding="utf-8")
    fragment = args.fragment_file.read_text(encoding="utf-8")
    args.output_file.write_text(update_body(body, fragment) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
