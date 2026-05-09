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

AMF_CMD="${AMF_BIN:-amf}"

# Socket-based push notification. The dashboard no longer polls fallback files.
if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$INPUT" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi
