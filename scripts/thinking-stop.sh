#!/bin/bash
# Claude Code hook: clear the session's thinking state.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify \
    --type thinking-stop \
    --fallback-remove /tmp/amf-thinking >/dev/null 2>&1
exit 0
