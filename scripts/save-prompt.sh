#!/bin/bash
# Claude Code UserPromptSubmit hook script.
# Sends prompt metadata over IPC to AMF, falling back to writing
# .claude/latest-prompt.txt when AMF is unavailable.

INPUT=$(cat)

PROMPT=$(echo "$INPUT" | jq -r '.prompt // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)

if [ -z "$PROMPT" ] || [ -z "$CWD" ]; then
    exit 0
fi

if command -v amf >/dev/null 2>&1; then
    PAYLOAD=$(jq -nc \
        --arg sid "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg prompt "$PROMPT" \
        '{type:"prompt-submit",session_id:$sid,cwd:$cwd,prompt:$prompt}')
    echo "$PAYLOAD" | amf notify 2>/dev/null && exit 0
fi

mkdir -p "$CWD/.claude"
printf '%s' "$PROMPT" > "$CWD/.claude/latest-prompt.txt"
