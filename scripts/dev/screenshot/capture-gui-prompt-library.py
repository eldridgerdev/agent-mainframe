#!/usr/bin/env python3
"""Use the isolated live-session fixture to validate native prompt-library IPC."""
import os
import pathlib
import runpy

scripts = pathlib.Path(__file__).resolve().parent
os.environ.setdefault("AMF_GUI_CAPTURE_PORT", "1441")
os.environ.setdefault("AMF_GUI_CAPTURE_FRAMES", str(scripts / "capture-gui-prompt-library-frames.py"))
runpy.run_path(str(scripts / "capture-gui-bookmarks.py"), run_name="__main__")
