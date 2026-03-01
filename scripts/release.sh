#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_TOML="$REPO_ROOT/Cargo.toml"

usage() {
  echo "Usage: $0 [major|minor|patch|<version>]" >&2
  echo "  major   bump major version (1.2.3 -> 2.0.0)" >&2
  echo "  minor   bump minor version (1.2.3 -> 1.3.0)" >&2
  echo "  patch   bump patch version (1.2.3 -> 1.2.4)" >&2
  echo "  <ver>   set explicit version (e.g. 1.5.0)" >&2
  exit 1
}

[ $# -eq 1 ] || usage
BUMP="$1"

# Ensure working tree is clean
if ! git -C "$REPO_ROOT" diff --quiet HEAD; then
  echo "Error: working tree is not clean. Commit or stash changes first." >&2
  exit 1
fi

# Parse current version from Cargo.toml
CURRENT=$(sed -n 's/^version = "\([^"]*\)".*/\1/p' "$CARGO_TOML" | head -1)
if [ -z "$CURRENT" ]; then
  echo "Error: could not parse version from $CARGO_TOML" >&2
  exit 1
fi

IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT"

# Compute new version
case "$BUMP" in
  major)
    NEW="$((MAJOR + 1)).0.0"
    ;;
  minor)
    NEW="$MAJOR.$((MINOR + 1)).0"
    ;;
  patch)
    NEW="$MAJOR.$MINOR.$((PATCH + 1))"
    ;;
  [0-9]*)
    NEW="$BUMP"
    ;;
  *)
    usage
    ;;
esac

echo "Bumping version: $CURRENT -> $NEW"

# Update Cargo.toml (first occurrence of version = "...")
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW\"/" "$CARGO_TOML"

# Update Cargo.lock
cargo check --quiet --manifest-path "$CARGO_TOML"

# Commit and tag
git -C "$REPO_ROOT" add Cargo.toml Cargo.lock
git -C "$REPO_ROOT" commit -m "chore: bump version to v$NEW"
git -C "$REPO_ROOT" tag -a "v$NEW" -m "Release v$NEW"

echo ""
echo "Done. Push the commit and tag with:"
echo "  git push && git push --tags"
