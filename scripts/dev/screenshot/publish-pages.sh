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
SUMMARY=""
ALLOWED_ACTOR="eldridgerdev"
GEOMETRY="120x40"
GIF=false
SEED=""
SEED_FEATURE=""
CONFIG_FILE=""
STRICT=0

usage() {
  cat <<EOF
Usage: $(basename "$0") --pr <number> --scenario <file> [options]

Dispatch the isolated capture workflow (run from main; only its capture job
checks out --ref), then download this run's rendered frames and deploy the
private Pages gallery from this machine. The Cloudflare credentials never enter
CI: set CLOUDFLARE_API_TOKEN (the owner keeps it in ~/.secrets/cf-amf-pages.env,
sourced by the 'amf-publish-screenshots' shell wrapper) and CLOUDFLARE_ACCOUNT_ID,
and have wrangler on PATH or npx available. 'wrangler login' also works but is
unreliable here; prefer the token. Failures warn by default.

  --pr <number>          Pull request to update (required)
  --scenario <file>      Repository-relative scenario (required)
  --summary <text>       What the evidence flow proves (required)
  --ref <branch>         Pushed ref to capture (default: current branch)
  --pages-project <name> Pages project (default: CF_PAGES_PROJECT variable)
  --geometry <WxH>       Capture geometry (default: 120x40)
  --gif                  Include an animated GIF
  --seed <file>          Repository-relative project seed payload
  --seed-feature <file>  Repository-relative feature seed payload
  --config <file>        Repository-relative AMF config payload
  --strict               Return nonzero on failure
EOF
}

warn() { echo "warning: $*" >&2; [[ "$STRICT" -eq 1 ]] && exit 1; exit 0; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --pr) PR_NUMBER="${2:-}"; shift 2 ;;
    --scenario) SCENARIO="${2:-}"; shift 2 ;;
    --summary) SUMMARY="${2:-}"; shift 2 ;;
    --ref) REF="${2:-}"; shift 2 ;;
    --pages-project) PROJECT="${2:-}"; shift 2 ;;
    --geometry) GEOMETRY="${2:-}"; shift 2 ;;
    --gif) GIF=true; shift ;;
    --seed) SEED="${2:-}"; shift 2 ;;
    --seed-feature) SEED_FEATURE="${2:-}"; shift 2 ;;
    --config) CONFIG_FILE="${2:-}"; shift 2 ;;
    --strict) STRICT=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) warn "unknown argument: $1" ;;
  esac
done

[[ "$PR_NUMBER" =~ ^[0-9]+$ ]] || warn "--pr must be numeric"
[[ "$GEOMETRY" =~ ^[0-9]+x[0-9]+$ ]] || warn "--geometry must look like WxH"
[[ -n "$SCENARIO" ]] || warn "--scenario is required"
[[ -n "$SUMMARY" && ${#SUMMARY} -le 600 ]] || warn "--summary is required and must be 600 characters or fewer"
command -v gh >/dev/null 2>&1 || warn "GitHub CLI (gh) is not installed"
if command -v wrangler >/dev/null 2>&1; then
  WRANGLER=(wrangler)
elif command -v npx >/dev/null 2>&1; then
  WRANGLER=(npx --yes wrangler@4)
else
  warn "need wrangler on PATH or npx available to deploy Cloudflare Pages"
fi
# Auth is an explicit API token (preferred) or a completed `wrangler login`.
# Check now so missing auth doesn't burn a full CI capture that can't publish.
# The owner keeps the token in ~/.secrets/cf-amf-pages.env; the
# `amf-publish-screenshots` shell wrapper sources it before calling this script.
if [[ -z "${CLOUDFLARE_API_TOKEN:-}" ]] && ! "${WRANGLER[@]}" whoami >/dev/null 2>&1; then
  warn "no Cloudflare auth: export CLOUDFLARE_API_TOKEN (see ~/.secrets/cf-amf-pages.env / the amf-publish-screenshots wrapper)"
fi
# Not a secret, but needed so the deploy picks the right account non-interactively.
[[ -n "${CLOUDFLARE_ACCOUNT_ID:-}" ]] || warn "CLOUDFLARE_ACCOUNT_ID is not set"
cd "$REPO_ROOT" || warn "cannot enter repository root"
REPO="$(gh repo view --json nameWithOwner --jq .nameWithOwner 2>/dev/null)" || warn "could not resolve GitHub repository"
ACTOR="$(gh api user --jq .login 2>/dev/null || true)"
[[ "$ACTOR" == "$ALLOWED_ACTOR" ]] || warn "authenticated gh user '$ACTOR' is not allowed to publish Pages previews"
REF="${REF:-$(git branch --show-current)}"
[[ -n "$REF" ]] || warn "detached HEAD; pass --ref with a pushed branch"
[[ -n "$PROJECT" ]] || PROJECT="$(gh variable get CF_PAGES_PROJECT --repo "$REPO" 2>/dev/null || true)"
[[ "$PROJECT" =~ ^[a-z0-9-]+$ ]] || warn "set CF_PAGES_PROJECT or pass --pages-project"

scenario_abs="$(realpath "$SCENARIO" 2>/dev/null)" || warn "scenario must exist"
case "$scenario_abs" in "$REPO_ROOT"/*) SCENARIO="${scenario_abs#"$REPO_ROOT/"}" ;; *) warn "scenario must be inside repository" ;; esac
optional_inputs=()
for pair in "seed:$SEED" "seed_feature:$SEED_FEATURE" "config:$CONFIG_FILE"; do
  key="${pair%%:*}"; value="${pair#*:}"
  if [[ -n "$value" ]]; then
    absolute="$(realpath "$value" 2>/dev/null)" || warn "$key must exist"
    case "$absolute" in "$REPO_ROOT"/*) optional_inputs+=(--raw-field "$key=${absolute#"$REPO_ROOT/"}") ;; *) warn "$key must be inside repository" ;; esac
  fi
done

request_id="$(date +%Y%m%d-%H%M%S)-$$"
if ! gh workflow run "$WORKFLOW" --repo "$REPO" --ref "$WORKFLOW_REF" \
  --raw-field "ref=$REF" --raw-field "scenario=$SCENARIO" \
  --raw-field "pr_number=$PR_NUMBER" --raw-field "geometry=$GEOMETRY" \
  --raw-field "gif=$GIF" --raw-field "request_id=$request_id" "${optional_inputs[@]}"; then
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
[[ "$state" == "completed success" ]] || warn "capture workflow did not succeed ($state): https://github.com/$REPO/actions/runs/$run_id"

# Privileged half runs here, not in CI. The capture job checked out --ref; this
# machine only ever touches the rendered frames it uploaded.
workdir="$(mktemp -d "${TMPDIR:-/tmp}/amf-pages.XXXXXX")"
fragment="$(mktemp "${TMPDIR:-/tmp}/amf-pages-fragment.XXXXXX")"
body="$(mktemp "${TMPDIR:-/tmp}/amf-pages-body.XXXXXX")"
updated="$(mktemp "${TMPDIR:-/tmp}/amf-pages-updated.XXXXXX")"
trap 'rm -rf "$workdir"; rm -f "$fragment" "$body" "$updated"' EXIT

artifact_name="amf-screenshot-pr-${PR_NUMBER}-${run_id}"
gh run download "$run_id" --repo "$REPO" --name "$artifact_name" --dir "$workdir/capture" \
  || warn "capture succeeded but its artifact could not be downloaded: https://github.com/$REPO/actions/runs/$run_id"

python3 "$SCRIPT_DIR/build_static_gallery.py" \
  --input-dir "$workdir/capture" --output-dir "$workdir/pages" \
  --pr-number "$PR_NUMBER" --summary "$SUMMARY" \
  || warn "downloaded the capture but the restricted gallery could not be built"

# --commit-dirty: the gallery lives in a temp dir, but wrangler still warns
# about this script's own repo checkout being dirty. It is irrelevant here.
"${WRANGLER[@]}" pages deploy "$workdir/pages" \
  --project-name="$PROJECT" --branch="pr-${PR_NUMBER}" --commit-dirty=true \
  || warn "gallery built but 'wrangler pages deploy' failed; check CLOUDFLARE_API_TOKEN (~/.secrets/cf-amf-pages.env) / CLOUDFLARE_ACCOUNT_ID"

url="https://pr-${PR_NUMBER}.${PROJECT}.pages.dev"
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
