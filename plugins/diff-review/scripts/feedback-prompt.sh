#!/bin/bash
#
# feedback-prompt.sh — Prompt the user for rejection feedback in a tmux popup
#
# Usage: feedback-prompt.sh <feedback_file>
#

set -euo pipefail

FEEDBACK_FILE="${1:?Usage: feedback-prompt.sh <feedback_file>}"

echo ""
echo "  ╭──────────────────────────────────────────────────────────────╮"
echo "  │              What should Claude do differently?              │"
echo "  ╰──────────────────────────────────────────────────────────────╯"
echo ""
read -p "  → " feedback
echo "$feedback" > "$FEEDBACK_FILE"
