#!/bin/bash
# Claude Code PreToolUse hook script
# Clears any pending notification for this session from the
# AMF dashboard, signalling that the agent is working again.

INPUT=$(cat)

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ] || [ -z "$CWD" ]; then
    exit 0
fi

CLEAR_MSG="{\"type\":\"clear\",\"session_id\":\"$SESSION_ID\",\"cwd\":\"$CWD\"}"

# Prefer socket-based clear (no polling required).
if command -v amf >/dev/null 2>&1; then
    echo "$CLEAR_MSG" | amf notify 2>/dev/null && exit 0
fi

# Fallback: remove notification file.
NOTIFY_DIR="$CWD/.claude/notifications"
rm -f "$NOTIFY_DIR/${SESSION_ID}.json"
