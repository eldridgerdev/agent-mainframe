#!/bin/bash
# Claude Code Stop hook: tell the dashboard the turn ended.
#
# Deliberately sets no `--type`: this forwards Claude's own payload, and the
# dashboard's generic path keys off `type` being absent.
#
# Never fails the agent's turn. A hook that exits non-zero aborts the tool call
# that triggered it, so delivery problems — AMF not running, or an `amf` binary
# older than these flags — are swallowed and the hook exits 0 regardless.

if [ "${AMF_ACTIVE:-}" != "1" ]; then
    exit 0
fi

"${AMF_BIN:-amf}" notify >/dev/null 2>&1
exit 0
