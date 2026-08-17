#!/bin/bash
# Claude Code PreToolUse hook: clear any pending notification for this session,
# signalling that the agent is working again.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify --type clear >/dev/null 2>&1
exit 0
