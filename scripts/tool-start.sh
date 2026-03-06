#!/bin/bash
# Claude Code PreToolUse hook script: mark active tool execution.
# Sends IPC event to AMF, falling back to /tmp sentinel.

INPUT=$(cat)

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

MSG="{\"type\":\"tool-start\",\"session_id\":\"$SESSION_ID\",\"cwd\":\"$CWD\",\"tool_name\":\"$TOOL_NAME\"}"

if command -v amf >/dev/null 2>&1; then
    echo "$MSG" | amf notify 2>/dev/null && exit 0
fi

mkdir -p /tmp/amf-tool
touch "/tmp/amf-tool/$SESSION_ID"
