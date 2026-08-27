#!/usr/bin/env bash
# Seed a completed AI-review UI fixture for the screenshot harness. It never
# invokes a model and never contacts GitHub.
set -euo pipefail

AMF_BIN="${AMF_BIN:?AMF_BIN must point at the running capture binary}"
PR_NUMBER="${AMF_SCREENSHOT_PR_NUMBER:?AMF_SCREENSHOT_PR_NUMBER is required}"
WORKDIR="${GITHUB_WORKSPACE:-$(git rev-parse --show-toplevel)}"
REPOSITORY="${GITHUB_REPOSITORY:-}"

[[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] || { echo "AMF_SCREENSHOT_PR_NUMBER must be a positive number" >&2; exit 1; }
[[ "$REPOSITORY" =~ ^[[:alnum:]_.-]+/[[:alnum:]_.-]+$ ]] || { echo "GITHUB_REPOSITORY must be owner/name for the AI-review screenshot fixture" >&2; exit 1; }

HEAD_SHA="$(git -C "$WORKDIR" rev-parse HEAD)"
fixture="$(mktemp "${TMPDIR:-/tmp}/amf-ai-review-fixture.XXXXXX.json")"
trap 'rm -f "$fixture"' EXIT

# All interpolated values above are constrained to GitHub/revision identifiers.
printf '%s\n' \
  '{' \
  "  \"pr_number\": $PR_NUMBER," \
  "  \"head_sha\": \"$HEAD_SHA\"," \
  '  "summary": "The completed deterministic review identifies the attribution and disclosure changes ready for a human to publish.",' \
  '  "findings": [' \
  '    {' \
  '      "path": "src/app/ai_review.rs",' \
  '      "line": 37,' \
  '      "side": "New",' \
  '      "body": "The review post dialog must retain its AI attribution after the summary is edited.",' \
  '      "diff_hunk": "@@ -34,6 +34,11 @@\\n fn ensure_ai_review_attribution(body: &str) -> String {\\n+    // Restore the disclosure before publishing.\\n }",' \
  '      "skipped": false,' \
  '      "published": false' \
  '    }' \
  '  ],' \
  '  "open": true,' \
  "  \"workdir\": \"$WORKDIR\"," \
  "  \"repository\": \"$REPOSITORY\"," \
  '  "head_ref": "screenshot-fixture"' \
  '}' >"$fixture"

"$AMF_BIN" automation seed-ai-review --file "$fixture"
