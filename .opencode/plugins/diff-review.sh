#!/bin/bash
#
# diff-review.sh — OpenCode plugin hook for reviewing file changes
#
# Opens vimdiff in a tmux popup. Press Enter to approve, r to reject
# with feedback, e to explain, q to cancel.
#
# Usage: diff-review.sh <json_file>
#   json_file contains: { "tool", "file_path", "old_string",
#                          "new_string", "content", "cwd" }
#
# Exit codes: 0 = approve, 2 = reject (stderr has feedback)
#
# Configuration (env vars):
#   DIFF_REVIEW_DELAY  — ms before keybindings activate (default: 1500)
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Parse input ───────────────────────────────────────────────

JSON_FILE="${1:?Usage: diff-review.sh <json_file>}"
HOOK_INPUT=$(cat "$JSON_FILE")

TOOL_NAME=$(echo "$HOOK_INPUT" | jq -r '.tool // empty')
FILE_PATH=$(echo "$HOOK_INPUT" | jq -r '.file_path // empty')
CWD=$(echo "$HOOK_INPUT" | jq -r '.cwd // empty')

if [[ -z "$FILE_PATH" ]]; then
    exit 0
fi

if [[ "$TOOL_NAME" != "write" && "$TOOL_NAME" != "edit" ]]; then
    exit 0
fi

# ── Display path relative to project root ─────────────────────

if [[ -n "$CWD" && "$FILE_PATH" == "$CWD"/* ]]; then
    DISPLAY_PATH="${FILE_PATH#"$CWD"/}"
else
    DISPLAY_PATH="$FILE_PATH"
fi

# ── Temp files ────────────────────────────────────────────────

FILE_EXT="${FILE_PATH##*.}"
INVOCATION_ID="$$"
SESSION_ID="${OPENCODE_SESSION_ID:-opencode}"
TEMP_DIR="/tmp/opencode-review/$SESSION_ID/$INVOCATION_ID"
SIGNAL_FILE="$TEMP_DIR/signal"
ORIGINAL_FILE="$TEMP_DIR/original.$FILE_EXT"
PROPOSED_FILE="$TEMP_DIR/proposed.$FILE_EXT"
LOCK_DIR="/tmp/opencode-review/popup.lock"

# ── Configuration ─────────────────────────────────────────────

ACTIVATION_DELAY="${DIFF_REVIEW_DELAY:-1500}"

# ── Dependency checks ─────────────────────────────────────────

check_requirements() {
    if [[ -z "${TMUX:-}" ]]; then
        echo "diff-review: not running inside tmux, skipping review." >&2
        return 1
    fi
    if ! command -v tmux &> /dev/null; then
        echo "diff-review: tmux is required but not found." >&2
        return 1
    fi
    if ! command -v nvim &> /dev/null; then
        echo "diff-review: neovim is required but not found." >&2
        return 1
    fi
    if ! command -v jq &> /dev/null; then
        echo "diff-review: jq is required but not found." >&2
        return 1
    fi
    return 0
}

# ── File capture ──────────────────────────────────────────────

capture_original_file() {
    mkdir -p "$TEMP_DIR"
    rm -f "$SIGNAL_FILE"
    if [[ -f "$FILE_PATH" ]]; then
        cp "$FILE_PATH" "$ORIGINAL_FILE"
    else
        touch "$ORIGINAL_FILE"
    fi
}

create_proposed_file() {
    if [[ "$TOOL_NAME" == "write" ]]; then
        echo "$HOOK_INPUT" | jq -r '.content // empty' > "$PROPOSED_FILE"
    elif [[ "$TOOL_NAME" == "edit" ]]; then
        local OLD_STRING_FILE="$TEMP_DIR/old_string"
        local NEW_STRING_FILE="$TEMP_DIR/new_string"
        echo "$HOOK_INPUT" | jq -r '.old_string // empty' > "$OLD_STRING_FILE"
        echo "$HOOK_INPUT" | jq -r '.new_string // empty' > "$NEW_STRING_FILE"
        OLD_FILE="$OLD_STRING_FILE" NEW_FILE="$NEW_STRING_FILE" perl -0777 -pe '
            BEGIN {
                open(F, "<", $ENV{OLD_FILE}) or die; local $/; $old = <F>; close F;
                open(F, "<", $ENV{NEW_FILE}) or die; local $/; $new = <F>; close F;
                chomp $old; chomp $new;
            }
            s/\Q$old\E/$new/s;
        ' "$ORIGINAL_FILE" > "$PROPOSED_FILE"
    fi
}

# ── Vimdiff popup ─────────────────────────────────────────────

open_review() {
    local vim_script="$TEMP_DIR/review.vim"
    local is_new_file=false
    [[ ! -f "$FILE_PATH" ]] && is_new_file=true

    cat > "$vim_script" << VIMSCRIPT
" OpenCode Diff Review — Enter=approve, r=reject, e=explain, q=cancel
let g:signal_file = '${SIGNAL_FILE}'
let g:claude_file_path = '${FILE_PATH}'
let g:claude_cwd = '${CWD}'

function! ClaudeApprove()
    call writefile(['approve'], g:signal_file)
    sleep 100m
    qa!
endfunction

function! ClaudeRejectWithFeedback()
    call writefile(['feedback'], g:signal_file)
    sleep 100m
    qa!
endfunction

function! ClaudeCancel()
    call writefile(['cancel'], g:signal_file)
    sleep 100m
    qa!
endfunction

function! ClaudeExplain()
    call writefile(['explain'], g:signal_file)
    sleep 100m
    qa!
endfunction

" Activation delay — prevent accidental keypresses when popup steals focus
let g:claude_keys_active = 0
let g:claude_activation_delay = ${ACTIVATION_DELAY}

function! ClaudeActivateKeys(timer)
    let g:claude_keys_active = 1
    nnoremap <buffer> <CR> :call ClaudeApprove()<CR>
    nnoremap <buffer> r :call ClaudeRejectWithFeedback()<CR>
    nnoremap <buffer> e :call ClaudeExplain()<CR>
    nnoremap <buffer> q :call ClaudeCancel()<CR>
    redrawtabline
    redraw
endfunction

function! ClaudeGuardedApprove()
    if !g:claude_keys_active
        echo "Keys locked — review the diff first..."
    else
        call ClaudeApprove()
    endif
endfunction

" Bind keys immediately but guarded — only approve works after delay
nnoremap <buffer> <CR> :call ClaudeGuardedApprove()<CR>
nnoremap <buffer> r <Nop>
nnoremap <buffer> e <Nop>
nnoremap <buffer> q <Nop>

autocmd VimEnter * call timer_start(g:claude_activation_delay, 'ClaudeActivateKeys')

" Custom highlight groups
highlight ClaudeHeader guifg=#ffffff guibg=#7c3aed gui=bold ctermbg=93 ctermfg=white cterm=bold
highlight ClaudeFile guifg=#e0e0e0 guibg=#7c3aed gui=NONE ctermbg=93 ctermfg=255 cterm=NONE
highlight ClaudeApprove guifg=#22c55e guibg=#7c3aed gui=bold ctermbg=93 ctermfg=green cterm=bold
highlight ClaudeFeedback guifg=#fbbf24 guibg=#7c3aed gui=bold ctermbg=93 ctermfg=yellow cterm=bold
highlight ClaudeCancel guifg=#f87171 guibg=#7c3aed gui=bold ctermbg=93 ctermfg=red cterm=bold
highlight ClaudeKey guifg=#fef08a guibg=#7c3aed gui=bold ctermbg=93 ctermfg=229 cterm=bold
highlight ClaudeOriginal guifg=#f38ba8 guibg=#313244 gui=bold ctermbg=237 ctermfg=211 cterm=bold
highlight ClaudeProposed guifg=#a6e3a1 guibg=#313244 gui=bold ctermbg=237 ctermfg=151 cterm=bold
highlight ClaudeExplain guifg=#89b4fa guibg=#7c3aed gui=bold ctermbg=93 ctermfg=117 cterm=bold
highlight ClaudeNewFile guifg=#89b4fa guibg=#313244 gui=bold ctermbg=237 ctermfg=117 cterm=bold

" Tabline: file path + actions
set showtabline=2
function! ClaudeTabline()
    let tl = '%#ClaudeHeader# DIFF REVIEW %#ClaudeFile#| ${DISPLAY_PATH} %='
    if g:claude_keys_active
        let tl .= '%#ClaudeKey#Enter %#ClaudeApprove#Approve  %#ClaudeKey#r %#ClaudeFeedback#Redo  %#ClaudeKey#e %#ClaudeExplain#?Explain  %#ClaudeKey#q %#ClaudeCancel#Cancel '
    else
        let tl .= '%#ClaudeCancel# Keys locked — reviewing... '
    endif
    return tl
endfunction
set tabline=%!ClaudeTabline()

VIMSCRIPT

    if [[ "$is_new_file" == "true" ]]; then
        cat >> "$vim_script" << 'VIMSCRIPT'
" New file — readonly single view
autocmd VimEnter * call s:SetupNewFile()
function! s:SetupNewFile()
    execute 'lcd ' . fnameescape(g:claude_cwd)
    silent! execute 'file ' . fnameescape(g:claude_file_path)
    setlocal nomodified
    filetype detect
    lua pcall(vim.treesitter.start)
    setlocal nomodifiable
    setlocal wrap
    setlocal linebreak
    setlocal number
    setlocal cursorline
    let &l:winbar = '%#ClaudeNewFile#  NEW FILE '
endfunction
VIMSCRIPT

        tmux display-popup -E -w 90% -h 90% \
            nvim -nR "$PROPOSED_FILE" -S "$vim_script" 2>/dev/null || return 1
    else
        cat >> "$vim_script" << 'VIMSCRIPT'
" Diff mode — setup windows after diff opens
autocmd VimEnter * call s:SetupWindows()
function! s:SetupWindows()
    execute 'lcd ' . fnameescape(g:claude_cwd)
    " Left window = original
    1wincmd w
    silent! execute 'file ' . fnameescape(g:claude_file_path . '.orig')
    setlocal nomodified
    filetype detect
    lua pcall(vim.treesitter.start)
    let &l:winbar = '%#ClaudeOriginal#  ORIGINAL '
    setlocal nomodifiable
    " Right window = proposed
    2wincmd w
    silent! execute 'file ' . fnameescape(g:claude_file_path)
    setlocal nomodified
    filetype detect
    lua pcall(vim.treesitter.start)
    let &l:winbar = '%#ClaudeProposed#  PROPOSED '
    setlocal nomodifiable
endfunction
VIMSCRIPT

        tmux display-popup -E -w 90% -h 90% \
            nvim -nd "$ORIGINAL_FILE" "$PROPOSED_FILE" -S "$vim_script" 2>/dev/null || return 1
    fi
}

# ── Decision handling ─────────────────────────────────────────

wait_for_decision() {
    sleep 0.2

    local timeout=120
    local elapsed=0
    while [[ ! -f "$SIGNAL_FILE" ]]; do
        sleep 1
        elapsed=$((elapsed + 1))
        [[ $elapsed -ge $timeout ]] && echo "timeout" && return
    done
    cat "$SIGNAL_FILE"
}

prompt_for_feedback() {
    local feedback_file="$TEMP_DIR/feedback.txt"
    rm -f "$feedback_file"

    tmux display-popup -E -w 70% -h 20% \
        bash "$SCRIPT_DIR/feedback-prompt.sh" "$feedback_file" 2>/dev/null || return 1

    if [[ -f "$feedback_file" ]]; then
        cat "$feedback_file"
    fi
}

show_explanation() {
    local diff_file="$TEMP_DIR/changes.diff"
    diff -u --label "original: $DISPLAY_PATH" --label "proposed: $DISPLAY_PATH" \
        "$ORIGINAL_FILE" "$PROPOSED_FILE" > "$diff_file" || true

    tmux display-popup -E -w 80% -h 80% \
        bash "$SCRIPT_DIR/explain.sh" "$diff_file" "$DISPLAY_PATH"
}

# ── Locking ───────────────────────────────────────────────────

acquire_lock() {
    [[ -f "$LOCK_DIR" ]] && rm -f "$LOCK_DIR"
    local max_age=120
    while ! mkdir "$LOCK_DIR" 2>/dev/null; do
        if [[ -d "$LOCK_DIR" ]]; then
            local lock_age
            lock_age=$(( $(date +%s) - $(stat -c %Y "$LOCK_DIR") ))
            if [[ $lock_age -gt $max_age ]]; then
                rm -rf "$LOCK_DIR" 2>/dev/null
            fi
        fi
        sleep 0.2
    done
    echo $$ > "$LOCK_DIR/pid" 2>/dev/null
}

# ── Cleanup ───────────────────────────────────────────────────

cleanup() {
    rm -rf "$LOCK_DIR" 2>/dev/null
    rm -rf "$TEMP_DIR"
}

# ── Main ──────────────────────────────────────────────────────

main() {
    check_requirements || exit 0
    capture_original_file
    create_proposed_file

    acquire_lock
    trap cleanup EXIT

    while true; do
        rm -f "$SIGNAL_FILE"
        open_review || exit 0

        local decision
        decision=$(wait_for_decision)

        case "$decision" in
            explain)
                show_explanation
                continue
                ;;
            approve|timeout)
                exit 0
                ;;
            feedback)
                local feedback
                feedback=$(prompt_for_feedback)
                if [[ -n "$feedback" ]]; then
                    echo "User rejected this change with feedback: $feedback" >&2
                else
                    echo "User rejected this change. Please try a different approach." >&2
                fi
                exit 2
                ;;
            cancel|*)
                echo "User cancelled this change." >&2
                exit 2
                ;;
        esac
    done
}

main
