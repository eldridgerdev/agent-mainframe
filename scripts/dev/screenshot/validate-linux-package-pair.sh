#!/usr/bin/env bash
# Only released payloads are run here; no GUI source build or dev server.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${1:?Pass an output directory}"

if [[ "${CI:-}" == "true" ]]; then
  sudo apt-get update
  sudo apt-get install -y fakeroot dpkg libgtk-3-0 libwebkit2gtk-4.1-0 \
    xvfb xauth python3-gi gir1.2-gtk-3.0 python3-websocket python3-xlib \
    dbus-x11 bubblewrap
fi
/usr/bin/python3 "$SCRIPT_DIR/test_linux_package_pair.py"
args=("$OUT_DIR" --capture --tui-target "${AMF_PACKAGE_TUI_TARGET:-musl}")
[[ -z "${AMF_PACKAGE_ASSETS_DIR:-}" ]] || args+=(--assets-dir "$AMF_PACKAGE_ASSETS_DIR")

if [[ "${CI:-}" == "true" || -z "${DISPLAY:-}" ]]; then
  xvfb-run --auto-servernum --server-args='-screen 0 1600x1000x24' \
    dbus-run-session -- env GDK_BACKEND=x11 \
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
    LIBGL_ALWAYS_SOFTWARE=1 \
    /usr/bin/python3 "$SCRIPT_DIR/validate-linux-package-pair.py" "${args[@]}"
else
  GDK_BACKEND=x11 /usr/bin/python3 "$SCRIPT_DIR/validate-linux-package-pair.py" "${args[@]}"
fi

# The hosted wrapper's initial TUI assertion is internal setup evidence.
if [[ -f "$OUT_DIR/001-gui-capture-ready.ansi" ]]; then
  mkdir -p "$OUT_DIR/setup"
  for extension in ansi txt; do
    mv "$OUT_DIR/001-gui-capture-ready.$extension" "$OUT_DIR/setup/"
  done
fi
