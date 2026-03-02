#!/bin/bash
# Claude Code UserPromptSubmit hook script
# Saves the latest user prompt to .claude/latest-prompt.txt
# for the Agent Mainframe dashboard.

INPUT=$(cat)

PROMPT=$(echo "$INPUT" | jq -r '.prompt // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$PROMPT" ] || [ -z "$CWD" ]; then
    exit 0
fi

mkdir -p "$CWD/.claude"
printf '%s' "$PROMPT" > "$CWD/.claude/latest-prompt.txt"
