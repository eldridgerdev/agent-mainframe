#!/bin/bash
#
# explain.sh — Show explanation for a diff
#
# Usage: explain.sh <diff_file> <display_path> [cwd]
#
# If [cwd]/.claude/review-notes.md exists and has an entry
# for <display_path>, shows that. Otherwise calls claude -p.
#

set -uo pipefail

DIFF_FILE="${1:?}"
DISPLAY_PATH="${2:?}"
CWD="${3:-}"

# ── Try saved review notes first ────────────────────────────────

find_note() {
    local notes="$CWD/.claude/review-notes.md"
    [[ -z "$CWD" || ! -f "$notes" ]] && return 1

    local prefix="## $DISPLAY_PATH"
    awk -v prefix="$prefix" '
        /^## / {
            if (in_block && block != "") found = block
            in_block = 0; block = ""
        }
        substr($0, 1, length(prefix)) == prefix {
            in_block = 1; next
        }
        in_block && /^---$/ {
            found = block; in_block = 0; block = ""; next
        }
        in_block { block = block $0 "\n" }
        END {
            if (in_block && block != "") found = block
            if (found != "") printf "%s", found
        }
    ' "$notes"
}

note=$(find_note 2>/dev/null || true)

if [[ -n "$note" ]]; then
    echo ""
    echo "  Developer notes:"
    echo ""
    echo "$note" | sed 's/^/  /'
    echo ""
    read -rp "  Press Enter to return to review..."
    exit 0
fi

# ── Fallback: AI-generated explanation ──────────────────────────

spin() {
    local frames=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    local i=0
    tput civis
    while true; do
        printf "\r  %s Generating explanation for %s..." \
            "${frames[$i]}" "$DISPLAY_PATH"
        i=$(( (i + 1) % ${#frames[@]} ))
        sleep 0.08
    done
}

spin &
SPIN_PID=$!
trap 'kill $SPIN_PID 2>/dev/null; tput cnorm' EXIT

CLAUDE_CMD="${CLAUDE_CMD:-claude}"
if ! command -v "$CLAUDE_CMD" &>/dev/null; then
    for c in "$HOME/.local/bin/claude" \
              "$HOME/.claude/local/claude" \
              /usr/local/bin/claude; do
        [[ -x "$c" ]] && CLAUDE_CMD="$c" && break
    done
fi

explanation=$("$CLAUDE_CMD" -p \
    "Explain these code changes concisely. What is \
being changed and why?" \
    < "$DIFF_FILE" 2>/dev/null) || true

kill $SPIN_PID 2>/dev/null
wait $SPIN_PID 2>/dev/null
tput cnorm
printf "\r\033[K"

echo ""
if [[ -z "$explanation" ]]; then
    echo "  (No notes found and claude CLI unavailable)"
else
    echo "$explanation"
fi
echo ""
read -rp "  Press Enter to return to review..."
