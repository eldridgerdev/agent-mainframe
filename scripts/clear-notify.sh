#!/bin/bash
# Claude Code Stop hook script
# Reads JSON from stdin and removes the notification file
# for the Agent Mainframe dashboard.

INPUT=$(cat)

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ] || [ -z "$CWD" ]; then
    exit 0
fi

NOTIFY_DIR="$CWD/.claude/notifications"

rm -f "$NOTIFY_DIR/${SESSION_ID}.json"
