#!/bin/bash
# Claude Code hook: mark the session as thinking.
#
# The harness's hook JSON is passed straight through on stdin; `amf notify`
# merges in AMF's session metadata from the environment it inherits from this
# script. No JSON is parsed here, so no `jq` is required.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify \
    --type thinking-start \
    --fallback-touch /tmp/amf-thinking >/dev/null 2>&1
exit 0
