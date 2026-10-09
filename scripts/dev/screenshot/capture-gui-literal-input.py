#!/usr/bin/env python3
"""Run native literal-input proof with private fixtures and command auditing."""
import json
import os
import pathlib
import shutil
import subprocess
import sys

scripts = pathlib.Path(__file__).resolve().parent
# Avoid the outer screenshot harness's PATH wrappers when auditing GUI calls.
real_tmux = shutil.which("tmux", path="/usr/bin:/bin:/opt/homebrew/bin:/usr/local/bin")
assert real_tmux, "Install tmux before capturing"
env = os.environ | {
    "AMF_GUI_CAPTURE_HARNESS": str(scripts / "fixtures/gui-literal-input-harness.py"),
    "AMF_GUI_CAPTURE_FRAMES": str(scripts / "capture-gui-literal-input-frames.py"),
    "AMF_GUI_CAPTURE_REAL_TMUX": real_tmux,
    "AMF_TMUX_BIN": str(scripts / "fixtures/gui-literal-tmux.py"),
    "AMF_TMUX_INPUT_TRANSPORT": "control-pty",
}
subprocess.run(["/usr/bin/python3", str(scripts / "capture-gui-composer.py"), sys.argv[1]], env=env, check=True)
notes_path = pathlib.Path(sys.argv[1]) / "capture-notes.jsonl"
notes = [json.loads(line) for line in notes_path.read_text().splitlines()]
for note in notes:
    note["file"] = pathlib.Path(note["file"]).with_suffix(".ansi").name
notes_path.write_text("".join(json.dumps(note) + "\n" for note in notes))
