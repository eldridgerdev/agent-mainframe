#!/bin/bash
#
# custom-diff-review.sh — AMF custom diff review hook
#
# Sends a structured diff-review request to the AMF TUI and waits for
# approve/reject/cancel. Falls back to file-based notification when the
# IPC path is unavailable.
#

set -euo pipefail

if [[ "${AMF_ACTIVE:-}" != "1" ]]; then
    exit 0
fi

# Root-repository Claude settings are inherited by sessions launched from Git
# worktrees. AMF passes the Git root that owns this hook as the first argument;
# an inherited hook must not review changes for a different feature.
EXPECTED_WORKDIR="${1:-}"
if [[ -n "$EXPECTED_WORKDIR" ]]; then
    EXPECTED_GIT_ROOT=$(git -C "$EXPECTED_WORKDIR" rev-parse --show-toplevel 2>/dev/null || true)
    ACTIVE_GIT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || true)
    if [[ -n "$EXPECTED_GIT_ROOT" ]]; then
        EXPECTED_GIT_ROOT=$(cd "$EXPECTED_GIT_ROOT" && pwd -P)
    fi
    if [[ -n "$ACTIVE_GIT_ROOT" ]]; then
        ACTIVE_GIT_ROOT=$(cd "$ACTIVE_GIT_ROOT" && pwd -P)
    fi
    if [[ -z "$EXPECTED_GIT_ROOT" || -z "$ACTIVE_GIT_ROOT" || "$ACTIVE_GIT_ROOT" != "$EXPECTED_GIT_ROOT" ]]; then
        exit 0
    fi
fi

HOOK_INPUT=$(cat)

# `amf hook-field` reads the hook JSON so this script needs no JSON parser of
# its own. It prints nothing and exits 0 for an absent field, which is the same
# shape as the `jq -r '... // empty'` it replaces.
AMF_CMD="${AMF_BIN:-amf}"
hook_field() {
    printf '%s' "$HOOK_INPUT" | "$AMF_CMD" hook-field "$@" 2>/dev/null || true
}

TOOL_NAME=$(hook_field tool_name)
SESSION_ID=$(hook_field session_id)
SESSION_ID="${SESSION_ID:-unknown}"
AMF_SESSION_ID="${AMF_SESSION:-}"

if [[ "$TOOL_NAME" != "Edit" && "$TOOL_NAME" != "Write" ]]; then
    exit 0
fi

CWD=$(hook_field cwd)
FILE_PATH=$(hook_field tool_input.file_path)

if [[ -z "$FILE_PATH" ]]; then
    exit 0
fi

if [[ -n "$CWD" && "$FILE_PATH" == "$CWD"/* ]]; then
    DISPLAY_PATH="${FILE_PATH#"$CWD"/}"
else
    DISPLAY_PATH="$FILE_PATH"
fi

FILE_EXT="${FILE_PATH##*.}"
INVOCATION_ID="$$"
TEMP_DIR="/tmp/claude-review/custom/$SESSION_ID/$INVOCATION_ID"
ORIGINAL_FILE="$TEMP_DIR/original.$FILE_EXT"
PROPOSED_FILE="$TEMP_DIR/proposed.$FILE_EXT"
RESPONSE_FILE="$TEMP_DIR/response.json"
PROCEED_SIGNAL="$TEMP_DIR/proceed"
GIT_ROOT=$(git -C "${CWD}" rev-parse --show-toplevel 2>/dev/null || echo "${CWD}")
NOTIFY_DIR="${GIT_ROOT}/.claude/notifications"
NOTIFICATION_FILE="$NOTIFY_DIR/${SESSION_ID}-diff-${INVOCATION_ID}.json"
IS_NEW_FILE=false
if [[ ! -f "$FILE_PATH" ]]; then
    IS_NEW_FILE=true
fi

log_fallback() {
    local reason="$1"
    local home_dir="${HOME:-/tmp}"
    local state_home="${XDG_STATE_HOME:-$home_dir/.local/state}"
    local log_dir="$state_home/amf"
    mkdir -p "$log_dir" 2>/dev/null || true
    printf '[WARN] [diff-review] notify-wait fallback for session=%s amf_session=%s cwd=%s file=%s: %s\n' \
        "$SESSION_ID" "$AMF_SESSION_ID" "$CWD" "$DISPLAY_PATH" "$reason" \
        >> "$log_dir/debug.log" 2>/dev/null || true
}

cleanup() {
    rm -f "$NOTIFICATION_FILE" 2>/dev/null || true
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

capture_original_file() {
    mkdir -p "$TEMP_DIR"
    rm -f "$RESPONSE_FILE" "$PROCEED_SIGNAL"
    if [[ -f "$FILE_PATH" ]]; then
        cp "$FILE_PATH" "$ORIGINAL_FILE"
    else
        touch "$ORIGINAL_FILE"
    fi
}

create_proposed_file() {
    if [[ "$TOOL_NAME" == "Write" ]]; then
        hook_field tool_input.content > "$PROPOSED_FILE"
    else
        local old_string_file="$TEMP_DIR/old_string"
        local new_string_file="$TEMP_DIR/new_string"
        hook_field tool_input.old_string > "$old_string_file"
        hook_field tool_input.new_string > "$new_string_file"
        OLD_FILE="$old_string_file" NEW_FILE="$new_string_file" perl -0777 -pe '
            BEGIN {
                open(F, "<", $ENV{OLD_FILE}) or die; local $/; $old = <F>; close F;
                open(F, "<", $ENV{NEW_FILE}) or die; local $/; $new = <F>; close F;
                chomp $old; chomp $new;
            }
            s/\Q$old\E/$new/s;
        ' "$ORIGINAL_FILE" > "$PROPOSED_FILE"
    fi
}

has_changes() {
    [[ "$TOOL_NAME" == "Write" ]] && return 0
    ! diff -q "$ORIGINAL_FILE" "$PROPOSED_FILE" > /dev/null 2>&1
}

old_snippet() {
    if [[ "$TOOL_NAME" == "Write" ]]; then
        printf ""
    else
        hook_field tool_input.old_string
    fi
}

new_snippet() {
    if [[ "$TOOL_NAME" == "Write" ]]; then
        hook_field tool_input.content
    else
        hook_field tool_input.new_string
    fi
}

build_payload() {
    local payload_file="$TEMP_DIR/payload.json"

    # Snippets go via files, not argv: an edit's old/new strings are whole
    # regions of a source file and would risk ARG_MAX on the command line.
    # `mkdir` here rather than relying on `capture_original_file` having run —
    # writing the payload should not depend on another step's side effect.
    mkdir -p "$TEMP_DIR"
    local old_file="$TEMP_DIR/old_snippet" new_file="$TEMP_DIR/new_snippet"
    old_snippet > "$old_file"
    new_snippet > "$new_file"

    # `session_id` is deliberately the *provider's* id with the AMF session
    # carried alongside in `amf_session`; the dashboard matches this
    # notification on both. An explicit --field outranks the environment, so
    # `amf hook-payload` will not substitute the tmux session here.
    "$AMF_CMD" hook-payload \
        --type diff-review \
        --field "session_id=$SESSION_ID" \
        --field "amf_session=$AMF_SESSION_ID" \
        --field "cwd=$CWD" \
        --field "message=Review: $DISPLAY_PATH" \
        --field "file_path=$FILE_PATH" \
        --field "relative_path=$DISPLAY_PATH" \
        --field "tool=$(printf '%s' "$TOOL_NAME" | tr '[:upper:]' '[:lower:]')" \
        --field "change_id=$INVOCATION_ID" \
        --field "original_file=$ORIGINAL_FILE" \
        --field "proposed_file=$PROPOSED_FILE" \
        --field "response_file=$RESPONSE_FILE" \
        --field "proceed_signal=$PROCEED_SIGNAL" \
        --field-from-file "old_snippet=$old_file" \
        --field-from-file "new_snippet=$new_file" \
        --bool-field "is_new_file=$IS_NEW_FILE" \
        < /dev/null > "$payload_file"

    printf "%s" "$payload_file"
}

send_notification_wait() {
    local payload_file="$1"
    if ! command -v "$AMF_CMD" >/dev/null 2>&1; then
        log_fallback "amf command not found"
        return 1
    fi

    local response
    local error_file="$TEMP_DIR/notify-wait.err"
    if ! response=$(cat "$payload_file" | "$AMF_CMD" notify-wait --timeout-ms 120000 2>"$error_file"); then
        local error
        error=$(tr '\n' ' ' < "$error_file" | sed 's/[[:space:]]\+/ /g' | cut -c1-240)
        log_fallback "${error:-notify-wait failed}"
        return 1
    fi

    printf "%s" "$response"
}

write_notification() {
    local payload_file="$1"
    mkdir -p "$NOTIFY_DIR"
    cp "$payload_file" "$NOTIFICATION_FILE"
}

wait_for_response_file() {
    while [[ ! -f "$PROCEED_SIGNAL" ]]; do
        sleep 0.3
    done
    if [[ -f "$RESPONSE_FILE" ]]; then
        cat "$RESPONSE_FILE"
    fi
}

handle_response() {
    local response="$1"
    local decision reject skip reason

    # AMF replies flat — `{type, decision, reason, skip, reject}` — with
    # `decision` the current field and the booleans sent alongside it. Both are
    # read because either alone is a complete answer.
    decision=$(printf '%s' "$response" | "$AMF_CMD" hook-field decision 2>/dev/null || true)

    if [[ -z "$decision" ]]; then
        reject=$(printf '%s' "$response" | "$AMF_CMD" hook-field reject 2>/dev/null || true)
        skip=$(printf '%s' "$response" | "$AMF_CMD" hook-field skip 2>/dev/null || true)
        if [[ "$reject" == "true" ]]; then
            decision="reject"
        elif [[ "$skip" == "true" ]]; then
            decision="cancel"
        else
            decision="proceed"
        fi
    fi

    case "$decision" in
        proceed)
            exit 0
            ;;
        reject)
            reason=$(printf '%s' "$response" | "$AMF_CMD" hook-field reason 2>/dev/null || true)
            if [[ -n "$reason" ]]; then
                echo "User rejected this change with feedback: $reason" >&2
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
}

main() {
    capture_original_file
    create_proposed_file

    if ! has_changes; then
        exit 0
    fi

    local payload_file response
    payload_file=$(build_payload)

    if response=$(send_notification_wait "$payload_file"); then
        handle_response "$response"
    fi

    write_notification "$payload_file"
    response=$(wait_for_response_file)
    handle_response "$response"
}

main
