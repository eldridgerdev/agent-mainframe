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
PAYLOAD="$(
    echo "$INPUT" | jq -c \
        --arg provider_session_id "$SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        '. 
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end' \
        2>/dev/null
)"

# Socket-based push notification. The dashboard no longer polls fallback files.
if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "${PAYLOAD:-$INPUT}" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi
