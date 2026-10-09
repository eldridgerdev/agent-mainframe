#!/usr/bin/env python3
"""Capture theme selection in a private native GUI with offline terminals."""
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
