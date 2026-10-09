#!/usr/bin/env python3
"""Run native debug-log viewer proof against an isolated GUI and SQLite DB."""
import os
import pathlib
import subprocess
import sys

scripts = pathlib.Path(__file__).resolve().parent
env = os.environ | {
    "AMF_GUI_CAPTURE_FRAMES": str(scripts / "capture-gui-debug-log-frames.py"),
}
subprocess.run(
    ["/usr/bin/python3", str(scripts / "capture-gui-composer.py"), sys.argv[1]],
    env=env,
    check=True,
)
