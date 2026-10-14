#!/usr/bin/env python3
"""Exercise current GUI startup with released AppRun Python redirects."""
import os
import pathlib
import runpy
scripts = pathlib.Path(__file__).resolve().parent
os.environ.setdefault("AMF_GUI_CAPTURE_PORT", "1441")
os.environ["AMF_GUI_CAPTURE_FRESH_SERVER_PYTHON"] = "1"
os.environ.setdefault("AMF_GUI_CAPTURE_PROOF", "AppImage Python environment")
os.environ.setdefault("AMF_GUI_CAPTURE_FRAMES", str(scripts / "capture-gui-appimage-python-frames.py"))
runpy.run_path(str(scripts / "capture-gui-bookmarks.py"), run_name="__main__")
