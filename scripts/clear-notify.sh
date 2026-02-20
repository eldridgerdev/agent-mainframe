#!/bin/bash
# Claude Code Stop hook script
# Reads JSON from stdin and removes the notification file
# for the Agent Mainframe dashboard.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NOTIFY_DIR="$SCRIPT_DIR/notifications"

# Read JSON from stdin
INPUT=$(cat)

# Extract session_id from the JSON
SESSION_ID=$(echo "$INPUT" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

# Remove the notification file if it exists
rm -f "$NOTIFY_DIR/${SESSION_ID}.json"
