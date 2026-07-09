#!/usr/bin/env bash
# Gathers current PR status including checks and reviews.
# Requires the gh CLI to be installed and authenticated.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"

# Check if a PR exists for this branch
if ! gh pr view --json number &>/dev/null 2>&1; then
    echo "No pull request found for branch: $CURRENT_BRANCH"
    echo ""
    echo "To create one, use /amf:pr-create"
    exit 0
fi

echo "=== PR Info ==="
gh pr view --json number,title,state,url,baseRefName \
    --template '{{printf "PR #%v: %s\nState: %s\nBase: %s\nURL: %s\n" .number .title .state .baseRefName .url}}'
echo ""

echo "=== CI Checks ==="
gh pr checks 2>/dev/null || echo "No checks configured."
echo ""

echo "=== Reviews ==="
gh pr view --json reviews \
    --template '{{range .reviews}}{{printf "%s: %s\n" .author.login .state}}{{end}}' \
    2>/dev/null || echo "No reviews yet."
echo ""

echo "=== Mergeable Status ==="
gh pr view --json mergeable,mergeStateStatus \
    --template '{{printf "Mergeable: %s\nMerge state: %s\n" .mergeable .mergeStateStatus}}' \
    2>/dev/null || true
