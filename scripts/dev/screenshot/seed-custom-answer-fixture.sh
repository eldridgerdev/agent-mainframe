#!/usr/bin/env bash
# Seed a throwaway project + feature for the plan-interview custom-answer
# screenshot. The project's amf.json opts out of the built-in question bank
# and adds one required select question, so `P` opens an interview whose only
# question is a choice question -- the surface the custom-answer box lives on.
#
# Uses the Codex harness because that is the one the screenshot CI installs;
# the scenario enables it in the isolated registry before calling this.
set -euo pipefail

AMF_BIN="${AMF_BIN:?AMF_BIN must point at the running capture binary}"

demo_repo="$(mktemp -d "${TMPDIR:-/tmp}/amf-custom-answer-demo.XXXXXX")"
git init -q "$demo_repo"
git -C "$demo_repo" config user.email screenshot@example.com
git -C "$demo_repo" config user.name "AMF Screenshot"

cat >"$demo_repo/amf.json" <<'JSON'
{
  "skip_builtin_questions": true,
  "plan_questions": [
    {
      "id": "surface",
      "text": "Where should the plan-preview summary appear?",
      "options": [
        "Dashboard sidebar",
        "A dedicated overlay",
        "The feature status bar"
      ],
      "optional": false
    }
  ]
}
JSON
cat >"$demo_repo/README.md" <<'MD'
# custom-answer-demo

Scratch repo for the plan-mode custom-answer screenshot.
MD
git -C "$demo_repo" add -A
git -C "$demo_repo" commit -qm "seed demo repo with a required select plan question"

project_seed="$demo_repo/.seed-project.json"
feature_seed="$demo_repo/.seed-feature.json"
printf '{"path":"%s","project_name":"custom-answer-demo","dry_run":false}\n' "$demo_repo" >"$project_seed"
cat >"$feature_seed" <<'JSON'
{
  "project_name": "custom-answer-demo",
  "branch": "plan-preview-summary",
  "agent": "codex",
  "mode": "vibe",
  "review": false,
  "plan_mode": false,
  "create_terminal": false,
  "use_worktree": false,
  "enable_chrome": false,
  "hook_choice": null,
  "dry_run": false
}
JSON

"$AMF_BIN" automation create-project --file "$project_seed"
"$AMF_BIN" automation create-feature --file "$feature_seed"
