#!/usr/bin/env python3
"""Use the isolated live-session fixture to validate native prompt-library IPC."""
import json
import os
import pathlib
import runpy
import sys
import urllib.parse

scripts = pathlib.Path(__file__).resolve().parent
# The GUI loads the devUrl compiled into it, whatever port Vite is given: 1420
# unless the build had a TAURI_CONFIG override, which the scenario's run: line
# supplies together with the port. Serving any other port leaves the window
# blank until the frames script times out, so follow the build and refuse a
# mismatch up front.
dev_url = json.loads(os.environ.get("TAURI_CONFIG") or "{}").get("build", {}).get("devUrl", "http://localhost:1420")
built_port = str(urllib.parse.urlsplit(dev_url).port)
if os.environ.setdefault("AMF_GUI_CAPTURE_PORT", built_port) != built_port:
    sys.exit(
        f"AMF_GUI_CAPTURE_PORT={os.environ['AMF_GUI_CAPTURE_PORT']}, but the GUI build loads {dev_url}. "
        "Set TAURI_CONFIG's build.devUrl to that port and rebuild amf-gui, or unset the port."
    )
os.environ.setdefault("AMF_GUI_CAPTURE_PROOF", "Prompt library insertion")
os.environ.setdefault("AMF_GUI_CAPTURE_FRAMES", str(scripts / "capture-gui-prompt-library-frames.py"))
runpy.run_path(str(scripts / "capture-gui-bookmarks.py"), run_name="__main__")
