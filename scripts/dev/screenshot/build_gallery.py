#!/usr/bin/env python3
"""Build a self-contained HTML gallery and metadata manifest for a capture run."""

import argparse
import base64
import json
import mimetypes
from datetime import datetime, timezone
from html import escape
from pathlib import Path


def image_data_uri(path: Path) -> str:
    mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    encoded = base64.b64encode(path.read_bytes()).decode("ascii")
    return f"data:{mime};base64,{encoded}"


def label_for(path: Path) -> str:
    stem = path.stem
    if "-" in stem and stem[:3].isdigit():
        return stem[4:].replace("-", " ")
    return stem.replace("-", " ")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--input-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path, help="HTML gallery path")
    ap.add_argument("--repository", required=True)
    ap.add_argument("--ref", required=True)
    ap.add_argument("--sha", required=True)
    ap.add_argument("--scenario", required=True)
    ap.add_argument("--geometry", required=True)
    ap.add_argument("--pr-number", required=True, type=int)
    ap.add_argument("--run-id", required=True)
    ap.add_argument("--artifact-name", required=True)
    ap.add_argument("--retention-days", required=True, type=int)
    args = ap.parse_args()

    input_dir = args.input_dir.resolve()
    frames = sorted(input_dir.glob("*.png"))
    gifs = sorted(input_dir.glob("*.gif"))
    ansi_by_stem = {path.stem: path.name for path in input_dir.glob("*.ansi")}
    text_by_stem = {path.stem: path.name for path in input_dir.glob("*.txt")}

    captures = []
    figures = []
    for frame in frames:
        label = label_for(frame)
        captures.append(
            {
                "label": label,
                "png": frame.name,
                "ansi": ansi_by_stem.get(frame.stem),
                "text": text_by_stem.get(frame.stem),
            }
        )
        figures.append(
            "<figure><div class=\"terminal\"><div class=\"bar\">AMF screenshot</div>"
            f"<img src=\"{image_data_uri(frame)}\" alt=\"{escape(label)}\"></div>"
            f"<figcaption>{escape(label)}</figcaption></figure>"
        )

    gif_entry = None
    if gifs:
        gif = gifs[0]
        gif_entry = {"file": gif.name, "label": label_for(gif)}

    metadata = {
        "schema": 1,
        "captured_at": datetime.now(timezone.utc).isoformat(),
        "repository": args.repository,
        "pr_number": args.pr_number,
        "ref": args.ref,
        "sha": args.sha,
        "scenario": args.scenario,
        "geometry": args.geometry,
        "workflow_run_id": args.run_id,
        "artifact_name": args.artifact_name,
        "retention_days": args.retention_days,
        "captures": captures,
        "gif": gif_entry,
    }
    (input_dir / "capture-metadata.json").write_text(
        json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
    )

    title = f"AMF screenshot proof — PR #{args.pr_number}"
    context = (
        f"{args.repository} · ref {args.ref} · {args.geometry} · "
        f"scenario {args.scenario} · artifact retention {args.retention_days} days"
    )
    html = f"""<!doctype html>
<meta charset="utf-8">
<title>{escape(title)}</title>
<style>
body {{ margin: 2rem auto; max-width: 1100px; padding: 0 1rem; background: #16181d; color: #e6e6e6; font: 15px system-ui, sans-serif; }}
h1 {{ margin-bottom: .25rem; }}
.context {{ color: #a9b1bd; margin-bottom: 2rem; }}
figure {{ margin: 0 0 2rem; }}
.terminal {{ border: 1px solid #424955; border-radius: 8px; overflow: hidden; background: #1e1e1e; box-shadow: 0 8px 22px #0005; }}
.bar {{ padding: .45rem .8rem; background: #2a2e37; color: #a9b1bd; font: 12px ui-monospace, monospace; }}
img {{ display: block; width: 100%; height: auto; }}
figcaption {{ padding: .5rem 0; color: #c7ced8; }}
</style>
<h1>{escape(title)}</h1>
<p class="context">{escape(context)}</p>
{''.join(figures)}
"""
    args.out.write_text(html, encoding="utf-8")
    print(f"wrote {args.out} ({len(frames)} PNG frames)")


if __name__ == "__main__":
    main()
