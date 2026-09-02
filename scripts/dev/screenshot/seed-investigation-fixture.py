#!/usr/bin/env python3
"""Inject a PR-Triage "Investigate" fixture into a scratch AMF DB.

Usage: seed-investigation-fixture.py <db_path> <repo_root> [pr_number]

Never invokes a model. Makes one read-only `gh pr view` call (from
<repo_root>) to learn the target PR's head SHA so the seeded
`pr_review_cache` row is a cache hit when PR Triage opens that PR by
number — which lets the pane show hand-written review comments with no
live comment fetch.

Seeds:
  * a `features` row (workdir = <repo_root>) so the demo project has a
    feature to select and `pr_investigations` rows can resolve a project id;
  * a `pr_review_cache` row: a normalized PrReview with two inline review
    comments, one of them a question;
  * a `pr_investigations` row: a *completed* read-only investigation of that
    question, with a markdown answer and one follow-up turn.
"""
import json
import subprocess
import sqlite3
import sys
from datetime import datetime, timedelta, timezone

db_path = sys.argv[1]
repo_root = sys.argv[2]
pr_number = int(sys.argv[3]) if len(sys.argv) > 3 else 434

OWNER, REPO = "eldridgerdev", "agent-mainframe"
QUESTION_ID = 700101
CHANGE_ID = 700102

head_sha = subprocess.run(
    ["gh", "pr", "view", str(pr_number), "--json", "headRefOid", "-q", ".headRefOid"],
    cwd=repo_root,
    check=True,
    capture_output=True,
    text=True,
).stdout.strip()

now = datetime.now(timezone.utc)
iso = now.isoformat()


def stamp(minutes_ago: int) -> str:
    return (now - timedelta(minutes=minutes_ago)).strftime("%Y-%m-%d %H:%M:%S")


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
    "diff_hunk": "@@ -905,6 +905,10 @@ impl PrComment {\n         if self.file_level {\n             return None;\n         }\n+        if let Some(line) = self.line\n+            && let Some(window) = window_github_hunk(hunk, line as usize, ...) {\n+            return Some(Cow::Owned(window));\n+        }",
    "body": "Does this handle the empty-changeset case, or can `window_github_hunk` end up calling `head()` on an empty slice when the comment has no anchor line?",
    "snippet": "Does this handle the empty-changeset case, or can window_github_hunk...",
    "in_reply_to": None,
    "thread_id": "PRRT_demo_q",
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
    "diff_hunk": "@@ -1350,3 +1350,7 @@\n+pub fn combined_fix_prompt(comments: &[&PrComment]) -> String {\n+    let mut out = String::from(\n+        \"Address these PR review comments. ...\",\n+    );",
    "body": "Rename `out` to `prompt` here for consistency with the other builders in this module.",
    "snippet": "Rename out to prompt here for consistency with the other builders...",
    "in_reply_to": None,
    "thread_id": "PRRT_demo_c",
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

ANSWER = (
    "**Verdict: the concern is already handled.**\n\n"
    "`window_github_hunk` (`src/app/pr_review.rs:911`) is only reached inside the "
    "`if let Some(line) = self.line` arm, and the file-level / unanchored case "
    "returns `None` earlier at line 906 (`if self.file_level { return None; }`) "
    "and again below via the `WHOLE_FILE_HUNK_LINES` backstop. `head()` is never "
    "called on an empty slice on this path.\n\n"
    "No code change is needed here. If the guard were removed, the failure mode "
    "would be an out-of-range slice index for a file-level comment — worth a "
    "regression test in `pr_review::tests`, not a fix in this function.\n\n"
    "_Files read: src/app/pr_review.rs (`prompt_hunk`, `window_github_hunk`, "
    "`WHOLE_FILE_HUNK_LINES`)._"
)

FOLLOW_UPS = [
    {
        "question": "Would an explicit debug_assert make that invariant clearer for the next reader?",
        "answer": (
            "A `debug_assert!(self.line.is_some())` at the top of the windowed "
            "branch would document it at the call site with no behaviour change. "
            "Low value though — the `if let Some(line)` right above it already "
            "makes the precondition local and obvious."
        ),
        "harness": "codex",
        "created_at": stamp(3),
    }
]

CONTEXT = (
    "Investigate this PR review comment. Someone triaging the pull request "
    "flagged it as a question to answer, not a change to make.\n\n"
    f"PR #{pr_number}: pr-review: fix G reopening an already-closed/merged PR\n\n"
    "PR description:\n(none)\n\n"
    "--- The review comment ---\n"
    "File: src/app/pr_review.rs:911\n"
    "Comment (@aria-reviews): Does this handle the empty-changeset case ...\n"
)

conn = sqlite3.connect(db_path)
project_id = conn.execute(
    "SELECT id FROM projects WHERE name = 'pr-triage-investigate-demo' LIMIT 1"
).fetchone()[0]

conn.execute(
    "INSERT OR REPLACE INTO features "
    "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
    "VALUES ('inv-demo-feature', ?, 'investigate demo', 'pr-triage-investigate', ?, 0, "
    "'amf-inv-demo', 'vibe', 'codex', 'stopped', 0, datetime('now'), datetime('now'))",
    (project_id, repo_root),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at) "
    "VALUES (?, ?, ?, datetime('now'))",
    (pr_number, head_sha, json.dumps(review)),
)

conn.execute(
    "INSERT OR REPLACE INTO pr_investigations "
    "(project_id, pr_number, comment_id, head_sha, harness, context_snapshot, answer, follow_ups, status, error, created_at, updated_at) "
    "VALUES (?, ?, ?, ?, 'codex', ?, ?, ?, 'complete', NULL, ?, ?)",
    (
        project_id,
        pr_number,
        QUESTION_ID,
        head_sha,
        CONTEXT,
        ANSWER,
        json.dumps(FOLLOW_UPS),
        stamp(6),
        stamp(3),
    ),
)

conn.commit()
conn.close()
print(
    f"seeded investigate fixture: pr={pr_number} head={head_sha[:12]} "
    f"question_comment={QUESTION_ID} project={project_id}"
)
