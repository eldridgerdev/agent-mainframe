#!/usr/bin/env bash
set -u -o pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
WORKFLOW="amf-screenshot-artifact.yml"
GEOMETRY="120x40"
REF=""
PR_NUMBER=""
SCENARIO=""
OUT_DIR=""
SEED=""
SEED_FEATURE=""
CONFIG_FILE=""
GIF=0
STRICT=0
RETENTION_DAYS=14

usage() {
    cat <<EOF
Usage: $(basename "$0") --pr <number> --scenario <file> [options]

Dispatch the GitHub Actions screenshot workflow, download its artifact, and
replace the marked screenshot section in the PR body. Failures warn and exit
successfully by default so the surrounding PR workflow can continue.

Options:
  --pr <number>          Pull request to update (required)
  --scenario <file>      Scenario path relative to this repository (required)
  --ref <branch>         Pushed ref to capture (default: current branch)
  --geometry <WxH>       Capture geometry (default: $GEOMETRY)
  --seed <file>          Repository-relative automation seed payload
  --seed-feature <file>  Repository-relative feature seed payload
  --config <file>        Repository-relative AMF config JSON
  --gif                  Also assemble an animated GIF
  --out-dir <dir>        Download artifact here (default: temporary directory)
  --strict               Return nonzero when dispatch, capture, download, or
                         PR update fails
  -h, --help             Show this help

The uploaded artifact contains PNG/GIF files, ANSI and text captures,
capture-metadata.json, and a self-contained gallery.html. It is retained for
$RETENTION_DAYS days and the PR body points to the artifact page.
EOF
}

warn_or_exit() {
    echo "warning: $*" >&2
    if [[ "$STRICT" -eq 1 ]]; then
        exit 1
    fi
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pr) PR_NUMBER="${2:-}"; shift 2 ;;
        --scenario) SCENARIO="${2:-}"; shift 2 ;;
        --ref) REF="${2:-}"; shift 2 ;;
        --geometry) GEOMETRY="${2:-}"; shift 2 ;;
        --seed) SEED="${2:-}"; shift 2 ;;
        --seed-feature) SEED_FEATURE="${2:-}"; shift 2 ;;
        --config) CONFIG_FILE="${2:-}"; shift 2 ;;
        --gif) GIF=1; shift ;;
        --out-dir) OUT_DIR="${2:-}"; shift 2 ;;
        --strict) STRICT=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) warn_or_exit "unknown argument: $1" ;;
    esac
done

[[ "$PR_NUMBER" =~ ^[0-9]+$ ]] || warn_or_exit "--pr must be a numeric pull request number"
[[ -n "$SCENARIO" ]] || warn_or_exit "--scenario is required"
[[ "$GEOMETRY" =~ ^[0-9]+x[0-9]+$ ]] || warn_or_exit "--geometry must look like WxH"
command -v gh >/dev/null 2>&1 || warn_or_exit "GitHub CLI (gh) is not installed"

cd "$REPO_ROOT" || warn_or_exit "cannot enter repository root"
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || \
    warn_or_exit "could not resolve the GitHub repository"
REF="${REF:-$(git branch --show-current)}"
[[ -n "$REF" ]] || warn_or_exit "detached HEAD; pass --ref with a pushed branch"

resolve_repo_path() {
    local input="$1"
    local absolute
    absolute="$(realpath "$input" 2>/dev/null)" || return 1
    case "$absolute" in
        "$REPO_ROOT"/*) printf '%s\n' "${absolute#"$REPO_ROOT/"}" ;;
        *) return 1 ;;
    esac
}

SCENARIO_REF="$(resolve_repo_path "$SCENARIO")" || \
    warn_or_exit "scenario must be an existing file inside the repository"
[[ -f "$REPO_ROOT/$SCENARIO_REF" ]] || warn_or_exit "scenario not found: $SCENARIO"

optional_input_args=()
for pair in "seed:$SEED" "seed_feature:$SEED_FEATURE" "config:$CONFIG_FILE"; do
    key="${pair%%:*}"
    value="${pair#*:}"
    if [[ -n "$value" ]]; then
        resolved="$(resolve_repo_path "$value")" || warn_or_exit "$key must be inside the repository"
        [[ -f "$REPO_ROOT/$resolved" ]] || warn_or_exit "$key file not found: $value"
        optional_input_args+=(--raw-field "${key}=$resolved")
    fi
done

request_id="$(date +%Y%m%d-%H%M%S)-$$"
gif_value=false
[[ "$GIF" -eq 1 ]] && gif_value=true

dispatch_args=(
    "$WORKFLOW" --repo "$REPO" --ref "$REF"
    --raw-field "ref=$REF"
    --raw-field "scenario=$SCENARIO_REF"
    --raw-field "pr_number=$PR_NUMBER"
    --raw-field "geometry=$GEOMETRY"
    --raw-field "gif=$gif_value"
    --raw-field "request_id=$request_id"
)
dispatch_args+=("${optional_input_args[@]}")

if ! gh workflow run "${dispatch_args[@]}" >/dev/null 2>&1; then
    warn_or_exit "could not dispatch $WORKFLOW; ensure it is present on the remote default branch and gh is authenticated"
fi

run_id=""
for _ in {1..30}; do
    runs="$(gh run list --repo "$REPO" --workflow "$WORKFLOW" --branch "$REF" \
        --event workflow_dispatch --limit 30 --json databaseId,displayTitle 2>/dev/null || true)"
    run_id="$(RUNS_JSON="$runs" REQUEST_ID="$request_id" python3 - <<'PY'
import json, os
for run in json.loads(os.environ.get("RUNS_JSON", "") or "[]"):
    if os.environ["REQUEST_ID"] in run.get("displayTitle", ""):
        print(run["databaseId"])
        break
PY
)"
    [[ -n "$run_id" ]] && break
    sleep 2
done
[[ -n "$run_id" ]] || warn_or_exit "workflow dispatched but its run could not be located"

run_url="https://github.com/$REPO/actions/runs/$run_id"
status=""
conclusion=""
for _ in {1..180}; do
    run_json="$(gh run view "$run_id" --repo "$REPO" --json status,conclusion 2>/dev/null || true)"
    read -r status conclusion < <(RUN_JSON="$run_json" python3 - <<'PY'
import json, os
data = json.loads(os.environ.get("RUN_JSON", "") or "{}")
print(data.get("status", ""), data.get("conclusion", ""))
PY
)
    [[ "$status" == "completed" ]] && break
    sleep 5
done
if [[ "$status" != "completed" || "$conclusion" != "success" ]]; then
    warn_or_exit "screenshot workflow did not succeed (status=$status conclusion=$conclusion): $run_url"
fi

artifact_name="amf-screenshot-pr-${PR_NUMBER}-${run_id}"
artifact_json="$(mktemp "${TMPDIR:-/tmp}/amf-artifact.XXXXXX.json")"
trap 'rm -f "$artifact_json"' EXIT
if ! gh api "repos/$REPO/actions/runs/$run_id/artifacts?per_page=100" >"$artifact_json" 2>/dev/null; then
    warn_or_exit "workflow succeeded but artifact metadata could not be read: $run_url"
fi

read -r artifact_id expires_at < <(ARTIFACT_JSON="$artifact_json" ARTIFACT_NAME="$artifact_name" python3 - <<'PY'
import json, os
data = json.load(open(os.environ["ARTIFACT_JSON"], encoding="utf-8"))
for artifact in data.get("artifacts", []):
    if artifact.get("name") == os.environ["ARTIFACT_NAME"]:
        print(artifact.get("id", ""), artifact.get("expires_at", ""))
        break
PY
)
[[ -n "${artifact_id:-}" ]] || warn_or_exit "workflow succeeded but artifact '$artifact_name' was not found: $run_url"

artifact_url="https://github.com/$REPO/actions/runs/$run_id/artifacts/$artifact_id"
if [[ -z "$OUT_DIR" ]]; then
    OUT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/amf-screenshot-artifact.XXXXXX")"
else
    mkdir -p "$OUT_DIR" || warn_or_exit "could not create --out-dir '$OUT_DIR'"
    out_abs="$(cd "$OUT_DIR" && pwd -P)"
    case "$out_abs" in
        "$REPO_ROOT"/*) warn_or_exit "--out-dir must be outside the repository" ;;
    esac
fi

if ! gh run download "$run_id" --repo "$REPO" --name "$artifact_name" --dir "$OUT_DIR" >/dev/null 2>&1; then
    warn_or_exit "artifact was created but could not be downloaded: $artifact_url"
fi

fragment="$OUT_DIR/pr-description.md"
python3 - "$fragment" "$artifact_url" "$PR_NUMBER" "$REF" "$expires_at" <<'PY'
import sys
from pathlib import Path

out, url, pr, ref, expires = sys.argv[1:]
Path(out).write_text(
    "<!-- amf:screenshots:start -->\n"
    f"### Visual proof\n\n"
    f"[Download the AMF screenshot artifact (PR #{pr}, `{ref}`)]({url})\n\n"
    f"The artifact includes a self-contained `gallery.html`, rendered PNG/GIF "
    f"files, text assertions, and capture metadata. It expires on `{expires}`.\n"
    "<!-- amf:screenshots:end -->\n",
    encoding="utf-8",
)
PY

body_file="$(mktemp "${TMPDIR:-/tmp}/amf-pr-body.XXXXXX")"
updated_body="$(mktemp "${TMPDIR:-/tmp}/amf-pr-body-updated.XXXXXX")"
trap 'rm -f "$artifact_json" "$body_file" "$updated_body"' EXIT
if ! gh pr view "$PR_NUMBER" --repo "$REPO" --json body --jq .body >"$body_file" 2>/dev/null; then
    warn_or_exit "artifact is ready, but the existing PR body could not be read: $artifact_url"
fi
if ! python3 "$SCRIPT_DIR/update_pr_body.py" --body-file "$body_file" \
    --fragment-file "$fragment" --output-file "$updated_body"; then
    warn_or_exit "artifact is ready, but the PR screenshot markers could not be updated: $artifact_url"
fi
if ! gh pr edit "$PR_NUMBER" --repo "$REPO" --body-file "$updated_body" >/dev/null 2>&1; then
    warn_or_exit "artifact is ready, but the PR body update failed: $artifact_url"
fi

echo "artifact: $artifact_url"
echo "expires: $expires_at"
echo "local output: $OUT_DIR"
echo "PR updated: #$PR_NUMBER"
