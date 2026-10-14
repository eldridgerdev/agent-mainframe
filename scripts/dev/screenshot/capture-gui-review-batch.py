#!/usr/bin/env python3
"""Reuse the private Git/SQLite/tmux fixture for native review batch proof."""
import os
import pathlib
import runpy

scripts = pathlib.Path(__file__).resolve().parent
os.environ.setdefault("AMF_GUI_CAPTURE_PORT", "1441")
os.environ.setdefault("AMF_GUI_CAPTURE_FRAMES", str(scripts / "capture-gui-review-batch-frames.py"))
runpy.run_path(str(scripts / "capture-gui-bookmarks.py"), run_name="__main__")
