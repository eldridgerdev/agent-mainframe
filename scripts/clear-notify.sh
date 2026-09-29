#!/bin/bash
# Claude Code PreToolUse hook: clear any pending notification for this session,
# signalling that the agent is working again.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

# $AMF_BIN names the amf that started this session. A rebuilt or deleted
# build leaves it pointing at nothing, so fall back to `amf` on PATH.
[ -x "${AMF_BIN:-}" ] || AMF_BIN=amf
"$AMF_BIN" notify --type clear >/dev/null 2>&1
exit 0
