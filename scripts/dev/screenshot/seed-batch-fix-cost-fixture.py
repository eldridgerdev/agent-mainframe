#!/usr/bin/env python3
"""Inject a combined-batch fix-cost fixture into a scratch AMF DB.

Usage: seed-batch-cost.py <db_path> <pr_number> <head_sha>

Adds a pr_review_cache row with one review comment anchored to the same
file/line as the seeded AI-review finding, plus two pr_comment_triage rows
sharing a batch_id (one resolved, carrying the shared fix cost). This is what
App::ai_review_finding_fix_costs() correlates to render
"Fix cost (est.): $0.12 · combined (2)" on the finding.
"""
import json
import sqlite3
import sys
from datetime import datetime, timezone

db_path, pr_number, head_sha = sys.argv[1], int(sys.argv[2]), sys.argv[3]

FILE = "src/app/ai_review.rs"
LINE = 37
RESOLVED_ID = 900001
SIBLING_ID = 900002
BATCH_ID = "demo-batch-7f3a"
FIX_COST = "$0.12"

now = datetime.now(timezone.utc).isoformat()

comment = {
    "id": RESOLVED_ID,
    "kind": "Inline",
    "author": "a-reviewer",
    "is_bot": False,
    "path": FILE,
    "line": LINE,
    "side": "RIGHT",
    "outdated": False,
    "diff_hunk": None,
    "body": "The review post dialog must retain its AI attribution after the summary is edited.",
    "snippet": "retain AI attribution after the summary is edited",
    "in_reply_to": None,
    "thread_id": None,
    "is_resolved": True,
    "triage": "Done",
    "local_note": None,
}
sibling_comment = dict(
    comment,
    id=SIBLING_ID,
    path="src/app/pr_review.rs",
    line=3210,
    is_resolved=False,
    triage="Fixing",
    body="Guard the combined-batch dispatch against an empty selection.",
    snippet="guard the combined-batch dispatch",
)

review = {
    "pr": {
        "number": pr_number,
        "head_sha": head_sha,
        "url": f"https://github.com/eldridgerdev/agent-mainframe/pull/{pr_number}",
        "owner": "eldridgerdev",
        "repo": "agent-mainframe",
        "head_ref": "pr-triage-cost-to-fix",
    },
    "comments": [comment, sibling_comment],
    "fetched_at": now,
}

conn = sqlite3.connect(db_path)
conn.execute(
    "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at) "
    "VALUES (?, ?, ?, datetime('now'))",
    (pr_number, head_sha, json.dumps(review)),
)
for cid, state, batch_cost in (
    (RESOLVED_ID, "done", FIX_COST),
    (SIBLING_ID, "fixing", None),
):
    conn.execute(
        "INSERT OR REPLACE INTO pr_comment_triage "
        "(pr_number, comment_id, head_sha, state, note, updated_at, batch_id, batch_fix_cost) "
        "VALUES (?, ?, ?, ?, NULL, datetime('now'), ?, ?)",
        (pr_number, cid, head_sha, state, BATCH_ID, batch_cost),
    )
conn.commit()
conn.close()
print(f"seeded batch-cost fixture: pr={pr_number} comments={RESOLVED_ID},{SIBLING_ID} batch={BATCH_ID}")
