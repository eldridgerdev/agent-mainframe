#!/usr/bin/env python3
"""Inject a PR-Triage fixture for the "optional Investigate context" proof.

Usage: seed-investigate-context-fixture.py <db_path> <repo_root> <pr_number>

Fully offline for the comment content — no model, no comment fetch. One real
read-only `gh pr view <pr_number> --json headRefOid` call resolves the head SHA
so the seeded `pr_review_cache` row is a cache hit the moment PR Triage opens
that PR: the current branch need not have its own PR, because the scenario
reaches the PR through the picker (`G` -> pick -> Enter), and
`GhCli::fetch_pr_by_number` returns exactly this SHA.

Seeds:
  * a `features` row (workdir = <repo_root>) so the demo project has a feature;
  * a `pr_review_cache` row: a normalized PrReview with two inline review
    comments, one of them a question (so the comment-action footer hints,
    including the new `e context` affordance, are shown).

No `pr_investigations` row: the feature under review is the *pre-run* optional
context box, not a finished investigation.
"""
import json
import subprocess
import sqlite3
import sys
from datetime import datetime, timezone

db_path = sys.argv[1]
repo_root = sys.argv[2]
pr_number = int(sys.argv[3])

OWNER, REPO = "eldridgerdev", "agent-mainframe"
QUESTION_ID = 710101
CHANGE_ID = 710102

head_sha = subprocess.run(
    ["gh", "pr", "view", str(pr_number), "--json", "headRefOid", "-q", ".headRefOid"],
    check=True,
    capture_output=True,
    text=True,
    cwd=repo_root,
).stdout.strip()

iso = datetime.now(timezone.utc).isoformat()

question = {
    "id": QUESTION_ID,
    "kind": "Inline",
    "author": "aria-reviews",
    "is_bot": False,
    "path": "src/app/pr_review.rs",
    "line": 911,
    "side": "RIGHT",
    "outdated": False,
    "file_level": False,
    "diff_hunk": (
        "@@ -905,6 +905,10 @@ impl PrComment {\n"
        "         if self.file_level {\n"
        "             return None;\n"
        "         }\n"
        "+        if let Some(line) = self.line\n"
        "+            && let Some(window) = window_github_hunk(hunk, line as usize, ...) {\n"
        "+            return Some(Cow::Owned(window));\n"
        "+        }"
    ),
    "body": (
        "Does this handle the empty-changeset case, or can `window_github_hunk` "
        "end up calling `head()` on an empty slice when the comment has no "
        "anchor line?"
    ),
    "snippet": "Does this handle the empty-changeset case, or can window_github_hunk...",
    "in_reply_to": None,
    "thread_id": "PRRT_ctx_q",
    "is_resolved": False,
    "triage": "Untriaged",
    "local_note": None,
}

change_request = {
    "id": CHANGE_ID,
    "kind": "Inline",
    "author": "aria-reviews",
    "is_bot": False,
    "path": "src/app/pr_review.rs",
    "line": 1353,
    "side": "RIGHT",
    "outdated": False,
    "file_level": False,
    "diff_hunk": (
        "@@ -1350,3 +1350,7 @@\n"
        "+pub fn combined_fix_prompt(comments: &[&PrComment]) -> String {\n"
        "+    let mut out = String::from(\n"
        "+        \"Address these PR review comments. ...\",\n"
        "+    );"
    ),
    "body": "Rename `out` to `prompt` here for consistency with the other builders in this module.",
    "snippet": "Rename out to prompt here for consistency with the other builders...",
    "in_reply_to": None,
    "thread_id": "PRRT_ctx_c",
    "is_resolved": False,
    "triage": "Untriaged",
    "local_note": None,
}

review = {
    "pr": {
        "number": pr_number,
        "head_sha": head_sha,
        "url": f"https://github.com/{OWNER}/{REPO}/pull/{pr_number}",
        "owner": OWNER,
        "repo": REPO,
        "head_ref": "",
    },
    "comments": [question, change_request],
    "fetched_at": iso,
}

conn = sqlite3.connect(db_path)
project_id = conn.execute(
    "SELECT id FROM projects WHERE name = 'investigate-context-demo' LIMIT 1"
).fetchone()[0]

conn.execute(
    "INSERT OR REPLACE INTO features "
    "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
    "VALUES ('ctx-demo-feature', ?, 'investigate context demo', 'pr-triage-investigate-optionally', ?, 0, "
    "'amf-ctx-demo', 'vibe', 'codex', 'stopped', 0, datetime('now'), datetime('now'))",
    (project_id, repo_root),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at) "
    "VALUES (?, ?, ?, datetime('now'))",
    (pr_number, head_sha, json.dumps(review)),
)

conn.commit()
conn.close()
print(
    f"seeded investigate-context fixture: pr={pr_number} head={head_sha[:12]} "
    f"question_comment={QUESTION_ID} project={project_id}"
)
