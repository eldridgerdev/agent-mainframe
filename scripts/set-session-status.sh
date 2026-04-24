#!/bin/bash
# Write a custom session status into the AMF database.
#
# AMF sets AMF_SESSION_ID and AMF_STATUS_DIR when launching custom sessions.
# Call this script from your session's hook to report a one-line status shown
# in the AMF dashboard.
#
# Usage: set-session-status.sh "status text"

STATUS_TEXT="${1:-}"
SESSION_ID="${AMF_SESSION_ID:-}"

if [ -z "$SESSION_ID" ] || [ -z "$STATUS_TEXT" ]; then
    exit 0
fi

if command -v amf >/dev/null 2>&1; then
    amf set-status "$SESSION_ID" "$STATUS_TEXT" 2>/dev/null && exit 0
fi

# Fallback: write to legacy file path when amf is not in PATH
if [ -n "$AMF_STATUS_DIR" ]; then
    mkdir -p "$AMF_STATUS_DIR"
    printf '%s\n' "$STATUS_TEXT" > "$AMF_STATUS_DIR/${SESSION_ID}.txt"
fi
