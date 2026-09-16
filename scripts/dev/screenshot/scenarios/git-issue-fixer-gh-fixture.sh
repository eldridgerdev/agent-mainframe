#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" == "api" && "${2:-}" == repos/eldridgerdev/agent-mainframe/issues\?* ]]; then
    printf '%s\n' '[{"number":635,"title":"Doc/marketing site on *.pages.dev is blocked by corporate web filters","body":"Corporate web filters can block the current Pages hostname.","html_url":"https://github.com/eldridgerdev/agent-mainframe/issues/635","labels":[{"name":"documentation"}],"updated_at":"2026-09-15T12:00:00Z"},{"number":634,"title":"Stop sessions without deleting them","body":"Keep stop and delete actions distinct.","html_url":"https://github.com/eldridgerdev/agent-mainframe/issues/634","labels":[{"name":"bug"}],"updated_at":"2026-09-14T12:00:00Z"}]'
    exit 0
fi

exec "${AMF_SCREENSHOT_REAL_GH:?real gh path was not provided}" "$@"
