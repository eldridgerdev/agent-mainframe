#!/bin/bash
# Claude Code PreToolUse hook script: mark active tool execution.
# Sends IPC event to AMF, falling back to /tmp sentinel.

INPUT=$(cat)

SESSION_ID="${AMF_SESSION:-$(echo "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)}"
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
        | if $task_status != "" then .task_status = $task_status else . end'
)"

if command -v amf >/dev/null 2>&1; then
    echo "$MSG" | amf notify 2>/dev/null && exit 0
fi

mkdir -p /tmp/amf-tool
touch "/tmp/amf-tool/$SESSION_ID"
