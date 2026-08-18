#!/bin/bash
# Claude Code PostToolUse hook: clear active tool execution.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify \
    --type tool-stop \
    --fallback-remove /tmp/amf-tool >/dev/null 2>&1
exit 0
