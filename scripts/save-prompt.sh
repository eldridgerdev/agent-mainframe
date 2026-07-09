#!/bin/bash
# Claude Code UserPromptSubmit hook script.
# Sends prompt metadata over IPC to AMF, falling back to writing
# .claude/latest-prompt.txt when AMF is unavailable.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

INPUT=$(cat)

PROMPT=$(echo "$INPUT" | jq -r '.prompt // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
AMF_SESSION_ID="${AMF_SESSION:-$SESSION_ID}"

if [ -z "$PROMPT" ] || [ -z "$CWD" ]; then
    exit 0
fi

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    PAYLOAD=$(jq -nc \
        --arg sid "$AMF_SESSION_ID" \
        --arg cwd "$CWD" \
        --arg prompt "$PROMPT" \
        --arg provider_session_id "$SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        '{type:"prompt-submit",session_id:$sid,cwd:$cwd,prompt:$prompt}
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end')
    echo "$PAYLOAD" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi

mkdir -p "$CWD/.claude"
printf '%s' "$PROMPT" > "$CWD/.claude/latest-prompt.txt"
