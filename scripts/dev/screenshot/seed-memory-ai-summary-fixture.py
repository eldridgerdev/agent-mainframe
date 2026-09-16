#!/usr/bin/env python3
"""Inject a PR-Triage "add to memory" fixture into a scratch AMF DB.

Usage: seed-memory-ai-summary-fixture.py <db_path> <repo_root> [pr_number]

Fully offline — no model, no network. Uses `git rev-parse HEAD` in <repo_root>
for the PR head SHA so the seeded `pr_review_cache` row is a cache hit the
moment PR Triage resolves that branch's PR: the pane then shows a
hand-written review comment with no live comment fetch.

`pr_number` defaults to the PR opened from this feature's own branch (the one
this fixture lives in); `G` resolves straight to it, exactly like
`seed-investigation-fixture.py`'s approach. Pass an explicit number only if
this branch's real PR number differs from the default.

Also seeds `store_meta.available_harnesses` with two harnesses so the
memory-add dialog's `s` action (summarize with AI) opens its per-use
harness picker instead of skipping straight past it.

Seeds:
  * a `features` row (workdir = <repo_root>) so the demo project has a feature;
  * a `pr_review_cache` row: a normalized PrReview with one inline review
    comment worth turning into a review-memory finding;
  * `store_meta.available_harnesses` = ["claude", "codex"].
"""
import json
import subprocess
import sqlite3
import sys
from datetime import datetime, timezone

db_path = sys.argv[1]
repo_root = sys.argv[2]
pr_number = int(sys.argv[3]) if len(sys.argv) > 3 else 636

OWNER, REPO = "eldridgerdev", "agent-mainframe"
COMMENT_ID = 700201

head_sha = subprocess.run(
    ["git", "-C", repo_root, "rev-parse", "HEAD"],
    check=True,
    capture_output=True,
    text=True,
).stdout.strip()

iso = datetime.now(timezone.utc).isoformat()

comment = {
    "id": COMMENT_ID,
    "kind": "Inline",
    "author": "aria-reviews",
    "is_bot": False,
    "path": "src/app/pr_review/memory.rs",
    "line": 214,
    "side": "RIGHT",
    "outdated": False,
    "file_level": False,
    "diff_hunk": "@@ -208,6 +208,10 @@ pub(super) fn run_review_memory_bootstrap(\n     for entry in &entries {\n         let comments = GhCli::pr_review_comments(&workdir, entry.number).unwrap_or_default();\n+        let reviews = GhCli::pr_reviews(&workdir, entry.number).unwrap_or_default();\n+        let text = bootstrap_pr_text(&comments, &reviews);\n+        if !text.is_empty() {\n+            pr_bodies.push((entry.number, entry.title.clone(), text));",
    "body": "This loop fetches every PR's comments serially. On a repo with a long history and `--depth all`, that's one `gh` round-trip per PR in sequence — worth batching or at least noting the cost in the running-screen copy so it doesn't look hung.",
    "snippet": "This loop fetches every PR's comments serially...",
    "in_reply_to": None,
    "thread_id": "PRRT_memo_demo",
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
    "comments": [comment],
    "fetched_at": iso,
}

conn = sqlite3.connect(db_path)
project_id = conn.execute(
    "SELECT id FROM projects WHERE name = 'memory-ai-summary-demo' LIMIT 1"
).fetchone()[0]

conn.execute(
    "INSERT OR REPLACE INTO features "
    "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
    "VALUES ('memo-demo-feature', ?, 'memory ai summary demo', 'memory-ai-summary-demo', ?, 0, "
    "'amf-memo-demo', 'vibe', 'codex', 'stopped', 0, datetime('now'), datetime('now'))",
    (project_id, repo_root),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at) "
    "VALUES (?, ?, ?, datetime('now'))",
    (pr_number, head_sha, json.dumps(review)),
)

conn.execute(
    "INSERT OR REPLACE INTO store_meta (key, value) VALUES ('available_harnesses', ?)",
    (json.dumps(["claude", "codex"]),),
)

conn.commit()
conn.close()
print(
    f"seeded memory-ai-summary fixture: pr={pr_number} head={head_sha[:12]} "
    f"comment={COMMENT_ID} project={project_id}"
)
