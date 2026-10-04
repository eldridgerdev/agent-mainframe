#!/usr/bin/env bash
# Use the established native GUI build/display wrapper with a private tmux fixture.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export AMF_GUI_CAPTURE_RUNNER="$SCRIPT_DIR/capture-gui-composer.py"
export AMF_GUI_CAPTURE_FRAMES="$SCRIPT_DIR/capture-gui-composer-frames.py"
bash "$SCRIPT_DIR/capture-gui-diff.sh" "${1:?Pass an output directory}"
