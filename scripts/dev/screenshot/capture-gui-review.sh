#!/usr/bin/env bash
# Reuse the native GUI capture's isolated database, Git fixture and cleanup.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
export AMF_GUI_CAPTURE_FRAMES="$SCRIPT_DIR/capture-gui-review-frames.py"
bash "$SCRIPT_DIR/capture-gui-diff.sh" "${1:?Pass an output directory}"
