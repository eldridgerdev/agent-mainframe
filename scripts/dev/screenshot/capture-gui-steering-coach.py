#!/usr/bin/env python3
"""Reuse the private Git/SQLite/tmux fixture for native steering-coach proof."""
import os
import pathlib
import runpy

scripts = pathlib.Path(__file__).resolve().parent
os.environ.setdefault("AMF_GUI_CAPTURE_PORT", "1442")
os.environ.setdefault("AMF_GUI_CAPTURE_PROOF", "Steering coach checkbox and advisory panel")
os.environ.setdefault("AMF_GUI_CAPTURE_FRAMES", str(scripts / "capture-gui-steering-coach-frames.py"))
runpy.run_path(str(scripts / "capture-gui-bookmarks.py"), run_name="__main__")
