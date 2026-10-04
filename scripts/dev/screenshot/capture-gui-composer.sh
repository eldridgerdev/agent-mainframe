#!/usr/bin/env bash
# Use the established native GUI build/display wrapper with a private tmux fixture.
# The wrapper's post-capture step moves the TUI setup frame, which
# scenarios/gui-agent-composer.txt shoots as `gui-capture-ready` like every GUI
# scenario. Frame notes are keyed `.ansi` because build_static_gallery.py looks
# each PNG's note up under that suffix.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export AMF_GUI_CAPTURE_RUNNER="$SCRIPT_DIR/capture-gui-composer.py"
export AMF_GUI_CAPTURE_FRAMES="$SCRIPT_DIR/capture-gui-composer-frames.py"
bash "$SCRIPT_DIR/capture-gui-diff.sh" "${1:?Pass an output directory}"
