#!/bin/bash
# Claude Code PreToolUse hook script: mark active tool execution.
# Sends IPC event to AMF, falling back to /tmp sentinel.

INPUT=$(cat)

SESSION_ID="${AMF_SESSION:-$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)}"
PROVIDER_SESSION_ID=$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty' 2>/dev/null)
TASK_ID=$(echo "$INPUT" | jq -r '.tool_input.taskId // empty' 2>/dev/null)
TASK_SUBJECT=$(echo "$INPUT" | jq -r '.tool_input.subject // empty' 2>/dev/null)
TASK_DESCRIPTION=$(echo "$INPUT" | jq -r '.tool_input.description // empty' 2>/dev/null)
TASK_ACTIVE_FORM=$(echo "$INPUT" | jq -r '.tool_input.activeForm // empty' 2>/dev/null)
TASK_STATUS=$(echo "$INPUT" | jq -r '.tool_input.status // empty' 2>/dev/null)

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

MSG="$(
    jq -nc \
        --arg session_id "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg provider_session_id "$PROVIDER_SESSION_ID" \
        --arg amf_feature_session_id "${AMF_FEATURE_SESSION_ID:-}" \
        --arg amf_tmux_session "${AMF_TMUX_SESSION:-${AMF_SESSION:-}}" \
        --arg amf_tmux_window "${AMF_TMUX_WINDOW:-}" \
        --arg tool_name "$TOOL_NAME" \
        --arg task_id "$TASK_ID" \
        --arg task_subject "$TASK_SUBJECT" \
        --arg task_description "$TASK_DESCRIPTION" \
        --arg task_active_form "$TASK_ACTIVE_FORM" \
        --arg task_status "$TASK_STATUS" \
        '{
            type: "tool-start",
            session_id: $session_id,
            cwd: $cwd,
            tool_name: $tool_name
        }
        | if $task_id != "" then .task_id = $task_id else . end
        | if $task_subject != "" then .task_subject = $task_subject else . end
        | if $task_description != "" then .task_description = $task_description else . end
        | if $task_active_form != "" then .task_active_form = $task_active_form else . end
        | if $task_status != "" then .task_status = $task_status else . end
        | if $provider_session_id != "" then .provider_session_id = $provider_session_id else . end
        | if $amf_feature_session_id != "" then .amf_feature_session_id = $amf_feature_session_id else . end
        | if $amf_tmux_session != "" then .amf_tmux_session = $amf_tmux_session else . end
        | if $amf_tmux_window != "" then .amf_tmux_window = $amf_tmux_window else . end'
)"

AMF_CMD="${AMF_BIN:-amf}"

if command -v "$AMF_CMD" >/dev/null 2>&1; then
    echo "$MSG" | "$AMF_CMD" notify 2>/dev/null && exit 0
fi

mkdir -p /tmp/amf-tool
touch "/tmp/amf-tool/$SESSION_ID"
