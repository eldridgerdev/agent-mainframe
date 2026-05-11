#!/bin/bash
# Claude Code hook script: mark session as thinking.
# Sends IPC event to AMF, falling back to /tmp sentinel.

INPUT=$(cat)

# AMF_SESSION is the tmux session name set by AMF when launching Claude.
# This matches the key used by the dashboard; Claude's hook session_id is a UUID.
SESSION_ID="${AMF_SESSION:-$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)}"
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

MSG="{\"type\":\"thinking-start\",\"session_id\":\"$SESSION_ID\",\"cwd\":\"$CWD\"}"

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$MSG" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi

mkdir -p /tmp/amf-thinking
touch "/tmp/amf-thinking/$SESSION_ID"
