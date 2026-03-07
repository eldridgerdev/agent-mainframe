#!/bin/bash
# Claude Code Stop hook script
# Sends a notification to the AMF dashboard via IPC socket,
# falling back to writing a file if amf is not in PATH.

INPUT=$(cat)

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ] || [ -z "$CWD" ]; then
    exit 0
fi

# Prefer socket-based push notification (no polling required).
if command -v amf >/dev/null 2>&1; then
    echo "$INPUT" | amf notify 2>/dev/null && exit 0
fi

# Fallback: write notification file for AMF to poll.
NOTIFY_DIR="$CWD/.claude/notifications"
mkdir -p "$NOTIFY_DIR"
echo "$INPUT" > "$NOTIFY_DIR/${SESSION_ID}.json"
