#!/usr/bin/env bash
# Native desktop capture invoked by the ordinary isolated screenshot scenario.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT_DIR="${1:?Pass an output directory}"
CAPTURE_RUNNER="${AMF_GUI_CAPTURE_RUNNER:-$SCRIPT_DIR/capture-gui-diff.py}"

# The capture-only CI job checks out the requested ref and holds no Pages token.
# Install native GUI dependencies here, so TUI-only captures stay lightweight.
if [[ "${CI:-}" == "true" ]]; then
  sudo apt-get update
  sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev xvfb xauth \
    python3-gi gir1.2-gtk-3.0 python3-websocket python3-xlib dbus-x11 bubblewrap
fi
npm --prefix "$REPO_ROOT/gui" ci --no-audit --no-fund
npm --prefix "$REPO_ROOT/gui" run build
cargo build --manifest-path "$REPO_ROOT/Cargo.toml" -p amf-gui --locked

if [[ "${CI:-}" == "true" || -z "${DISPLAY:-}" ]]; then
  # A runner's DISPLAY variable does not guarantee a usable desktop. Give
  # this capture its own X server and D-Bus session. WebKit's nested process
  # sandbox is disabled only on the isolated, unprivileged capture runner.
  xvfb-run --auto-servernum --server-args='-screen 0 1600x1000x24' \
    dbus-run-session -- env GDK_BACKEND=x11 \
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
    LIBGL_ALWAYS_SOFTWARE=1 \
    /usr/bin/python3 "$CAPTURE_RUNNER" "$OUT_DIR"
else
  GDK_BACKEND=x11 /usr/bin/python3 "$CAPTURE_RUNNER" "$OUT_DIR"
fi

# Keep the TUI setup assertion internal. Publish only the native GUI frames.
mkdir -p "$OUT_DIR/setup"
for extension in ansi txt; do
  mv "$OUT_DIR/001-gui-capture-ready.$extension" "$OUT_DIR/setup/"
done
