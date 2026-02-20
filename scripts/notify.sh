#!/bin/bash
# Claude Code Notification hook script
# Reads JSON from stdin and writes a notification file
# for the Agent Mainframe dashboard.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
NOTIFY_DIR="$SCRIPT_DIR/notifications"
mkdir -p "$NOTIFY_DIR"

# Read JSON from stdin
INPUT=$(cat)

# Extract session_id from the JSON
SESSION_ID=$(echo "$INPUT" | sed -n 's/.*"session_id"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

# Write the notification file
echo "$INPUT" > "$NOTIFY_DIR/${SESSION_ID}.json"
