#!/bin/bash
# Claude Code PostToolUse hook script: clear active tool execution.
# Sends IPC event to AMF, falling back to /tmp sentinel removal.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

INPUT=$(cat)

SESSION_ID="${AMF_SESSION:-$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)}"
PROVIDER_SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

MSG="$(
    jq -nc \
        --arg session_id "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg tool_name "$TOOL_NAME" \
        --arg provider_session_id "$PROVIDER_SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        '{
            type: "tool-stop",
            session_id: $session_id,
            cwd: $cwd,
            tool_name: $tool_name
        }
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end'
)"

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$MSG" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi

rm -f "/tmp/amf-tool/$SESSION_ID"
