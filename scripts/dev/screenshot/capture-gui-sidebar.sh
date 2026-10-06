#!/usr/bin/env bash
# Use the established native GUI build/display wrapper with the sidebar-parity
# fixtures. scenarios/gui-sidebar-parity.txt shoots the TUI setup frame as
# `gui-capture-ready` like every GUI scenario; the wrapper moves it aside.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export AMF_GUI_CAPTURE_RUNNER="$SCRIPT_DIR/capture-gui-sidebar.py"
export AMF_GUI_CAPTURE_FRAMES="$SCRIPT_DIR/capture-gui-sidebar-frames.py"
bash "$SCRIPT_DIR/capture-gui-diff.sh" "${1:?Pass an output directory}"
