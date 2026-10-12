#!/usr/bin/env bash
# Build an actual debug AppImage with embedded frontend assets, without FUSE.
set -euo pipefail
[[ -z "${TAURI_CONFIG:-}" ]] || { echo 'Unset TAURI_CONFIG to build the embedded frontend without overrides' >&2; exit 1; }
root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
out="${1:?Usage: build-debug-appimage.sh OUTPUT_DIRECTORY}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
source_ref="$(git -C "$root" rev-parse HEAD)"
git -C "$root" diff --quiet HEAD -- src gui/src gui/src-tauri Cargo.toml Cargo.lock || {
  echo 'Product source differs from the recorded ref' >&2; exit 1;
}
build_tmp="$(mktemp -d /tmp/amf-appimage-build.XXXXXX)"
trap 'rm -rf "$build_tmp"' EXIT
# Tauri builds embedded assets; devUrl and Vite are deliberately unused.
touch "$build_tmp/started"
cd "$root/gui"
# This target must be exclusively owned. Cargo can retain another worktree's
# product rlib in a shared target; keep dependency caches but rebuild both AMF packages.
cargo clean --package agent-mainframe --package amf-gui
TMPDIR="$build_tmp" APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri -- build --ci --debug --bundles appimage -- --locked
build_target="${CARGO_TARGET_DIR:-$root/target}"
image="$(find "$build_target/debug/bundle/appimage" -maxdepth 1 -type f -name '*.AppImage' -newer "$build_tmp/started" -print -quit)"
[[ -n "$image" ]] || { echo 'No debug AppImage produced' >&2; exit 1; }
artifact="$out/amf-gui-debug-${source_ref:0:8}.AppImage"
cp "$image" "$artifact"
chmod +x "$artifact"
# Require the actual packaged debug binary to contain the fix, not just source.
(cd "$build_tmp" && "$artifact" --appimage-extract usr/bin/amf-gui > extract.log)
nm -C "$build_tmp/squashfs-root/usr/bin/amf-gui" > "$build_tmp/symbols.txt"
rg -q 'agent_mainframe::tmux::configure_tmux_python_environment' "$build_tmp/symbols.txt" || {
  echo 'Packaged binary does not contain the AppImage Python fix' >&2; exit 1;
}
python3 - "$artifact" "$source_ref" <<'PY'
import hashlib,json,pathlib,sys
image=pathlib.Path(sys.argv[1])
report={'source_ref':sys.argv[2],'artifact':str(image),'sha256':hashlib.sha256(image.read_bytes()).hexdigest(),'profile':'debug','product_packages_rebuilt':True,'packaged_fix_symbol_present':True,'frontend':'embedded production assets','execution':'extract-and-run; no FUSE or desktop installation'}
(image.parent/'build-report.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
PY
