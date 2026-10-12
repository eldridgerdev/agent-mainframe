#!/usr/bin/env bash
# Render .ansi captures to same-named PNGs: with xterm.js (render_xterm.cjs)
# where Node and a headless Chromium are available, otherwise with the Pillow
# renderer (render_ansi.py). One renderer draws the whole set, so the frames
# of a run always match each other.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
COLS=120
ROWS=40
SCALE=2
RENDERER=auto
FILES=()

usage() {
    cat <<EOF
Usage: $(basename "$0") [options] <capture.ansi>...

Options:
  --cols <n>, --rows <n>  Grid size; match the capture's --geometry
                          (default: ${COLS}x${ROWS})
  --scale <n>             Pixel density multiplier (default: $SCALE)
  --renderer <which>      auto   xterm.js, falling back to Pillow (default)
                          xterm  xterm.js only; fail if it cannot run
                          pillow render_ansi.py only
  -h, --help              Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cols) COLS="$2"; shift 2 ;;
        --rows) ROWS="$2"; shift 2 ;;
        --scale) SCALE="$2"; shift 2 ;;
        --renderer) RENDERER="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        --*) echo "error: unknown option: $1" >&2; usage >&2; exit 2 ;;
        *) FILES+=("$1"); shift ;;
    esac
done

case "$RENDERER" in
    auto|xterm|pillow) ;;
    *) echo "error: --renderer must be auto, xterm, or pillow" >&2; exit 2 ;;
esac
if [[ ${#FILES[@]} -eq 0 ]]; then
    echo "error: no .ansi captures given" >&2
    usage >&2
    exit 2
fi

if [[ "$RENDERER" != pillow ]]; then
    xterm_args=(--cols "$COLS" --rows "$ROWS" --scale "$SCALE")
    # auto never downloads: it uses xterm.js only where its packages are
    # already installed, and otherwise falls back at once instead of waiting
    # out an npm install on every call. Asking for xterm by name fetches them.
    [[ "$RENDERER" == auto ]] && xterm_args+=(--no-install)
    if command -v node >/dev/null 2>&1 && node "$SCRIPT_DIR/render_xterm.cjs" \
        "${xterm_args[@]}" "${FILES[@]}"; then
        exit 0
    fi
    if [[ "$RENDERER" == xterm ]]; then
        echo "error: the xterm.js renderer could not run" >&2
        exit 1
    fi
    echo "warning: the xterm.js renderer could not run; using render_ansi.py instead." >&2
    echo "         'node $SCRIPT_DIR/render_xterm.cjs --install' sets it up." >&2
    # A workflow's frames are read from its gallery, not its job log, so the
    # fallback is annotated on the run. On stderr: amf-capture.sh --gif
    # discards this script's stdout.
    if [[ "${GITHUB_ACTIONS:-}" == true ]]; then
        echo "::warning title=Screenshots rendered with Pillow::The xterm.js renderer could not run, so these frames were drawn by render_ansi.py; colours and dim text differ from the desktop app." >&2
    fi
fi

for file in "${FILES[@]}"; do
    python3 "$SCRIPT_DIR/render_ansi.py" "$file" --out "${file%.ansi}.png" \
        --cols "$COLS" --rows "$ROWS" --scale "$SCALE"
done
