#!/usr/bin/env bash
set -euo pipefail

# Pulse AMF thinking sentinel for a tmux session so dashboard
# thinking detection can be tested without waiting for real agent activity.
#
# Usage:
#   scripts/test-thinking.sh <tmux_session> [seconds]
# Example:
#   scripts/test-thinking.sh amf-my-feature 8

if [[ $# -lt 1 || $# -gt 2 ]]; then
    echo "Usage: $0 <tmux_session> [seconds]" >&2
    exit 1
fi

SESSION="$1"
DURATION="${2:-8}"
SENTINEL_DIR="/tmp/amf-thinking"
SENTINEL_PATH="$SENTINEL_DIR/$SESSION"

if ! [[ "$DURATION" =~ ^[0-9]+$ ]]; then
    echo "seconds must be an integer" >&2
    exit 1
fi

mkdir -p "$SENTINEL_DIR"

end_ts=$(( $(date +%s) + DURATION ))
echo "Pulsing thinking sentinel for session '$SESSION' for ${DURATION}s"
while [[ $(date +%s) -lt $end_ts ]]; do
    touch "$SENTINEL_PATH"
    sleep 0.5
done

rm -f "$SENTINEL_PATH"
echo "Done. Sentinel cleared."
