#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

AMF_BIN=""
OUT_DIR=""
GEOMETRY="120x40"
KEEP=0
SCENARIO=""
SEED=""
SEED_FEATURE=""
GIF=0
GIF_PATH=""
READY_TIMEOUT_SECS=15

usage() {
    cat <<EOF
Usage: $(basename "$0") [options]

Launches an isolated, throwaway AMF instance in its own tmux session
(own XDG_CONFIG_HOME / XDG_STATE_HOME, real HOME preserved), waits for
the dashboard to come up, sends a few keys, and dumps raw ANSI
capture-pane output for later rendering. Never touches the real
~/.config/amf or a real amf tmux session.

Options:
  --amf-bin <path>     Path to the amf binary (default: builds/uses
                        $REPO_ROOT/target/debug/amf)
  --out-dir <dir>       Where to write numbered .ansi captures
                        (default: <scratch-root>/shots)
  --geometry <WxH>      tmux session geometry (default: $GEOMETRY)
  --scenario <file>     Newline-delimited steps, one per line, each a
                        '|'-separated list of:
                          key:<name>   tmux send-keys key name (e.g.
                                       key:Enter, key:j)
                          text:<text>  tmux send-keys -l literal text
                                       (e.g. text:my feature name)
                          wait:<ms>    sleep this many milliseconds
                          shot:<label> capture-pane -> NNN-<label>.ansi
                                       (+ escape-free NNN-<label>.txt)
                        Blank lines and lines starting with '#' are
                        skipped. Without --scenario, runs a small
                        built-in smoke test (dashboard-ready, press j,
                        after-j).
  --seed <file>         Automation JSON payload (see docs/automation/)
                        applied against the scratch instance once the
                        dashboard is ready, before any --scenario steps
                        run. The action (create-project vs
                        create-feature) is inferred from the payload's
                        top-level keys ('path' -> create-project,
                        'branch' -> create-feature).
  --seed-feature <file> A second automation JSON payload, always applied
                        as create-feature, right after --seed. Lets a
                        project (--seed) and its first feature be seeded
                        together in one scratch instance -- e.g.
                        scenarios/seed-project.json paired with
                        scenarios/seed-feature.json, whose project_name
                        must match. A plain --seed only ever runs one
                        automation call, so a scenario needing a feature
                        already present (a project with no features has
                        nothing for most scenarios to show) needs this.
  --gif [path]          After all shot: steps, render every numbered
                        .ansi capture to a PNG and assemble them into
                        an animated GIF. Path defaults to
                        <out-dir>/capture.gif. Off by default.
  --keep                Keep the scratch root (config/state/shots) on
                        exit instead of deleting it.
  -h, --help            Show this help.

Environment:
  AMF_SHOT_DIR          Scratch root parent (default: /tmp/amf-shots)
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --amf-bin)
            AMF_BIN="$2"
            shift 2
            ;;
        --out-dir)
            OUT_DIR="$2"
            shift 2
            ;;
        --geometry)
            GEOMETRY="$2"
            shift 2
            ;;
        --scenario)
            SCENARIO="$2"
            shift 2
            ;;
        --seed)
            SEED="$2"
            shift 2
            ;;
        --seed-feature)
            SEED_FEATURE="$2"
            shift 2
            ;;
        --gif)
            GIF=1
            shift
            # Optional path arg: only consume it if present and not
            # itself another flag.
            if [[ $# -gt 0 && "$1" != -* ]]; then
                GIF_PATH="$1"
                shift
            fi
            ;;
        --keep)
            KEEP=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
done

if ! command -v tmux >/dev/null 2>&1; then
    echo "error: tmux not found in PATH" >&2
    exit 1
fi

if [[ ! "$GEOMETRY" =~ ^[0-9]+x[0-9]+$ ]]; then
    echo "error: --geometry must look like WxH (got '$GEOMETRY')" >&2
    exit 1
fi
COLS="${GEOMETRY%x*}"
ROWS="${GEOMETRY#*x}"

if [[ -n "$SCENARIO" && ! -f "$SCENARIO" ]]; then
    echo "error: scenario file not found: $SCENARIO" >&2
    exit 1
fi

if [[ -n "$SEED" && ! -f "$SEED" ]]; then
    echo "error: seed file not found: $SEED" >&2
    exit 1
fi

if [[ -n "$SEED_FEATURE" && ! -f "$SEED_FEATURE" ]]; then
    echo "error: seed-feature file not found: $SEED_FEATURE" >&2
    exit 1
fi

TS="$(date +%Y%m%d-%H%M%S)-$$"
SHOT_ROOT="${AMF_SHOT_DIR:-/tmp/amf-shots}/$TS"
CONFIG_DIR="$SHOT_ROOT/config"
STATE_DIR="$SHOT_ROOT/state"
OUT_DIR="${OUT_DIR:-$SHOT_ROOT/shots}"
SESSION="amf-shot-$TS"
if [[ "$GIF" -eq 1 && -z "$GIF_PATH" ]]; then
    GIF_PATH="$OUT_DIR/capture.gif"
fi

# Pre-create the XDG amf subdirs (not just their parents). AMF's
# amf_config_dir() falls back to the legacy ~/.config/amf path whenever
# the real HOME already has one AND the XDG-resolved dir doesn't exist
# yet -- see src/project.rs amf_config_dir_with(). Since we deliberately
# keep the real HOME (so `claude` auth / git identity work), the XDG amf
# dir must already exist before amf starts, or it will silently open the
# user's real database instead of this scratch one.
mkdir -p "$CONFIG_DIR/amf" "$STATE_DIR/amf" "$OUT_DIR"

if [[ -z "$AMF_BIN" ]]; then
    AMF_BIN="$REPO_ROOT/target/debug/amf"
    if [[ ! -x "$AMF_BIN" ]]; then
        echo "amf binary not found, building (cargo build -j 2)..." >&2
        (cd "$REPO_ROOT" && cargo build -j 2)
    fi
fi
if [[ ! -x "$AMF_BIN" ]]; then
    echo "error: amf binary not found or not executable: $AMF_BIN" >&2
    exit 1
fi

# Snapshot tmux sessions that exist *before* we touch anything, so cleanup
# can find whatever AMF itself spawns beyond $SESSION -- see the note by
# `cleanup()` below.
BASELINE_SESSIONS="$(tmux list-sessions -F '#{session_name}' 2>/dev/null || true)"

cleanup() {
    tmux kill-session -t "$SESSION" >/dev/null 2>&1 || true
    if [[ "$KEEP" -ne 1 ]]; then
        # AMF starting a real feature (via --seed or a scenario driving the
        # creation wizard) spawns its own top-level tmux session
        # (amf-<project>-<feature>), launching a real `claude`/`codex`/etc.
        # process. That session lives on the shared tmux server, not nested
        # under $SESSION, so killing $SESSION above doesn't reach it and it
        # would otherwise run forever after this "throwaway" script exits.
        # Diff against the pre-run snapshot (rather than pattern-matching
        # names) so this works regardless of what the feature/project was
        # named.
        while IFS= read -r s; do
            [[ -z "$s" || "$s" == "$SESSION" ]] && continue
            if ! grep -qxF "$s" <<<"$BASELINE_SESSIONS"; then
                tmux kill-session -t "$s" >/dev/null 2>&1 || true
                echo "cleaned up spawned session: $s" >&2
            fi
        done < <(tmux list-sessions -F '#{session_name}' 2>/dev/null || true)
        rm -rf "$SHOT_ROOT"
    fi
}
trap cleanup EXIT

# gh keeps its own auth under XDG_CONFIG_HOME/gh by default. Resolve the
# *real* one before we override XDG_CONFIG_HOME below for AMF's isolation --
# otherwise every `gh` call AMF shells out to (PR Triage, PR picker, etc.)
# would see an empty, unauthenticated config under the scratch root instead
# of the user's real credentials. Pinning GH_CONFIG_DIR explicitly keeps gh
# on the real config while AMF's own config/state stay isolated, the same
# way HOME is left alone below for `claude` auth / git identity.
REAL_GH_CONFIG_DIR="${GH_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/gh}"

export XDG_CONFIG_HOME="$CONFIG_DIR"
export XDG_STATE_HOME="$STATE_DIR"
# HOME is intentionally left untouched so `claude` auth and git identity
# keep working inside the scratch instance.

echo "scratch root: $SHOT_ROOT" >&2
echo "tmux session: $SESSION ($GEOMETRY)" >&2

# `-e` is required, not just `export`: when a tmux server is already
# running (e.g. the user's own amf-* sessions), `new-session` attaches
# to that existing server, and the new session's process environment is
# NOT refreshed from this shell's just-exported vars -- it falls back to
# whatever environment the server itself was started with. Without `-e`
# here, amf silently launches against the real ~/.config/amf instead of
# the scratch one.
tmux new-session -d -s "$SESSION" -x "$COLS" -y "$ROWS" -c "$SHOT_ROOT" \
    -e "XDG_CONFIG_HOME=$CONFIG_DIR" \
    -e "XDG_STATE_HOME=$STATE_DIR" \
    -e "GH_CONFIG_DIR=$REAL_GH_CONFIG_DIR" \
    "$AMF_BIN"

step=0
shot() {
    local label="$1"
    step=$((step + 1))
    local file
    file="$(printf '%s/%03d-%s.ansi' "$OUT_DIR" "$step" "$label")"
    tmux capture-pane -e -p -t "$SESSION" >"$file"
    # Plain-text twin: escape-free, so an agent can grep/read it to verify
    # content far more cheaply than reading the .ansi or the rendered PNG.
    tmux capture-pane -p -t "$SESSION" >"${file%.ansi}.txt"
    echo "shot: $file" >&2
}

wait_for_text() {
    local needle="$1"
    local timeout_secs="$2"
    local waited_ms=0
    local interval_ms=200
    while (( waited_ms < timeout_secs * 1000 )); do
        if tmux capture-pane -p -t "$SESSION" 2>/dev/null | grep -qF "$needle"; then
            return 0
        fi
        sleep 0.2
        waited_ms=$((waited_ms + interval_ms))
    done
    echo "error: timed out after ${timeout_secs}s waiting for dashboard text: '$needle'" >&2
    echo "--- last captured pane ---" >&2
    tmux capture-pane -p -t "$SESSION" >&2 || true
    return 1
}

wait_for_any() {
    local timeout_secs="$1"
    shift
    local needles=("$@")
    local waited_ms=0
    local pane needle
    while (( waited_ms < timeout_secs * 1000 )); do
        pane="$(tmux capture-pane -p -t "$SESSION" 2>/dev/null || true)"
        for needle in "${needles[@]}"; do
            if grep -qF "$needle" <<<"$pane"; then
                echo "$needle"
                return 0
            fi
        done
        sleep 0.2
        waited_ms=$((waited_ms + 200))
    done
    echo "error: timed out after ${timeout_secs}s waiting for any of: ${needles[*]}" >&2
    echo "--- last captured pane ---" >&2
    tmux capture-pane -p -t "$SESSION" >&2 || true
    return 1
}

# On a truly fresh scratch config, amf shows a one-time "Configure Agent
# Harnesses" onboarding dialog before the dashboard. Esc/c only confirm
# once at least one harness is enabled, so select the first entry
# (Enter kicks off an availability check), wait for it to resolve, then
# confirm.
first_screen="$(wait_for_any "$READY_TIMEOUT_SECS" "No projects yet" "Configure Agent Harnesses")"
if [[ "$first_screen" == "Configure Agent Harnesses" ]]; then
    echo "resolving first-run harness setup dialog" >&2
    tmux send-keys -t "$SESSION" Enter
    wait_for_any "$READY_TIMEOUT_SECS" "(installed)" "(not found" >/dev/null
    tmux send-keys -t "$SESSION" c
    wait_for_text "No projects yet" "$READY_TIMEOUT_SECS"
fi
echo "dashboard ready" >&2

# The automation JSON shape (docs/automation/*.template.json) has no
# "action" field of its own -- the CLI subcommand IS the action, so we
# infer create-project vs create-feature from which distinguishing key
# is present ('path' only on create-project, 'branch' only on
# create-feature). python3 is already a hard dependency (render_ansi.py).
seed_kind() {
    python3 - "$1" <<'PY'
import json
import sys

with open(sys.argv[1]) as f:
    data = json.load(f)
if "path" in data:
    print("create-project")
elif "branch" in data:
    print("create-feature")
else:
    print("unknown")
PY
}

if [[ -n "$SEED" ]]; then
    kind="$(seed_kind "$SEED")"
    if [[ "$kind" == "unknown" ]]; then
        echo "error: could not infer automation action from seed file '$SEED' (expected a 'path' key for create-project or a 'branch' key for create-feature)" >&2
        exit 1
    fi
    echo "seeding: $AMF_BIN automation $kind --file $SEED" >&2
    "$AMF_BIN" automation "$kind" --file "$SEED"
fi

if [[ -n "$SEED_FEATURE" ]]; then
    echo "seeding: $AMF_BIN automation create-feature --file $SEED_FEATURE" >&2
    "$AMF_BIN" automation create-feature --file "$SEED_FEATURE"
fi

run_scenario() {
    local file="$1"
    local line
    while IFS= read -r line || [[ -n "$line" ]]; do
        [[ -z "$line" || "$line" == \#* ]] && continue
        local IFS_SAVE="$IFS"
        IFS='|'
        read -ra parts <<<"$line"
        IFS="$IFS_SAVE"
        local part
        for part in "${parts[@]}"; do
            case "$part" in
                key:*)
                    tmux send-keys -t "$SESSION" "${part#key:}"
                    ;;
                text:*)
                    # -l: literal mode, so special chars aren't parsed as
                    # tmux key names.
                    tmux send-keys -t "$SESSION" -l -- "${part#text:}"
                    ;;
                wait:*)
                    local ms="${part#wait:}"
                    sleep "$(awk "BEGIN { printf \"%.3f\", $ms / 1000 }")"
                    ;;
                shot:*)
                    shot "${part#shot:}"
                    ;;
                "")
                    ;;
                *)
                    echo "warning: unrecognized scenario step '$part'" >&2
                    ;;
            esac
        done
    done <"$file"
}

if [[ -n "$SCENARIO" ]]; then
    run_scenario "$SCENARIO"
else
    shot "dashboard-ready"
    tmux send-keys -t "$SESSION" "j"
    sleep 0.3
    shot "after-j"
fi

echo "shots written to: $OUT_DIR" >&2

if [[ "$GIF" -eq 1 ]]; then
    shopt -s nullglob
    ansi_files=("$OUT_DIR"/*.ansi)
    shopt -u nullglob
    if [[ ${#ansi_files[@]} -eq 0 ]]; then
        echo "warning: no .ansi captures found in $OUT_DIR, skipping --gif" >&2
    else
        png_files=()
        for f in "${ansi_files[@]}"; do
            png="${f%.ansi}.png"
            # Explicit --cols/--rows: without them render_ansi.py infers
            # grid size from each frame's own content, which can vary
            # frame to frame and produce mismatched-size PNGs that Pillow
            # can't assemble into one GIF.
            python3 "$SCRIPT_DIR/render_ansi.py" "$f" --out "$png" --cols "$COLS" --rows "$ROWS" >/dev/null
            png_files+=("$png")
        done
        mkdir -p "$(dirname "$GIF_PATH")"
        python3 "$SCRIPT_DIR/assemble_gif.py" --out "$GIF_PATH" "${png_files[@]}"
        echo "gif written to: $GIF_PATH" >&2
    fi
fi
