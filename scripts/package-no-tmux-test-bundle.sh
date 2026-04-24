#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: package-no-tmux-test-bundle.sh <amf-binary> <output-archive>

Builds a runnable test archive for the repo's no-tmux Docker image.
It wraps the provided amf binary with its runtime loader and shared
libraries, then copies the already-bundled tmux directory from the
current tmux installation.
EOF
  exit 1
}

[ $# -eq 2 ] || usage

amf_bin="$1"
out_archive="$2"

[ -f "$amf_bin" ] || {
  echo "amf binary not found: $amf_bin" >&2
  exit 1
}

tmux_bin="$(command -v tmux || true)"
[ -n "$tmux_bin" ] || {
  echo "tmux not found in PATH" >&2
  exit 1
}

tmux_bundle_dir="$(cd "$(dirname "$tmux_bin")" && pwd)"
if [[ ! -f "$tmux_bundle_dir/tmux" ]]; then
  echo "expected tmux wrapper next to tmux binary: $tmux_bundle_dir/tmux" >&2
  exit 1
fi

bundle_name="$(basename "${out_archive%.tar.gz}")"
work_dir="$(mktemp -d)"
bundle_dir="$work_dir/$bundle_name"

cleanup() {
  rm -rf "$work_dir"
}
trap cleanup EXIT

mkdir -p "$bundle_dir"

copy_tree() {
  local src="$1"
  local dest="$2"
  mkdir -p "$(dirname "$dest")"
  cp -a "$src" "$dest"
}

amf_root="$bundle_dir/amf-root"
mkdir -p "$amf_root"
cp "$amf_bin" "$bundle_dir/amf-real"
chmod +x "$bundle_dir/amf-real"

mapfile -t amf_deps < <(
  ldd "$amf_bin" \
    | awk '{for (i = 1; i <= NF; i++) if ($i ~ /^\//) print $i}' \
    | sed 's/[()]//g' \
    | sort -u
)

if [[ ${#amf_deps[@]} -eq 0 ]]; then
  echo "failed to resolve runtime dependencies for $amf_bin" >&2
  exit 1
fi

amf_loader=""
amf_lib_dirs=()
for dep in "${amf_deps[@]}"; do
  if [[ "$dep" == *ld-linux* || "$dep" == *ld-musl* ]]; then
    amf_loader="$dep"
  else
    amf_lib_dirs+=("$(dirname "$dep")")
  fi
  copy_tree "$dep" "$amf_root$dep"
done

amf_lib_dirs=($(printf '%s\n' "${amf_lib_dirs[@]}" | sort -u))

cat >"$bundle_dir/amf" <<EOF
#!/usr/bin/env bash
set -euo pipefail
HERE="\$(cd "\$(dirname "\$0")" && pwd)"

amf_root="\$HERE/amf-root"
amf_real="\$HERE/amf-real"
loader="\$amf_root${amf_loader}"
lib_dirs=(
$(for dir in "${amf_lib_dirs[@]}"; do printf '  "%s/%s"\n' '$amf_root' "${dir#/}"; done)
)

if [[ -x "\$loader" && \${#lib_dirs[@]} -gt 0 ]]; then
  ld_path="\$(IFS=:; echo "\${lib_dirs[*]}")"
  exec "\$loader" --library-path "\$ld_path" "\$amf_real" "\$@"
fi

if [[ \${#lib_dirs[@]} -gt 0 ]]; then
  ld_path="\$(IFS=:; echo "\${lib_dirs[*]}")"
  export LD_LIBRARY_PATH="\$ld_path\${LD_LIBRARY_PATH:+:\$LD_LIBRARY_PATH}"
fi

exec "\$amf_real" "\$@"
EOF
chmod +x "$bundle_dir/amf"

cp -a "$tmux_bundle_dir/." "$bundle_dir/"

tar -C "$work_dir" -czf "$out_archive" "$bundle_name"
echo "Created $out_archive"
