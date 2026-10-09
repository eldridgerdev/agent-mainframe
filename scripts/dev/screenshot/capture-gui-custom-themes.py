#!/usr/bin/env python3
"""Capture theme selection in a private native GUI with offline terminals."""
import json
import os
import pathlib
import subprocess
import sys

scripts = pathlib.Path(__file__).resolve().parent
env = os.environ | {
    "AMF_GUI_CAPTURE_HARNESS": str(scripts / "fixtures/gui-theme-harness.py"),
    "AMF_GUI_CAPTURE_FRAMES": str(scripts / "capture-gui-custom-themes-frames.py"),
}
subprocess.run(["/usr/bin/python3", str(scripts / "capture-gui-composer.py"), sys.argv[1]], env=env, check=True)

# The gallery publisher associates each rendered image with its ANSI-name key.
# Native frames are already PNGs, so normalize only the notes, not the images.
notes_path = pathlib.Path(sys.argv[1]) / "capture-notes.jsonl"
notes = [json.loads(line) for line in notes_path.read_text().splitlines()]
for note in notes:
    note["file"] = pathlib.Path(note["file"]).with_suffix(".ansi").name
notes_path.write_text("".join(json.dumps(note) + "\n" for note in notes))
