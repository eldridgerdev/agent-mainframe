#!/usr/bin/env bash
#
# codex-diff-review.sh — Codex vibeless-mode file change watcher
#
# Watches a workdir for file writes made by Codex and sends
# change-reason notifications to a running AMF instance.
# When the user rejects a change, the file is reverted.
#
# Usage: codex-diff-review.sh <workdir>
#   AMF_SESSION env var must be set (done by AMF on launch).

set -uo pipefail

WORKDIR="${1:-$PWD}"
SESSION_ID="${AMF_SESSION:-}"

# Require all tools
for cmd in inotifywait amf jq git; do
    command -v "$cmd" >/dev/null 2>&1 || exit 0
done
[ -n "$SESSION_ID" ] || exit 0

cd "$WORKDIR"

# ── Main event loop ──────────────────────────────────────────────

inotifywait -m -r \
    --format '%w%f' \
    -e close_write \
    --exclude '/(\.git|\.codex|target|node_modules)/' \
    "$WORKDIR" 2>/dev/null \
| while IFS= read -r FILE; do

    [ -f "$FILE" ] || continue

    RELATIVE="${FILE#"$WORKDIR"/}"

    # Skip hidden paths (belt-and-suspenders with --exclude above)
    case "$RELATIVE" in
        .* | */.* ) continue ;;
    esac

    # Determine old content (from git HEAD) and tool type
    OLD_CONTENT="" TOOL="write"
    if git ls-files --error-unmatch "$FILE" >/dev/null 2>&1; then
        OLD_CONTENT="$(git show "HEAD:${RELATIVE}" 2>/dev/null || true)"
        TOOL="edit"
    fi

    NEW_CONTENT="$(cat "$FILE" 2>/dev/null || true)"

    # Skip if file is unchanged from HEAD
    [ "$OLD_CONTENT" != "$NEW_CONTENT" ] || continue

    # Build and send the change-reason notification, wait for response
    PAYLOAD="$(jq -nc \
        --arg session_id "$SESSION_ID" \
        --arg cwd "$WORKDIR" \
        --arg file_path "$FILE" \
        --arg relative_path "$RELATIVE" \
        --arg old_snippet "$OLD_CONTENT" \
        --arg new_snippet "$NEW_CONTENT" \
        --arg tool "$TOOL" \
        '{
            "type": "change-reason",
            "notification_type": "change-reason",
            "session_id": $session_id,
            "cwd": $cwd,
            "file_path": $file_path,
            "relative_path": $relative_path,
            "old_snippet": $old_snippet,
            "new_snippet": $new_snippet,
            "tool": $tool
        }')"

    RESPONSE="$(echo "$PAYLOAD" | amf notify-wait --timeout-ms 120000 2>/dev/null || true)"

    REJECTED="$(echo "$RESPONSE" \
        | jq -r '.response.reject // false' 2>/dev/null \
        || echo false)"

    if [ "$REJECTED" = "true" ]; then
        if git ls-files --error-unmatch "$FILE" >/dev/null 2>&1; then
            git checkout -- "$FILE" 2>/dev/null || true
        else
            rm -f "$FILE"
        fi
    fi

done
