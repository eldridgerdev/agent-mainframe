#!/bin/bash
# Claude Code PreToolUse hook: mark active tool execution.
#
# `amf notify` lifts the nested `tool_input.{taskId,subject,...}` fields into
# the flat `task_*` shape the dashboard reads.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify \
    --type tool-start \
    --fallback-touch /tmp/amf-tool >/dev/null 2>&1
exit 0
