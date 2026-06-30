#!/bin/bash
# Claude Code PreToolUse hook script
# Clears any pending notification for this session from the
# AMF dashboard, signalling that the agent is working again.

INPUT=$(cat)

PROVIDER_SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
SESSION_ID="${AMF_SESSION:-$PROVIDER_SESSION_ID}"
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ] || [ -z "$CWD" ]; then
    exit 0
fi

CLEAR_MSG="$(
    jq -nc \
        --arg session_id "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg provider_session_id "$PROVIDER_SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        '{
            type: "clear",
            session_id: $session_id,
            cwd: $cwd
        }
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end'
)"

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$CLEAR_MSG" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi
