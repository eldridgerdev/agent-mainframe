#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_TOML="$REPO_ROOT/Cargo.toml"
CHANGELOG="$REPO_ROOT/CHANGELOG.md"

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

echo "Current version: $CURRENT"

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

IFS='.' read -r NEW_MAJOR NEW_MINOR NEW_PATCH <<< "$NEW"

require_changelog=0
if [ "$NEW_MAJOR" -gt "$MAJOR" ]; then
  require_changelog=1
elif [ "$NEW_MAJOR" -eq "$MAJOR" ] && [ "$NEW_MINOR" -gt "$MINOR" ]; then
  require_changelog=1
fi

if [ "$require_changelog" -eq 1 ]; then
  if [ ! -f "$CHANGELOG" ]; then
    echo "Error: missing $CHANGELOG" >&2
    exit 1
  fi

  heading_regex="^## \\[v$NEW\\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$"
  if ! grep -Eq "$heading_regex" "$CHANGELOG"; then
    echo "Error: CHANGELOG.md must contain a section heading for v$NEW." >&2
    echo "Expected: ## [v$NEW] - YYYY-MM-DD" >&2
    exit 1
  fi

  section_body="$(
    awk -v version="v$NEW" '
      $0 ~ "^## \\[" version "\\] - " { capture=1; next }
      capture && $0 ~ "^## \\[" { exit }
      capture { print }
    ' "$CHANGELOG"
  )"

  meaningful_body="$(
    printf '%s\n' "$section_body" \
      | sed '/^[[:space:]]*$/d' \
      | sed '/^_No unreleased changes yet\\._$/d'
  )"

  if [ -z "$(printf '%s' "$meaningful_body" | tr -d '[:space:]')" ]; then
    echo "Error: CHANGELOG.md section for v$NEW is empty." >&2
    echo "Add user-facing release notes and migration guidance before releasing." >&2
    exit 1
  fi

  echo "Validated CHANGELOG.md entry for v$NEW"
fi

# Update Cargo.toml (first occurrence of version = "...")
sed -i "0,/^version = \"$CURRENT\"/s//version = \"$NEW\"/" "$CARGO_TOML"

# Update Cargo.lock
cargo check --quiet --manifest-path "$CARGO_TOML"

# Commit and tag
git -C "$REPO_ROOT" add Cargo.toml Cargo.lock CHANGELOG.md
git -C "$REPO_ROOT" commit -m "chore: bump version to v$NEW"
git -C "$REPO_ROOT" tag -a "v$NEW" -m "Release v$NEW"

echo ""
echo "Done. Push the commit and tag with:"
echo "  git push && git push --tags"
