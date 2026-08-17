#!/bin/bash
# Claude Code UserPromptSubmit hook: report the submitted prompt.
#
# `--require prompt` reproduces the old "exit early when there is no prompt"
# guard, and the fallback writes `.claude/latest-prompt.txt` when the dashboard
# cannot be reached.
#
# Never fails the agent's turn — see notify.sh.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify \
    --type prompt-submit \
    --require prompt \
    --fallback-write-field prompt latest-prompt.txt >/dev/null 2>&1
exit 0
