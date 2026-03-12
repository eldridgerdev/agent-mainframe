#!/bin/bash
#
# final-review.sh — Interactive final review of all changed files
#
# Usage: final-review.sh <workdir> [base-ref]
#
# Goes through every file changed since base-ref (default: master or
# main), shows vimdiff + developer notes, and prompts for approval or
# rejection with feedback.
#
# Keys (after activation delay):
#   Enter  — approve this file
#   r      — reject (prompts for feedback)
#   n      — show developer notes then re-open diff
#   e      — explain selection (visual mode) or whole diff (normal mode)
#   a      — ask a question about selection (visual mode) or whole diff (normal mode)
#   Esc    — skip (neither approve nor reject)
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKDIR="${1:?Usage: final-review.sh <workdir> [base-ref]}"
BASE_REF="${2:-}"

tmux() {
    local tmux_bin="${AMF_TMUX_BIN:-tmux}"
    if [[ -n "${AMF_TMUX_SOCKET:-}" ]]; then
        "$tmux_bin" -S "$AMF_TMUX_SOCKET" "$@"
    else
        "$tmux_bin" "$@"
    fi
}

ACTIVATION_DELAY=800
INVOCATION_ID=$$
TEMP_DIR="/tmp/claude-review/final/$INVOCATION_ID"
mkdir -p "$TEMP_DIR"

CLAUDE_CMD="${CLAUDE_CMD:-claude}"
if ! command -v "$CLAUDE_CMD" &>/dev/null; then
    for c in "$HOME/.local/bin/claude" \
              "$HOME/.claude/local/claude" \
              /usr/local/bin/claude; do
        [[ -x "$c" ]] && CLAUDE_CMD="$c" && break
    done
fi

cleanup() { rm -rf "$TEMP_DIR"; }
trap cleanup EXIT

# ── Determine base ref ──────────────────────────────────────────

determine_base() {
    cd "$WORKDIR"
    if [[ -n "$BASE_REF" ]]; then
        echo "$BASE_REF"
        return
    fi
    if git rev-parse --verify master &>/dev/null 2>&1; then
        echo "master"
    elif git rev-parse --verify main &>/dev/null 2>&1; then
        echo "main"
    else
        echo "HEAD~1"
    fi
}

# ── Look up developer notes for a file ─────────────────────────

extract_note() {
    local display_path="$1"
    local note_out="$2"
    local notes_file="$WORKDIR/.claude/review-notes.md"
    [[ ! -f "$notes_file" ]] && return 0

    awk -v prefix="## $display_path" '
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
    ' "$notes_file" > "$note_out"
}

# ── Show developer notes in a popup ────────────────────────────

show_note_popup() {
    local rel_path="$1"
    local note_file="$2"

    if [[ ! -s "$note_file" ]]; then
        tmux display-popup -E -w 55% -h 25% \
            bash -c "
                echo ''
                echo '  No developer notes found for:'
                echo \"  $rel_path\"
                echo ''
                read -rp '  Press Enter to view diff...'
            " 2>/dev/null || true
        return
    fi

    tmux display-popup -E -w 80% -h 80% \
        bash -c "
            echo ''
            echo '  Developer notes for: $rel_path'
            printf '  %s\n' '──────────────────────────────────────'
            echo ''
            cat '$note_file' | sed 's/^/  /'
            echo ''
            read -rp '  Press Enter to view diff...'
        " 2>/dev/null || true
}

# ── Spinner for AI calls ──────────────────────────────────────────

spin() {
    local msg="$1"
    local frames=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
    local i=0
    tput civis
    while true; do
        printf "\r  %s %s" "${frames[$i]}" "$msg"
        i=$(( (i + 1) % ${#frames[@]} ))
        sleep 0.08
    done
}

# ── Show AI explanation for selected lines or diff ──────────────────

show_explanation_popup() {
    local rel_path="$1"
    local content_file="$2"
    local mode="${3:-selection}"

    if [[ ! -s "$content_file" ]]; then
        tmux display-popup -E -w 55% -h 25% \
            bash -c "
                echo ''
                echo '  No content to explain.'
                echo ''
                read -rp '  Press Enter to return to diff...'
            " 2>/dev/null || true
        return
    fi

    local prompt header_label
    if [[ "$mode" == "diff" ]]; then
        prompt="Explain these code changes concisely. What was changed and why?"
        header_label="Diff for"
    else
        prompt="Explain these code lines concisely. What do they do and why?"
        header_label="Selection in"
    fi

    tmux display-popup -E -w 80% -h 70% \
        bash -c '
            CLAUDE_CMD="'"$CLAUDE_CMD"'"
            CONTENT_FILE="'"$content_file"'"
            PROMPT="'"$prompt"'"
            HEADER_LABEL="'"$header_label"'"
            REL_PATH="'"$rel_path"'"

            spin() {
                local msg="$1"
                local frames=("⠋" "⠙" "⠹" "⠸" "⠼" "⠴" "⠦" "⠧" "⠇" "⠏")
                local i=0
                tput civis
                while true; do
                    printf "\r  %s %s" "${frames[$i]}" "$msg"
                    i=$(( (i + 1) % ${#frames[@]} ))
                    sleep 0.08
                done
            }

            spin "Generating explanation..." &
            SPIN_PID=$!
            trap "kill \$SPIN_PID 2>/dev/null; tput cnorm" EXIT

            explanation=$("$CLAUDE_CMD" -p "$PROMPT" < "$CONTENT_FILE" 2>/dev/null) || true

            kill $SPIN_PID 2>/dev/null
            wait $SPIN_PID 2>/dev/null
            tput cnorm
            printf "\r\033[K"

            echo ""
            echo "  Explanation for $HEADER_LABEL: $REL_PATH"
            printf "  %s\n" "──────────────────────────────────────"
            echo ""
            if [[ -n "$explanation" ]]; then
                echo "$explanation" | sed "s/^/  /"
            else
                echo "  (claude CLI unavailable)"
            fi
            echo ""
            read -rp "  Press Enter to return to diff..."
        ' 2>/dev/null || true
}

# ── Ask a question about selected lines or diff ────────────────────────

show_ask_popup() {
    local rel_path="$1"
    local content_file="$2"
    local mode="${3:-selection}"
    local question_file="$TEMP_DIR/question.txt"

    tmux display-popup -E -w 70% -h 25% \
        bash -c "
            echo ''
            echo '  Ask a question about the $mode:'
            echo \"  File: $rel_path\"
            echo ''
            read -rp '  > ' question
            [[ -n \"\$question\" ]] && echo \"\$question\" > '$question_file'
        " 2>/dev/null || true

    if [[ ! -s "$question_file" ]]; then
        return
    fi

    local question
    question=$(cat "$question_file")

    local header_label
    if [[ "$mode" == "diff" ]]; then
        header_label="Diff for"
    else
        header_label="Selection in"
    fi

    local context=""
    if [[ -s "$content_file" ]]; then
        context=$(cat "$content_file")
    fi

    tmux display-popup -E -w 80% -h 70% \
        bash -c '
            CLAUDE_CMD="'"$CLAUDE_CMD"'"
            CONTEXT="'"$context"'"
            QUESTION="'"$question"'"
            HEADER_LABEL="'"$header_label"'"
            REL_PATH="'"$rel_path"'"

            spin() {
                local msg="$1"
                local frames=("⠋" "⠙" "⠹" "⠸" "⠼" "⠴" "⠦" "⠧" "⠇" "⠏")
                local i=0
                tput civis
                while true; do
                    printf "\r  %s %s" "${frames[$i]}" "$msg"
                    i=$(( (i + 1) % ${#frames[@]} ))
                    sleep 0.08
                done
            }

            spin "Thinking..." &
            SPIN_PID=$!
            trap "kill \$SPIN_PID 2>/dev/null; tput cnorm" EXIT

            answer=$("$CLAUDE_CMD" -p "Context:\n\`\`\`\n$CONTEXT\n\`\`\`\n\nQuestion: $QUESTION" 2>/dev/null) || true

            kill $SPIN_PID 2>/dev/null
            wait $SPIN_PID 2>/dev/null
            tput cnorm
            printf "\r\033[K"

            echo ""
            echo "  Q: $QUESTION"
            echo "  ($HEADER_LABEL: $REL_PATH)"
            printf "  %s\n" "──────────────────────────────────────"
            echo ""
            if [[ -n "$answer" ]]; then
                echo "$answer" | sed "s/^/  /"
            else
                echo "  (claude CLI unavailable)"
            fi
            echo ""
            read -rp "  Press Enter to return to diff..."
        ' 2>/dev/null || true
}

# ── Ask a question about selected lines or diff ────────────────────────

show_ask_popup() {
    local rel_path="$1"
    local content_file="$2"
    local mode="${3:-selection}"
    local question_file="$TEMP_DIR/question.txt"

    tmux display-popup -E -w 70% -h 25% \
        bash -c "
            echo ''
            echo '  Ask a question about the $mode:'
            echo \"  File: $rel_path\"
            echo ''
            read -rp '  > ' question
            [[ -n \"\$question\" ]] && echo \"\$question\" > '$question_file'
        " 2>/dev/null || true

    if [[ ! -s "$question_file" ]]; then
        return
    fi

    local question
    question=$(cat "$question_file")

    local header_label
    if [[ "$mode" == "diff" ]]; then
        header_label="Diff for"
    else
        header_label="Selection in"
    fi

    spin "Thinking..." &
    SPIN_PID=$!
    trap 'kill $SPIN_PID 2>/dev/null; tput cnorm' EXIT

    local context=""
    if [[ -s "$content_file" ]]; then
        context=$(cat "$content_file")
    fi

    local answer
    answer=$("$CLAUDE_CMD" -p "Context:\n\`\`\`\n$context\n\`\`\`\n\nQuestion: $question" 2>/dev/null) || true

    kill $SPIN_PID 2>/dev/null
    wait $SPIN_PID 2>/dev/null
    tput cnorm
    printf "\r\033[K"

    tmux display-popup -E -w 80% -h 70% \
        bash -c "
            echo ''
            echo \"  Q: $question\"
            echo \"  ($header_label: $rel_path)\"
            printf '  %s\n' '──────────────────────────────────────'
            echo ''
            if [[ -n \"$answer\" ]]; then
                echo \"$answer\" | sed 's/^/  /'
            else
                echo '  (claude CLI unavailable)'
            fi
            echo ''
            read -rp '  Press Enter to return to diff...'
        " 2>/dev/null || true
}

# ── Build and run vimdiff popup for one file ───────────────────

open_vimdiff() {
    local rel_path="$1"
    local original="$2"
    local proposed="$3"
    local signal="$4"
    local file_num="$5"
    local file_total="$6"
    local abs_path="$WORKDIR/$rel_path"
    local vim_script="$TEMP_DIR/review_${file_num}.vim"
    local selection_file="$TEMP_DIR/selection_${file_num}.txt"
    local is_new=false
    [[ ! -s "$original" ]] && is_new=true

    rm -f "$signal" "$selection_file"

    cat > "$vim_script" << VIMSCRIPT
" Final Review — ${file_num} of ${file_total}: ${rel_path}
let g:signal_file = '${signal}'
let g:selection_file = '${selection_file}'
let g:cwd = '${WORKDIR}'

function! FinalApprove()
    call writefile(['approve'], g:signal_file)
    sleep 100m | qa!
endfunction
function! FinalReject()
    call writefile(['reject'], g:signal_file)
    sleep 100m | qa!
endfunction
function! FinalNote()
    call writefile(['note'], g:signal_file)
    sleep 100m | qa!
endfunction
function! FinalSkip()
    call writefile(['skip'], g:signal_file)
    sleep 100m | qa!
endfunction
function! FinalExplain()
    let sel_start = getpos("'<")
    let sel_end = getpos("'>")
    if sel_start[1] == 0 || sel_end[1] == 0
        call writefile(['explain:'], g:signal_file)
    else
        let lines = getline(sel_start[1], sel_end[1])
        call writefile(lines, g:selection_file)
        call writefile(['explain:' . g:selection_file], g:signal_file)
    endif
    sleep 100m | qa!
endfunction
function! FinalAsk()
    let sel_start = getpos("'<")
    let sel_end = getpos("'>")
    if sel_start[1] == 0 || sel_end[1] == 0
        call writefile(['ask:'], g:signal_file)
    else
        let lines = getline(sel_start[1], sel_end[1])
        call writefile(lines, g:selection_file)
        call writefile(['ask:' . g:selection_file], g:signal_file)
    endif
    sleep 100m | qa!
endfunction

let g:keys_active = 0
function! ActivateKeys(timer)
    let g:keys_active = 1
    nnoremap <buffer> <CR> :call FinalApprove()<CR>
    nnoremap <buffer> r    :call FinalReject()<CR>
    nnoremap <buffer> n    :call FinalNote()<CR>
    nnoremap <buffer> e    :call FinalExplain()<CR>
    nnoremap <buffer> a    :call FinalAsk()<CR>
    nnoremap <buffer> <Esc> :call FinalSkip()<CR>
    vnoremap <buffer> e    :<C-U>call FinalExplain()<CR>
    vnoremap <buffer> a    :<C-U>call FinalAsk()<CR>
    redrawtabline | redraw
endfunction
function! GuardedApprove()
    if !g:keys_active | echo "Keys locked — review first..."
    else | call FinalApprove() | endif
endfunction
nnoremap <buffer> <CR> :call GuardedApprove()<CR>
nnoremap <buffer> r <Nop>
nnoremap <buffer> n <Nop>
nnoremap <buffer> e <Nop>
nnoremap <buffer> a <Nop>
nnoremap <buffer> <Esc> <Nop>
vnoremap <buffer> e <Nop>
vnoremap <buffer> a <Nop>
autocmd VimEnter * call timer_start(${ACTIVATION_DELAY}, 'ActivateKeys')

highlight FRHeader   guifg=#ffffff guibg=#2563eb gui=bold ctermbg=26  ctermfg=white  cterm=bold
highlight FRFile     guifg=#e0e0e0 guibg=#2563eb gui=NONE ctermbg=26  ctermfg=255
highlight FRApprove  guifg=#22c55e guibg=#2563eb gui=bold ctermbg=26  ctermfg=green  cterm=bold
highlight FRReject   guifg=#fbbf24 guibg=#2563eb gui=bold ctermbg=26  ctermfg=yellow cterm=bold
highlight FRNote     guifg=#89b4fa guibg=#2563eb gui=bold ctermbg=26  ctermfg=117    cterm=bold
highlight FRExplain  guifg=#a78bfa guibg=#2563eb gui=bold ctermbg=26  ctermfg=141    cterm=bold
highlight FRAsk      guifg=#f472b6 guibg=#2563eb gui=bold ctermbg=26  ctermfg=206    cterm=bold
highlight FRSkip     guifg=#94a3b8 guibg=#2563eb gui=NONE ctermbg=26  ctermfg=246
highlight FRKey      guifg=#fef08a guibg=#2563eb gui=bold ctermbg=26  ctermfg=229    cterm=bold
highlight FROriginal guifg=#f38ba8 guibg=#313244 gui=bold ctermbg=237 ctermfg=211    cterm=bold
highlight FRProposed guifg=#a6e3a1 guibg=#313244 gui=bold ctermbg=237 ctermfg=151    cterm=bold
highlight FRNew      guifg=#89b4fa guibg=#313244 gui=bold ctermbg=237 ctermfg=117    cterm=bold

set showtabline=2
function! FRTabline()
    let tl = '%#FRHeader# FINAL REVIEW [${file_num}/${file_total}] %#FRFile#│ ${rel_path} %='
    if g:keys_active
        let tl .= '%#FRKey#↵ %#FRApprove#Approve  %#FRKey#r %#FRReject#Redo  %#FRKey#n %#FRNote#Notes  %#FRKey#e %#FRExplain#Explain  %#FRKey#a %#FRAsk#Ask  %#FRKey#Esc %#FRSkip#Skip '
    else
        let tl .= '%#FRSkip# 🔒 Keys locked — review the diff... '
    endif
    return tl
endfunction
set tabline=%!FRTabline()

" TODO: Add mouse support for click-to-select and right-click context menu
" - <LeftMouse> to position cursor
" - <LeftDrag> for visual selection
" - <RightMouse> for context menu (approve/reject/explain/etc)
" - set mouse=a mousemodel=popup
VIMSCRIPT

    if [[ "$is_new" == "true" ]]; then
        cat >> "$vim_script" << 'VIMSCRIPT'
autocmd VimEnter * call s:Setup()
function! s:Setup()
    execute 'lcd ' . fnameescape(g:cwd)
    filetype detect
    lua pcall(vim.treesitter.start)
    setlocal nomodifiable nomodified wrap linebreak number cursorline
    let &l:winbar = '%#FRNew#  ★ NEW FILE '
endfunction
VIMSCRIPT
        tmux display-popup -E -w 90% -h 90% \
            nvim -nR "$proposed" -S "$vim_script" 2>/dev/null || true
    else
        cat >> "$vim_script" << 'VIMSCRIPT'
autocmd VimEnter * call s:Setup()
function! s:Setup()
    execute 'lcd ' . fnameescape(g:cwd)
    1wincmd w | filetype detect
    lua pcall(vim.treesitter.start)
    setlocal nomodifiable nomodified
    let &l:winbar = '%#FROriginal#  ← BASE '
    2wincmd w | filetype detect
    lua pcall(vim.treesitter.start)
    setlocal nomodifiable nomodified
    let &l:winbar = '%#FRProposed#  → CURRENT '
endfunction
VIMSCRIPT
        tmux display-popup -E -w 90% -h 90% \
            nvim -nd "$original" "$proposed" -S "$vim_script" 2>/dev/null || true
    fi
}

# ── Main review loop ────────────────────────────────────────────

cd "$WORKDIR"
BASE=$(determine_base)

CHANGED_FILES=$(git log --no-merges --pretty=format: --name-only "$BASE"..HEAD 2>/dev/null | grep -v '^$' | sort -u || true)
if [[ -z "$CHANGED_FILES" ]]; then
    tmux display-popup -E -w 60% -h 25% \
        bash -c "
            echo ''
            echo '  No committed changes found since $BASE.'
            echo '  (Uncommitted changes are excluded from review.)'
            echo ''
            read -rp '  Press Enter to close...'
        " 2>/dev/null || true
    exit 0
fi

FILE_COUNT=$(echo "$CHANGED_FILES" | grep -c . || true)
APPROVED=0
REJECTED=0
SKIPPED=0
declare -a FEEDBACK_PARTS

file_num=0
while IFS= read -r rel_path; do
    [[ -z "$rel_path" ]] && continue
    file_num=$((file_num + 1))

    FILE_EXT="${rel_path##*.}"
    ORIGINAL="$TEMP_DIR/orig_${file_num}.${FILE_EXT}"
    PROPOSED="$TEMP_DIR/prop_${file_num}.${FILE_EXT}"
    SIGNAL="$TEMP_DIR/sig_${file_num}"
    NOTE_FILE="$TEMP_DIR/note_${file_num}.md"

    # Prepare files — both sides from git, not the working tree,
    # so uncommitted edits (e.g. .claude/settings) are excluded.
    git show "${BASE}:${rel_path}" > "$ORIGINAL" 2>/dev/null || touch "$ORIGINAL"
    git show "HEAD:${rel_path}" > "$PROPOSED" 2>/dev/null || touch "$PROPOSED"
    extract_note "$rel_path" "$NOTE_FILE"

    while true; do
        open_vimdiff \
            "$rel_path" "$ORIGINAL" "$PROPOSED" \
            "$SIGNAL" "$file_num" "$FILE_COUNT"

        decision="skip"
        [[ -f "$SIGNAL" ]] && decision=$(cat "$SIGNAL")

        if [[ "$decision" == "note" ]]; then
            show_note_popup "$rel_path" "$NOTE_FILE"
            continue
        elif [[ "$decision" == explain* ]]; then
            explain_file="${decision#explain:}"
            if [[ -z "$explain_file" ]]; then
                DIFF_FILE="$TEMP_DIR/diff_${file_num}.txt"
                diff -u --label "base: $rel_path" --label "current: $rel_path" \
                    "$ORIGINAL" "$PROPOSED" > "$DIFF_FILE" 2>/dev/null || true
                show_explanation_popup "$rel_path" "$DIFF_FILE" "diff"
            else
                show_explanation_popup "$rel_path" "$explain_file" "selection"
            fi
            continue
        elif [[ "$decision" == ask* ]]; then
            ask_file="${decision#ask:}"
            if [[ -z "$ask_file" ]]; then
                DIFF_FILE="$TEMP_DIR/diff_${file_num}.txt"
                diff -u --label "base: $rel_path" --label "current: $rel_path" \
                    "$ORIGINAL" "$PROPOSED" > "$DIFF_FILE" 2>/dev/null || true
                show_ask_popup "$rel_path" "$DIFF_FILE" "diff"
            else
                show_ask_popup "$rel_path" "$ask_file" "selection"
            fi
            continue
        fi
        break
    done

    case "$decision" in
        approve)
            APPROVED=$((APPROVED + 1))
            ;;
        reject)
            REJECTED=$((REJECTED + 1))
            FEEDBACK_FILE="$TEMP_DIR/fb_${file_num}.txt"
            rm -f "$FEEDBACK_FILE"
            tmux display-popup -E -w 70% -h 20% \
                bash "$SCRIPT_DIR/feedback-prompt.sh" "$FEEDBACK_FILE" 2>/dev/null || true
            if [[ -f "$FEEDBACK_FILE" ]] && [[ -s "$FEEDBACK_FILE" ]]; then
                fb=$(cat "$FEEDBACK_FILE")
                FEEDBACK_PARTS+=("### ${rel_path}"$'\n\n'"${fb}")
            else
                FEEDBACK_PARTS+=("### ${rel_path}"$'\n\n'"(No feedback provided — needs revision)")
            fi
            ;;
        skip|*)
            SKIPPED=$((SKIPPED + 1))
            ;;
    esac

done <<< "$CHANGED_FILES"

# ── Summary ─────────────────────────────────────────────────────

if [[ $REJECTED -gt 0 ]]; then
    FEEDBACK_OUT="$WORKDIR/.claude/final-review-feedback.md"
    {
        echo "# Final Review Feedback"
        echo ""
        printf "Reviewed: %s\n" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo ""
        printf "**Files reviewed:** %d | **Approved:** %d | **Needs work:** %d | **Skipped:** %d\n" \
            "$FILE_COUNT" "$APPROVED" "$REJECTED" "$SKIPPED"
        echo ""
        echo "## Files Needing Revision"
        echo ""
        for part in "${FEEDBACK_PARTS[@]}"; do
            echo "$part"
            echo ""
        done
    } > "$FEEDBACK_OUT"

    tmux display-popup -E -w 65% -h 50% bash -c "
        echo ''
        echo '  ╔═══════════════════════════════════╗'
        echo '  ║       FINAL REVIEW COMPLETE       ║'
        echo '  ╚═══════════════════════════════════╝'
        echo ''
        echo \"  ✓ Approved:    $APPROVED file(s)\"
        echo \"  ↻ Needs work:  $REJECTED file(s)\"
        [[ $SKIPPED -gt 0 ]] && echo \"  ⊘ Skipped:     $SKIPPED file(s)\"
        echo ''
        echo '  Feedback saved to:'
        echo '  .claude/final-review-feedback.md'
        echo ''
        echo '  Send the file to Claude to apply revisions:'
        echo '  /read .claude/final-review-feedback.md'
        echo ''
        read -rp '  Press Enter to close...'
    " 2>/dev/null || true
else
    tmux display-popup -E -w 60% -h 40% bash -c "
        echo ''
        echo '  ╔═══════════════════════════════════╗'
        echo '  ║       FINAL REVIEW COMPLETE       ║'
        echo '  ╚═══════════════════════════════════╝'
        echo ''
        echo \"  ✓ All $APPROVED file(s) approved!\"
        [[ $SKIPPED -gt 0 ]] && echo \"  ⊘ Skipped: $SKIPPED file(s)\"
        echo ''
        read -rp '  Press Enter to close...'
    " 2>/dev/null || true
fi
