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

# $AMF_BIN names the amf that started this session. A rebuilt or deleted
# build leaves it pointing at nothing, so fall back to `amf` on PATH.
[ -x "${AMF_BIN:-}" ] || AMF_BIN=amf
"${AMF_BIN:-amf}" notify \
    --type tool-start \
    --fallback-touch /tmp/amf-tool >/dev/null 2>&1
exit 0
