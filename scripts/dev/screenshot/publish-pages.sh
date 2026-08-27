#!/usr/bin/env bash
# Publish browser-viewable screenshot evidence without exposing Pages credentials
# to the requested capture ref. See amf-screenshot-artifact.yml.
set -u -o pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
WORKFLOW="amf-screenshot-artifact.yml"
WORKFLOW_REF="main"
PR_NUMBER=""
SCENARIO=""
REF=""
PROJECT="${CF_PAGES_PROJECT:-}"
GEOMETRY="120x40"
GIF=false
STRICT=0

usage() {
  cat <<EOF
Usage: $(basename "$0") --pr <number> --scenario <file> [options]

Dispatch a protected Pages-preview capture. The workflow itself is always run
from main; only its capture job checks out --ref. Failures warn by default.

  --pr <number>          Pull request to update (required)
  --scenario <file>      Repository-relative scenario (required)
  --ref <branch>         Pushed ref to capture (default: current branch)
  --pages-project <name> Pages project (default: CF_PAGES_PROJECT variable)
  --geometry <WxH>       Capture geometry (default: 120x40)
  --gif                  Include an animated GIF
  --strict               Return nonzero on failure
EOF
}

warn() { echo "warning: $*" >&2; [[ "$STRICT" -eq 1 ]] && exit 1; exit 0; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pr) PR_NUMBER="${2:-}"; shift 2 ;;
    --scenario) SCENARIO="${2:-}"; shift 2 ;;
    --ref) REF="${2:-}"; shift 2 ;;
    --pages-project) PROJECT="${2:-}"; shift 2 ;;
    --geometry) GEOMETRY="${2:-}"; shift 2 ;;
    --gif) GIF=true; shift ;;
    --strict) STRICT=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) warn "unknown argument: $1" ;;
  esac
done

[[ "$PR_NUMBER" =~ ^[0-9]+$ ]] || warn "--pr must be numeric"
[[ "$GEOMETRY" =~ ^[0-9]+x[0-9]+$ ]] || warn "--geometry must look like WxH"
[[ -n "$SCENARIO" ]] || warn "--scenario is required"
command -v gh >/dev/null 2>&1 || warn "GitHub CLI (gh) is not installed"
cd "$REPO_ROOT" || warn "cannot enter repository root"
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || warn "could not resolve GitHub repository"
REF="${REF:-$(git branch --show-current)}"
[[ -n "$REF" ]] || warn "detached HEAD; pass --ref with a pushed branch"
[[ -n "$PROJECT" ]] || PROJECT="$(gh variable get CF_PAGES_PROJECT --repo "$REPO" 2>/dev/null || true)"
[[ "$PROJECT" =~ ^[a-z0-9-]+$ ]] || warn "set CF_PAGES_PROJECT or pass --pages-project"

scenario_abs="$(realpath "$SCENARIO" 2>/dev/null)" || warn "scenario must exist"
case "$scenario_abs" in "$REPO_ROOT"/*) SCENARIO="${scenario_abs#"$REPO_ROOT/"}" ;; *) warn "scenario must be inside repository" ;; esac

request_id="$(date +%Y%m%d-%H%M%S)-$$"
if ! gh workflow run "$WORKFLOW" --repo "$REPO" --ref "$WORKFLOW_REF" \
  --raw-field "ref=$REF" --raw-field "scenario=$SCENARIO" \
  --raw-field "pr_number=$PR_NUMBER" --raw-field "geometry=$GEOMETRY" \
  --raw-field "gif=$GIF" --raw-field "request_id=$request_id"; then
  warn "could not dispatch the Pages workflow; ensure main is pushed and gh is authenticated"
fi

run_id=""
for _ in {1..30}; do
  runs="$(gh run list --repo "$REPO" --workflow "$WORKFLOW" --branch "$WORKFLOW_REF" --event workflow_dispatch --limit 30 --json databaseId,displayTitle 2>/dev/null || true)"
  run_id="$(RUNS_JSON="$runs" REQUEST_ID="$request_id" python3 - <<'PY'
import json, os
for run in json.loads(os.environ.get("RUNS_JSON") or "[]"):
    if os.environ["REQUEST_ID"] in run.get("displayTitle", ""):
        print(run["databaseId"]); break
PY
)"
  [[ -n "$run_id" ]] && break
  sleep 2
done
[[ -n "$run_id" ]] || warn "workflow dispatched but its run could not be located"
for _ in {1..180}; do
  state="$(gh run view "$run_id" --repo "$REPO" --json status,conclusion --jq '.status + " " + (.conclusion // "")' 2>/dev/null || true)"
  [[ "$state" == completed* ]] && break
  sleep 5
done
[[ "$state" == "completed success" ]] || warn "Pages workflow did not succeed ($state): https://github.com/$REPO/actions/runs/$run_id"

url="https://pr-${PR_NUMBER}.${PROJECT}.pages.dev"
fragment="$(mktemp "${TMPDIR:-/tmp}/amf-pages-fragment.XXXXXX")"
body="$(mktemp "${TMPDIR:-/tmp}/amf-pages-body.XXXXXX")"
updated="$(mktemp "${TMPDIR:-/tmp}/amf-pages-updated.XXXXXX")"
trap 'rm -f "$fragment" "$body" "$updated"' EXIT
printf '<!-- amf:screenshots:start -->\n### Visual proof\n\n[Open the private screenshot gallery for PR #%s](%s)\n\nRestricted to approved Cloudflare Access reviewers.\n<!-- amf:screenshots:end -->\n' "$PR_NUMBER" "$url" >"$fragment"
gh pr view "$PR_NUMBER" --repo "$REPO" --json body --jq .body >"$body" 2>/dev/null || warn "gallery is ready but the existing PR body could not be read: $url"
python3 "$SCRIPT_DIR/update_pr_body.py" --body-file "$body" --fragment-file "$fragment" --output-file "$updated" || warn "gallery is ready but markers could not be updated: $url"
UPDATED_BODY="$updated" python3 - <<'PY' | gh api "repos/$REPO/issues/$PR_NUMBER" --method PATCH --input - >/dev/null 2>&1 || warn "gallery is ready but the PR body update failed: $url"
import json, os
from pathlib import Path
print(json.dumps({"body": Path(os.environ["UPDATED_BODY"]).read_text()}))
PY
echo "gallery: $url"
echo "PR updated: #$PR_NUMBER"
