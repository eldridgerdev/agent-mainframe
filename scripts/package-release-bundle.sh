#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 <target> <amf-binary> <output-archive>" >&2
  exit 1
}

[ $# -eq 3 ] || usage

TARGET="$1"
AMF_BIN="$2"
OUT_ARCHIVE="$3"

[ -f "$AMF_BIN" ] || {
  echo "amf binary not found: $AMF_BIN" >&2
  exit 1
}

BUNDLE_NAME="$(basename "${OUT_ARCHIVE%.tar.gz}")"
WORK_DIR="$(mktemp -d)"
BUNDLE_DIR="$WORK_DIR/$BUNDLE_NAME"

cleanup() {
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

mkdir -p "$BUNDLE_DIR"
cp "$AMF_BIN" "$BUNDLE_DIR/amf"
chmod +x "$BUNDLE_DIR/amf"

write_linux_wrapper() {
  cat >"$BUNDLE_DIR/tmux" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

if [[ -d "$HERE/tmux-root/usr/share/terminfo" ]]; then
  export TERMINFO_DIRS="$HERE/tmux-root/usr/share/terminfo${TERMINFO_DIRS:+:$TERMINFO_DIRS}"
fi

lib_dirs=()
for base in "$HERE/tmux-root/lib" "$HERE/tmux-root/usr/lib"; do
  if [[ -d "$base" ]]; then
    while IFS= read -r -d '' dir; do
      lib_dirs+=("$dir")
    done < <(find "$base" -type d -print0)
  fi
done

# Use the bundled dynamic linker if present so bundled libc and ld-linux are
# always a matched pair (fixes glibc version mismatches on the host).
LD_LINUX=""
for candidate in \
    "$HERE/tmux-root/usr/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" \
    "$HERE/tmux-root/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2" \
    "$HERE/tmux-root/usr/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1" \
    "$HERE/tmux-root/lib/aarch64-linux-gnu/ld-linux-aarch64.so.1"; do
  if [[ -f "$candidate" ]]; then
    LD_LINUX="$candidate"
    break
  fi
done

if [[ -n "$LD_LINUX" && ${#lib_dirs[@]} -gt 0 ]]; then
  ld_path="$(IFS=:; echo "${lib_dirs[*]}")"
  exec "$LD_LINUX" --library-path "$ld_path" "$HERE/tmux-real" "$@"
fi

# Fallback: no bundled linker, try with LD_LIBRARY_PATH
if [[ ${#lib_dirs[@]} -gt 0 ]]; then
  ld_path="$(IFS=:; echo "${lib_dirs[*]}")"
  export LD_LIBRARY_PATH="${ld_path}${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi

exec "$HERE/tmux-real" "$@"
EOF
  chmod +x "$BUNDLE_DIR/tmux"
}

write_macos_wrapper() {
  cat >"$BUNDLE_DIR/tmux" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

if [[ -d "$HERE/lib" ]]; then
  export DYLD_LIBRARY_PATH="$HERE/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
fi

exec "$HERE/tmux-real" "$@"
EOF
  chmod +x "$BUNDLE_DIR/tmux"
}

bundle_linux_tmux() {
  local arch="$1"
  local extract_dir="$WORK_DIR/tmux-root"
  local deb_dir="$WORK_DIR/debs"
  local host_arch
  local package_suffix=":$arch"

  mkdir -p "$extract_dir" "$deb_dir"

  host_arch="$(dpkg --print-architecture)"

  if [[ "$host_arch" == "$arch" ]]; then
    package_suffix=""
  elif [[ "$arch" == "arm64" ]]; then
    sudo dpkg --add-architecture arm64
  fi

  sudo apt-get update

  packages=()
  while IFS= read -r pkg; do
    [[ -n "$pkg" ]] || continue
    packages+=("$pkg")
  done < <(
    python3 - "$arch" "$package_suffix" <<'PY'
import subprocess
import sys

arch = sys.argv[1]
package_suffix = sys.argv[2]
cmd = [
    "apt-cache",
    "depends",
    "--recurse",
    "--no-recommends",
    "--no-suggests",
    "--no-conflicts",
    "--no-breaks",
    "--no-replaces",
    "--no-enhances",
    f"tmux{package_suffix}",
]
out = subprocess.check_output(cmd, text=True)
seen = set()
pkgs = []

for raw_line in out.splitlines():
    line = raw_line.strip()
    if not line or line.startswith(("<", "|")):
        continue
    if line.startswith(("Depends:", "PreDepends:")):
        pkg = line.split(":", 1)[1].strip()
    elif ":" in line:
        continue
    else:
        pkg = line
    if not pkg or pkg.startswith("<") or pkg in seen:
        continue
    seen.add(pkg)
    pkgs.append(pkg)

print("\n".join(pkgs))
PY
  )

  if [[ ${#packages[@]} -eq 0 ]]; then
    echo "Failed to resolve tmux packages for $arch" >&2
    exit 1
  fi

  pushd "$deb_dir" >/dev/null
  for pkg in "${packages[@]}"; do
    apt-get download "${pkg}${package_suffix}"
  done
  for deb in ./*.deb; do
    dpkg-deb -x "$deb" "$extract_dir"
  done
  popd >/dev/null

  cp "$extract_dir/usr/bin/tmux" "$BUNDLE_DIR/tmux-real"
  chmod +x "$BUNDLE_DIR/tmux-real"
  mv "$extract_dir" "$BUNDLE_DIR/tmux-root"
  write_linux_wrapper
}

bundle_macos_tmux() {
  local tmux_bin
  local brew_prefix
  local lib_dir="$BUNDLE_DIR/lib"
  local -a queue
  local -a deps
  local item
  local dep

  mkdir -p "$lib_dir"

  brew_prefix="$(brew --prefix)"
  tmux_bin="$(brew --prefix tmux)/bin/tmux"

  cp "$tmux_bin" "$BUNDLE_DIR/tmux-real"
  chmod +x "$BUNDLE_DIR/tmux-real"
  queue=("$tmux_bin")

  while [[ ${#queue[@]} -gt 0 ]]; do
    item="${queue[0]}"
    queue=("${queue[@]:1}")
    [[ -n "$item" ]] || continue
    [[ -f "$item" ]] || continue

    deps=()
    while IFS= read -r dep; do
      [[ -n "$dep" ]] || continue
      deps+=("$dep")
    done < <(otool -L "$item" | awk 'NR > 1 {print $1}')
    for dep in "${deps[@]}"; do
      case "$dep" in
        "$brew_prefix"/*)
          if [[ -f "$dep" && ! -f "$lib_dir/$(basename "$dep")" ]]; then
            cp -f "$dep" "$lib_dir/$(basename "$dep")"
            queue+=("$dep")
          fi
          ;;
      esac
    done
  done

  write_macos_wrapper
}

case "$TARGET" in
  x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl)
    bundle_linux_tmux amd64
    ;;
  aarch64-unknown-linux-gnu)
    bundle_linux_tmux arm64
    ;;
  aarch64-apple-darwin)
    bundle_macos_tmux
    ;;
  *)
    echo "Unsupported release target: $TARGET" >&2
    exit 1
    ;;
esac

tar -C "$WORK_DIR" -czf "$OUT_ARCHIVE" "$BUNDLE_NAME"
echo "Created $OUT_ARCHIVE"
