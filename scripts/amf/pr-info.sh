#!/usr/bin/env bash
# Gathers branch context for PR creation/updates.
# Outputs structured text with commits, diff stats, and changed files.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Detect base branch
detect_base_branch() {
    for branch in master main; do
        if git rev-parse --verify "origin/$branch" &>/dev/null; then
            echo "$branch"
            return
        fi
    done
    echo "master"
}

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
BASE_BRANCH="$(detect_base_branch)"
MERGE_BASE="$(git merge-base "origin/$BASE_BRANCH" HEAD 2>/dev/null || echo "")"

if [ -z "$MERGE_BASE" ]; then
    echo "ERROR: Could not find merge base with origin/$BASE_BRANCH"
    echo "Make sure you have fetched from origin."
    exit 1
fi

echo "=== Branch Info ==="
echo "Current branch: $CURRENT_BRANCH"
echo "Base branch: $BASE_BRANCH"
echo ""

echo "=== Commits ==="
git log --oneline "$MERGE_BASE..HEAD"
echo ""

echo "=== Diff Stats ==="
git diff --stat "$MERGE_BASE..HEAD"
echo ""

echo "=== Changed Files ==="
git diff --name-only "$MERGE_BASE..HEAD"
echo ""

# Check if branch is pushed
if git rev-parse --verify "origin/$CURRENT_BRANCH" &>/dev/null; then
    LOCAL="$(git rev-parse HEAD)"
    REMOTE="$(git rev-parse "origin/$CURRENT_BRANCH")"
    if [ "$LOCAL" = "$REMOTE" ]; then
        echo "=== Push Status ==="
        echo "Branch is up to date with origin."
    else
        echo "=== Push Status ==="
        echo "Branch has unpushed commits."
    fi
else
    echo "=== Push Status ==="
    echo "Branch has not been pushed to origin."
fi
