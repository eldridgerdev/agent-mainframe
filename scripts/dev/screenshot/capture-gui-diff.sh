#!/usr/bin/env bash
# Native desktop capture invoked by the ordinary isolated screenshot scenario.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT_DIR="${1:?Pass an output directory}"

# The capture-only CI job checks out the requested ref and holds no Pages token.
# Install native GUI dependencies here, so TUI-only captures stay lightweight.
if [[ "${CI:-}" == "true" ]]; then
  sudo apt-get update
  sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev xvfb xauth \
    python3-gi gir1.2-gtk-3.0 python3-websocket python3-xlib
fi
npm --prefix "$REPO_ROOT/gui" ci --no-audit --no-fund
npm --prefix "$REPO_ROOT/gui" run build
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p amf-gui --locked

if [[ -n "${DISPLAY:-}" ]]; then
  GDK_BACKEND=x11 /usr/bin/python3 "$SCRIPT_DIR/capture-gui-diff.py" "$OUT_DIR"
else
  xvfb-run --auto-servernum --server-args='-screen 0 1600x1000x24' \
    env GDK_BACKEND=x11 /usr/bin/python3 "$SCRIPT_DIR/capture-gui-diff.py" "$OUT_DIR"
fi

# Keep the TUI setup assertion internal. Publish only the native GUI frames.
mkdir -p "$OUT_DIR/setup"
for extension in ansi txt; do
  mv "$OUT_DIR/001-gui-capture-ready.$extension" "$OUT_DIR/setup/"
done
