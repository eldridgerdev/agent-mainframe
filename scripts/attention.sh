#!/bin/bash
# AMF attention hook.
#
# Reports *why* an agent session stopped, so the dashboard can distinguish a
# session asking a question from one that finished work awaiting review.
#
# argv[1] is the event kind: "question", "completed", or "waiting". Anything
# else (or nothing) is treated as "waiting" by AMF, which is the correct
# degradation for a harness whose signal we can't interpret.
#
# This is deliberately separate from notify.sh: it only feeds the in-memory
# attention layer and never queues a pending input, so wiring a new harness
# event here cannot disturb the existing notification flow.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

EVENT_KIND="${1:-waiting}"

INPUT=$(cat)

SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ] || [ -z "$CWD" ]; then
    exit 0
fi

AMF_CMD="${AMF_BIN:-amf}"
if ! command -v "$AMF_CMD" >/dev/null 2>&1; then
    exit 0
fi

PAYLOAD="$(jq -nc \
    --arg sid "$SESSION_ID" \
    --arg cwd "$CWD" \
    --arg kind "$EVENT_KIND" \
    --arg provider_session_id "$SESSION_ID" \
    --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
    --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
    --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
    '{
        type:"attention",
        source:"attention-hook",
        session_id:$sid,
        cwd:$cwd,
        amf_event_kind:$kind
    }
    | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
    | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
    | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
    | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end' \
    2>/dev/null)"

if [ -n "$PAYLOAD" ]; then
    echo "$PAYLOAD" | "$AMF_CMD" notify 2>/dev/null || true
fi
