#!/usr/bin/env bash
#
# codex-diff-review.sh — Codex vibeless-mode file change watcher
#
# Watches a workdir for file writes made by Codex and sends change-reason
# notifications to a running AMF instance. When the user rejects a change, the
# file is reverted.
#
# `amf notify-wait` builds the payload and reports the verdict as an exit code,
# so no JSON is parsed here. File bodies are passed as paths rather than
# arguments, keeping them clear of ARG_MAX.
#
# Usage: codex-diff-review.sh <workdir>
#   AMF_SESSION env var must be set (done by AMF on launch).

set -uo pipefail

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

WORKDIR="${1:-$PWD}"
SESSION_ID="${AMF_SESSION:-}"
AMF_CMD="${AMF_BIN:-amf}"

# inotifywait and git are genuine external requirements of this watcher; it
# exits quietly rather than half-working when either is absent.
for cmd in inotifywait git; do
    command -v "$cmd" >/dev/null 2>&1 || exit 0
done
command -v "$AMF_CMD" >/dev/null 2>&1 || exit 0
[ -n "$SESSION_ID" ] || exit 0

cd "$WORKDIR" || exit 0

REJECTED_EXIT=10

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

    # Old content comes from git HEAD, staged to a temp file so it can be
    # passed by path. An untracked file has no old content and stays empty.
    OLD_FILE="$(mktemp)" || continue
    TOOL="write"
    if git ls-files --error-unmatch "$FILE" >/dev/null 2>&1; then
        git show "HEAD:${RELATIVE}" > "$OLD_FILE" 2>/dev/null || true
        TOOL="edit"
    fi

    # Skip if the file is unchanged from HEAD.
    if cmp -s "$OLD_FILE" "$FILE"; then
        rm -f "$OLD_FILE"
        continue
    fi

    "$AMF_CMD" notify-wait --timeout-ms 120000 \
        --reject-exit-code "$REJECTED_EXIT" \
        --type change-reason \
        --field notification_type=change-reason \
        --field "file_path=$FILE" \
        --field "relative_path=$RELATIVE" \
        --field "tool=$TOOL" \
        --field-from-file "old_snippet=$OLD_FILE" \
        --field-from-file "new_snippet=$FILE" \
        < /dev/null > /dev/null 2>&1
    VERDICT=$?

    rm -f "$OLD_FILE"

    if [ "$VERDICT" -eq "$REJECTED_EXIT" ]; then
        if git ls-files --error-unmatch "$FILE" >/dev/null 2>&1; then
            git checkout -- "$FILE" 2>/dev/null || true
        else
            rm -f "$FILE"
        fi
    fi

done
