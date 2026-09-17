#!/usr/bin/env python3
"""Inject a PR-Triage "stale hunk" fixture into a scratch AMF DB.

Usage: seed-stale-hunk-fixture.py <db_path> <repo_root> <pr_number>

Fully offline once `gh pr view <pr_number>` resolves the PR's real, current
head SHA — that read-only call is the same one AMF itself makes when the
manual PR-number prompt (`#`) submits, so the seeded `pr_review_cache` row is
a cache hit the moment the scenario types that number in: the pane shows
hand-written review comments with no live comment fetch.

`pr_number` should be any currently-open PR in this repo (its real content is
irrelevant — only its number/head SHA are borrowed as a stable cache key, the
same trick `seed-investigation-fixture.py` and
`seed-batch-fix-cost-fixture.py` use for their own demo PRs).

Seeds:
  * a `features` row (workdir = <repo_root>) so the demo project has a
    feature to select before `G` opens PR Triage;
  * a `pr_review_cache` row: a normalized PrReview with three inline review
    comments — two anchored to the same file, one to a different file;
  * a `pr_comment_triage` row marking the first of the same-file comments
    `Fixing`, simulating "I already sent this one off to be fixed" before the
    second same-file comment is triaged.

Proves `PrComment::file_already_touched` / `fix_prompt_with_note`
(`src/app/pr_review/domain.rs`): opening the fix-confirm dialog (`f`) for the
second same-file comment shows the "already addressed earlier in this triage
session" staleness note; the third comment, on an untouched file, does not.
"""
import json
import subprocess
import sqlite3
import sys
from datetime import datetime, timezone

db_path = sys.argv[1]
repo_root = sys.argv[2]
pr_number = int(sys.argv[3])

SHARED_FILE = "src/app/pr_review/domain.rs"
OTHER_FILE = "src/app/pr_review/integration.rs"
TOUCHED_ID = 830101
UNTOUCHED_ID = 830102
OTHER_FILE_ID = 830103

pr_json = subprocess.run(
    ["gh", "pr", "view", str(pr_number), "--json", "headRefOid,url,headRefName"],
    check=True,
    capture_output=True,
    text=True,
).stdout
pr_meta = json.loads(pr_json)
head_sha = pr_meta["headRefOid"]
pr_url = pr_meta["url"]
owner, repo = pr_url.split("/")[3:5]
head_ref = pr_meta["headRefName"]

now = datetime.now(timezone.utc).isoformat()

touched_comment = {
    "id": TOUCHED_ID,
    "kind": "Inline",
    "author": "aria-reviews",
    "is_bot": False,
    "path": SHARED_FILE,
    "line": 815,
    "side": "RIGHT",
    "outdated": False,
    "file_level": False,
    "diff_hunk": "@@ -799,7 +799,7 @@ impl PrComment {\n     pub fn fix_prompt(&self) -> String {\n-        format!(\n-            \"Address this PR review comment.\\n{}\",\n-            self.fix_prompt_body()\n-        )\n+        self.fix_prompt_with_note(false)\n     }",
    "body": "Nit: this can just delegate to the new helper instead of duplicating the format! call.",
    "snippet": "Nit: this can just delegate to the new helper instead of duplicating...",
    "in_reply_to": None,
    "thread_id": "PRRT_stale_touched",
    "is_resolved": False,
    "triage": "Fixing",
    "local_note": None,
}

untouched_comment = dict(
    touched_comment,
    id=UNTOUCHED_ID,
    line=830,
    diff_hunk="@@ -937,6 +937,10 @@\n pub fn reply_posted_via_amf(reply: &PrComment) -> bool {\n     let body = reply.body.trim_end();\n     body.ends_with(AMF_ATTRIBUTION_FOOTER) || body.ends_with(AI_ATTRIBUTION_FOOTER)\n }\n+\n+pub(super) const STALE_HUNK_NOTE: &str = \"...\";",
    body="Should this note also mention the combined-batch case explicitly, or is that covered by the shared wording?",
    snippet="Should this note also mention the combined-batch case explicitly...",
    thread_id="PRRT_stale_untouched",
    triage="Untriaged",
)

other_file_comment = dict(
    touched_comment,
    id=OTHER_FILE_ID,
    path=OTHER_FILE,
    line=93,
    diff_hunk="@@ -90,7 +90,8 @@\n         let request = ReplyDraftRequest::new(comment.id, &state.review.pr.head_sha);\n-        let mut base = comment.fix_prompt();\n+        let touched = file_already_touched(comment, &state.review.comments);\n+        let mut base = comment.fix_prompt_with_note(touched);",
    body="Looks right — this reads the sibling triage state before the prompt is built, so it can't miss a fix that landed earlier in the same pane visit.",
    snippet="Looks right — this reads the sibling triage state before the prompt...",
    thread_id="PRRT_stale_other",
    triage="Untriaged",
)

review = {
    "pr": {
        "number": pr_number,
        "head_sha": head_sha,
        "url": pr_url,
        "owner": owner,
        "repo": repo,
        "head_ref": head_ref,
    },
    "comments": [touched_comment, untouched_comment, other_file_comment],
    "fetched_at": now,
}

conn = sqlite3.connect(db_path)
project_id = conn.execute(
    "SELECT id FROM projects WHERE name = 'pr-triage-stale-hunk-demo' LIMIT 1"
).fetchone()[0]

conn.execute(
    "INSERT OR REPLACE INTO features "
    "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
    "VALUES ('stale-hunk-demo-feature', ?, 'stale hunk demo', 'bug-pr-triage-comments-if-i-fix', ?, 0, "
    "'amf-stale-hunk-demo', 'vibe', 'claude', 'stopped', 0, datetime('now'), datetime('now'))",
    (project_id, repo_root),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at) "
    "VALUES (?, ?, ?, datetime('now'))",
    (pr_number, head_sha, json.dumps(review)),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_comment_triage "
    "(pr_number, comment_id, head_sha, state, note, updated_at, batch_id, batch_fix_cost) "
    "VALUES (?, ?, ?, 'fixing', NULL, datetime('now'), NULL, NULL)",
    (pr_number, TOUCHED_ID, head_sha),
)

conn.commit()
conn.close()
print(
    f"seeded stale-hunk fixture: pr={pr_number} head={head_sha[:12]} "
    f"touched={TOUCHED_ID} untouched={UNTOUCHED_ID} other_file={OTHER_FILE_ID} project={project_id}"
)
