#!/usr/bin/env bash
set -euo pipefail

# Codex notify hook:
# - clears Codex "thinking" state for this AMF session
# - persists the last submitted prompt for the latest-prompt dialog
# - emits an input-request event so AMF can notify the user
#
# Codex passes a JSON payload as argv[1]. We also support stdin
# to be robust across CLI versions.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

INPUT="${1:-}"
if [ -z "$INPUT" ] && ! [ -t 0 ]; then
    INPUT="$(cat || true)"
fi

SESSION_ID="${AMF_SESSION:-}"
PROVIDER_SESSION_ID=""
CWD=""
PROMPT=""

if command -v jq >/dev/null 2>&1; then
    if [ -n "$INPUT" ]; then
        SESSION_ID_FROM_INPUT="$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || true)"
        PROVIDER_SESSION_ID="$SESSION_ID_FROM_INPUT"
        CWD="$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null || true)"
        PROMPT="$(echo "$INPUT" | jq -r '
            def msg_text:
                if type == "string" then .
                elif type == "object" then
                    .text // (
                        .content |
                        if type == "string" then .
                        elif type == "array" then
                            map(
                                if type == "string" then .
                                elif type == "object" then (.text // empty)
                                else empty
                                end
                            ) | join("")
                        else empty
                        end
                    )
                else empty
                end;
            .prompt // .message // (
                [."input-messages"[]?, .input_messages[]?]
                | map(select((.role // "") == "user") | msg_text)
                | map(select(length > 0))
                | last
            ) // empty
        ' 2>/dev/null || true)"
        if [ -z "$SESSION_ID" ] && [ -n "$SESSION_ID_FROM_INPUT" ]; then
            SESSION_ID="$SESSION_ID_FROM_INPUT"
        fi
    fi
fi

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

if [ -z "$CWD" ]; then
    CWD="$PWD"
fi

# If an existing Codex notify command was present before AMF injection,
# replay it first so user behavior is preserved.
ORIGINAL_NOTIFY_FILE="$(dirname "$0")/amf-codex-notify-original.json"
if [ -f "$ORIGINAL_NOTIFY_FILE" ] && command -v jq >/dev/null 2>&1; then
    mapfile -t ORIGINAL_NOTIFY_CMD < <(jq -r '.[]' "$ORIGINAL_NOTIFY_FILE" 2>/dev/null || true)
    if [ "${#ORIGINAL_NOTIFY_CMD[@]}" -gt 0 ]; then
        "${ORIGINAL_NOTIFY_CMD[@]}" "$INPUT" >/dev/null 2>&1 || true
    fi
fi

STOP_MSG="$(jq -nc \
    --arg sid "$SESSION_ID" \
    --arg cwd "$CWD" \
    --arg provider_session_id "$PROVIDER_SESSION_ID" \
    --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
    --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
    --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
    '{
        type:"thinking-stop",
        source:"codex-notify",
        session_id:$sid,
        cwd:$cwd
    }
    | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
    | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
    | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
    | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end')"
PROMPT_MSG=""
if [ -n "$PROMPT" ]; then
    PROMPT_MSG="$(jq -nc \
        --arg sid "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg prompt "$PROMPT" \
        --arg provider_session_id "$PROVIDER_SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        '{type:"prompt-submit",source:"codex-notify",session_id:$sid,cwd:$cwd,prompt:$prompt}
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end')"
fi
INPUT_MSG="$(jq -nc \
    --arg sid "$SESSION_ID" \
    --arg cwd "$CWD" \
    --arg provider_session_id "$PROVIDER_SESSION_ID" \
    --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
    --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
    --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
    '{
        type:"input-request",
        source:"codex-notify",
        notification_type:"input-request",
        session_id:$sid,
        cwd:$cwd,
        message:"Codex finished and is waiting for input"
    }
    | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
    | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
    | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
    | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end')"

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$STOP_MSG" | "$AMF_CMD" notify 2>/dev/null || true
    if [ -n "$PROMPT_MSG" ]; then
        echo "$PROMPT_MSG" | "$AMF_CMD" notify 2>/dev/null || true
    fi
    echo "$INPUT_MSG" | "$AMF_CMD" notify 2>/dev/null || true
fi

if [ -n "$PROMPT" ]; then
    mkdir -p "$CWD/.claude"
    printf '%s' "$PROMPT" > "$CWD/.claude/latest-prompt.txt"
fi

mkdir -p /tmp/amf-thinking
rm -f "/tmp/amf-thinking/$SESSION_ID"
