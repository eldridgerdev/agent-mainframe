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

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$CLEAR_MSG" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi
