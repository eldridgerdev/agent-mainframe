#!/usr/bin/env bash
set -uo pipefail

# Codex notify hook:
# - clears Codex "thinking" state for this AMF session
# - persists the last submitted prompt for the latest-prompt dialog
# - emits an input-request event so AMF can notify the user
# - reports the turn as completed, for the dashboard's attention layer
#
# Codex passes a JSON payload as argv[1]. We also support stdin to be robust
# across CLI versions. The payload is forwarded untouched: `amf notify` does the
# parsing, including recovering the prompt from Codex's message list, so this
# script needs no `jq`.
#
# A user's own notify command is preserved by AMF at the Codex *config* level
# (the `notify` array keeps existing entries), so there is nothing to replay
# here.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

INPUT="${1:-}"
if [ -z "$INPUT" ] && ! [ -t 0 ]; then
    INPUT="$(cat || true)"
fi

AMF_CMD="${AMF_BIN:-amf}"

send() {
    printf '%s' "$INPUT" | "$AMF_CMD" notify --source codex-notify "$@" 2>/dev/null || true
}

send --type thinking-stop
send --type prompt-submit --derive-prompt --require prompt \
    --fallback-write-field prompt latest-prompt.txt
send --type input-request \
    --field notification_type=input-request \
    --field "message=Codex finished and is waiting for input"
# Codex's hook fires once, at turn end, and carries nothing that separates
# "I'm asking you something" from "I'm done". Reporting "completed" is the
# strongest claim the signal supports; AMF narrows anything it cannot justify.
send --type attention --event-kind completed

# Clear the thinking sentinel unconditionally, matching the pre-IPC behaviour:
# the dashboard prefers its IPC state, and a stale sentinel would otherwise sit
# there until its 2-second freshness window lapsed.
if [ -n "${AMF_SESSION:-}" ]; then
    rm -f "/tmp/amf-thinking/${AMF_SESSION}"
fi
