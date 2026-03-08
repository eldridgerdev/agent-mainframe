#!/usr/bin/env bash
set -euo pipefail

# Codex notify hook:
# - clears Codex "thinking" state for this AMF session
# - persists the last submitted prompt for the latest-prompt dialog
# - emits an input-request event so AMF can notify the user
#
# Codex passes a JSON payload as argv[1]. We also support stdin
# to be robust across CLI versions.

INPUT="${1:-}"
if [ -z "$INPUT" ] && ! [ -t 0 ]; then
    INPUT="$(cat || true)"
fi

SESSION_ID="${AMF_SESSION:-}"
CWD=""
PROMPT=""

if command -v jq >/dev/null 2>&1; then
    if [ -n "$INPUT" ]; then
        SESSION_ID_FROM_INPUT="$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null || true)"
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

STOP_MSG="{\"type\":\"thinking-stop\",\"source\":\"codex-notify\",\"session_id\":\"$SESSION_ID\",\"cwd\":\"$CWD\"}"
PROMPT_MSG=""
if [ -n "$PROMPT" ]; then
    PROMPT_MSG="$(jq -nc \
        --arg sid "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg prompt "$PROMPT" \
        '{type:"prompt-submit",source:"codex-notify",session_id:$sid,cwd:$cwd,prompt:$prompt}')"
fi
INPUT_MSG="{\"type\":\"input-request\",\"source\":\"codex-notify\",\"notification_type\":\"input-request\",\"session_id\":\"$SESSION_ID\",\"cwd\":\"$CWD\",\"message\":\"Codex finished and is waiting for input\"}"

if command -v amf >/dev/null 2>&1; then
    echo "$STOP_MSG" | amf notify 2>/dev/null || true
    if [ -n "$PROMPT_MSG" ]; then
        echo "$PROMPT_MSG" | amf notify 2>/dev/null || true
    fi
    echo "$INPUT_MSG" | amf notify 2>/dev/null || true
fi

if [ -n "$PROMPT" ]; then
    mkdir -p "$CWD/.claude"
    printf '%s' "$PROMPT" > "$CWD/.claude/latest-prompt.txt"
fi

# Fallback for when IPC delivery is unavailable.
mkdir -p /tmp/amf-thinking
rm -f "/tmp/amf-thinking/$SESSION_ID"

NOTIFY_DIR="$HOME/.config/amf/notifications"
mkdir -p "$NOTIFY_DIR"
FILE="$NOTIFY_DIR/codex-input-$(date +%s)-$$.json"
printf '%s\n' "$INPUT_MSG" > "$FILE"
