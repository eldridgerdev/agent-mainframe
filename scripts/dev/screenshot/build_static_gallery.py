#!/usr/bin/env python3
"""Build the restricted, script-free Pages gallery from a downloaded capture.

Input is the directory `gh run download` writes for an
`amf-screenshot-artifact.yml` run: rendered `*.png` / `*.gif` frames plus an
optional `capture-notes.jsonl` (one `{"file": ..., "note": ...}` object per
line). Output is a self-contained directory ready for `wrangler pages deploy` —
`index.html` with every image inlined as a `data:` URI and a `_headers` file
pinning a `default-src 'none'` Content-Security-Policy.

This runs on the operator's machine, not in CI: the Cloudflare credentials the
deploy needs never enter a GitHub runner. It deliberately never reads the raw
`.ansi` / `.txt` captures — only rendered images reach the public gallery.
"""

import argparse
import base64
import html
import json
from pathlib import Path

MAX_IMAGE_BYTES = 10 * 1024 * 1024

HEADERS = """/*
  Content-Security-Policy: default-src 'none'; img-src data:; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'
  Referrer-Policy: no-referrer
  X-Content-Type-Options: nosniff
  X-Frame-Options: DENY
"""


def load_notes(source: Path) -> dict[str, str]:
    notes: dict[str, str] = {}
    notes_path = source / "capture-notes.jsonl"
    if not notes_path.exists():
        return notes
    for line in notes_path.read_text(encoding="utf-8").splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        notes[str(item.get("file", ""))] = str(item.get("note", ""))[:600]
    return notes


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input-dir", required=True, type=Path,
                    help="directory gh run download wrote the capture artifact to")
    ap.add_argument("--output-dir", required=True, type=Path,
                    help="directory to write index.html and _headers into")
    ap.add_argument("--pr-number", required=True)
    ap.add_argument("--summary", required=True, help="one sentence describing the flow")
    args = ap.parse_args()

    source = args.input_dir
    destination = args.output_dir
    destination.mkdir(parents=True, exist_ok=True)

    images = sorted(source.glob("*.png"))
    gifs = sorted(source.glob("*.gif"))
    allowed = images + gifs
    if not allowed:
        raise SystemExit(f"no rendered screenshots in {source}")
    oversized = [p for p in allowed if p.stat().st_size > MAX_IMAGE_BYTES]
    if oversized:
        raise SystemExit(
            "refusing to publish an image larger than 10 MiB: "
            + ", ".join(p.name for p in oversized)
        )

    notes = load_notes(source)
    figures = []
    for path in allowed:
        encoded = base64.b64encode(path.read_bytes()).decode("ascii")
        mime = "image/png" if path.suffix == ".png" else "image/gif"
        label = html.escape(path.stem.replace("-", " "))
        note = html.escape(notes.get(path.with_suffix(".ansi").name, ""))
        proof = f"<p><strong>What this proves:</strong> {note}</p>" if note else ""
        figures.append(
            f'<figure><img src="data:{mime};base64,{encoded}" alt="{label}">'
            f"<figcaption><strong>Step {len(figures) + 1}:</strong> "
            f"{label}{proof}</figcaption></figure>"
        )

    pr = html.escape(str(args.pr_number))
    summary = html.escape(args.summary[:600])
    (destination / "index.html").write_text(
        "<!doctype html><meta charset=utf-8>"
        f"<title>AMF screenshot proof — PR #{pr}</title>"
        "<style>body{margin:2rem auto;max-width:1100px;padding:0 1rem;"
        "background:#16181d;color:#e6e6e6;font:15px system-ui,sans-serif}"
        ".flow{margin:1rem 0 2rem;padding:1rem;border-left:3px solid #6ea8fe;"
        "background:#20242c}figure{margin:0 0 2rem}img{display:block;width:100%;"
        "height:auto;border:1px solid #424955;border-radius:8px}"
        "figcaption{padding:.65rem 0;color:#c7ced8}figcaption p{margin:.4rem 0 0}</style>"
        f"<h1>AMF screenshot proof — PR #{pr}</h1>"
        f"<section class=flow><strong>Flow under review</strong><p>{summary}</p></section>"
        "<h2>Evidence walkthrough</h2>" + "".join(figures),
        encoding="utf-8",
    )
    (destination / "_headers").write_text(HEADERS, encoding="utf-8")
    print(f"wrote {destination}/index.html ({len(images)} PNG, {len(gifs)} GIF)")


if __name__ == "__main__":
    main()
