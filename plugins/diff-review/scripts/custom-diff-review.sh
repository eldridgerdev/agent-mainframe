#!/bin/bash
#
# custom-diff-review.sh — AMF custom diff review hook
#
# Sends a structured diff-review request to the AMF TUI and waits for
# approve/reject/cancel. Falls back to file-based notification when the
# IPC path is unavailable.
#

set -euo pipefail

HOOK_INPUT=$(cat)
TOOL_NAME=$(echo "$HOOK_INPUT" | jq -r '.tool_name // empty')
SESSION_ID=$(echo "$HOOK_INPUT" | jq -r '.session_id // "unknown"')

if [[ "$TOOL_NAME" != "Edit" && "$TOOL_NAME" != "Write" ]]; then
    exit 0
fi

CWD=$(echo "$HOOK_INPUT" | jq -r '.cwd // empty')
TOOL_INPUT=$(echo "$HOOK_INPUT" | jq -r '.tool_input // empty')
FILE_PATH=$(echo "$TOOL_INPUT" | jq -r '.file_path // empty')

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
        echo "$TOOL_INPUT" | jq -r '.content // empty' > "$PROPOSED_FILE"
    else
        local old_string_file="$TEMP_DIR/old_string"
        local new_string_file="$TEMP_DIR/new_string"
        echo "$TOOL_INPUT" | jq -r '.old_string // empty' > "$old_string_file"
        echo "$TOOL_INPUT" | jq -r '.new_string // empty' > "$new_string_file"
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
        echo "$TOOL_INPUT" | jq -r '.old_string // empty'
    fi
}

new_snippet() {
    if [[ "$TOOL_NAME" == "Write" ]]; then
        echo "$TOOL_INPUT" | jq -r '.content // empty'
    else
        echo "$TOOL_INPUT" | jq -r '.new_string // empty'
    fi
}

build_payload() {
    local payload_file="$TEMP_DIR/payload.json"
    jq -n \
        --arg sid "$SESSION_ID" \
        --arg cwd "$CWD" \
        --arg msg "Review: $DISPLAY_PATH" \
        --arg fp "$FILE_PATH" \
        --arg rel "$DISPLAY_PATH" \
        --arg tool "$(echo "$TOOL_NAME" | tr '[:upper:]' '[:lower:]')" \
        --arg change_id "$INVOCATION_ID" \
        --arg old "$(old_snippet)" \
        --arg new "$(new_snippet)" \
        --arg original_file "$ORIGINAL_FILE" \
        --arg proposed_file "$PROPOSED_FILE" \
        --arg response_file "$RESPONSE_FILE" \
        --arg proceed_signal "$PROCEED_SIGNAL" \
        --argjson is_new_file "$IS_NEW_FILE" \
        '{
            type: "diff-review",
            session_id: $sid,
            cwd: $cwd,
            message: $msg,
            file_path: $fp,
            relative_path: $rel,
            tool: $tool,
            change_id: $change_id,
            old_snippet: $old,
            new_snippet: $new,
            original_file: $original_file,
            proposed_file: $proposed_file,
            is_new_file: $is_new_file,
            response_file: $response_file,
            proceed_signal: $proceed_signal
        }' > "$payload_file"
    printf "%s" "$payload_file"
}

send_notification_wait() {
    local payload_file="$1"
    if ! command -v amf >/dev/null 2>&1; then
        return 1
    fi

    local response
    if ! response=$(cat "$payload_file" | amf notify-wait --timeout-ms 120000 2>/dev/null); then
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
    local decision
    decision=$(echo "$response" | jq -r '.decision // empty' 2>/dev/null || true)

    if [[ -z "$decision" ]]; then
        local reject skip
        reject=$(echo "$response" | jq -r '.reject // false' 2>/dev/null || echo "false")
        skip=$(echo "$response" | jq -r '.skip // false' 2>/dev/null || echo "false")
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
            local reason
            reason=$(echo "$response" | jq -r '.reason // empty' 2>/dev/null || true)
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
