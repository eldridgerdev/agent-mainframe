#!/usr/bin/python3
"""Offline `gh` for the native PR Triage screenshot scenario.

Serves one canned pull request (#12) with review comments, a review summary,
a bot conversation comment and two review threads. Every call is logged. Any
GitHub write (REST POST or a GraphQL mutation) is logged separately and
refused, so the capture can prove it never wrote. The PR head is read from
a state file, letting the frame script move it to exercise stale refusals.
"""
import json
import os
import pathlib
import sys

state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
args = sys.argv[1:]
with (state / "calls.jsonl").open("a") as calls:
    calls.write(json.dumps(args) + "\n")

OWNER, REPO, NUMBER = "demo-org", "invoice-api", 12
URL = f"https://github.com/{OWNER}/{REPO}/pull/{NUMBER}"
HEAD = (state / "head.txt").read_text().strip()


def out(value):
    print(json.dumps(value))
    raise SystemExit(0)


def refuse_write():
    with (state / "writes.jsonl").open("a") as writes:
        writes.write(json.dumps(args) + "\n")
    print("offline fixture: GitHub writes are disabled in this capture", file=sys.stderr)
    raise SystemExit(1)


user = lambda login, kind="User": {"login": login, "type": kind}
HUNK = (
    "@@ -5,6 +5,7 @@ export type Invoice = {\n"
    " \n"
    " export function invoiceTotal(invoice: Invoice): number {\n"
    "   const tax = invoice.subtotal * invoice.taxRate;\n"
    "-  return invoice.subtotal + tax;\n"
    "+  const total = invoice.subtotal + tax;\n"
    "+  return Math.round(total * 100) / 100;\n"
    " }"
)
REVIEW_COMMENTS = [
    {
        "id": 7001, "path": "invoice.ts", "line": 9, "original_line": 9, "side": "RIGHT",
        "diff_hunk": HUNK, "user": user("aria-reviews"), "pull_request_review_id": 8001,
        "body": "Does this round negative totals the way finance expects? `Math.round(-2.345 * 100)` rounds toward positive infinity, so refunds may be off by a cent.",
    },
    {
        "id": 7003, "path": "invoice.ts", "line": 9, "original_line": 9, "side": "RIGHT",
        "diff_hunk": HUNK, "user": user("demo-reviewer"), "in_reply_to_id": 7001,
        "body": "Looking into it before changing anything.\n\n— posted via AMF",
    },
    {
        "id": 7002, "path": "invoice.ts", "line": 16, "original_line": 16, "side": "RIGHT",
        "diff_hunk": "@@ -12,3 +13,6 @@ export function formatTotal(total: number): string {\n-  return total.toFixed(2);\n+  return new Intl.NumberFormat(\"en-US\", {\n+    style: \"currency\",\n+    currency: \"USD\",\n+  }).format(total);",
        "user": user("aria-reviews"), "pull_request_review_id": 8001,
        "body": "Consider taking the currency from the invoice instead of hard-coding USD.",
    },
]
REVIEWS = [{"id": 8001, "state": "CHANGES_REQUESTED", "user": user("aria-reviews"),
            "body": "Rounding needs a test for negative totals before this merges."}]
ISSUE_COMMENTS = [{"id": 9001, "user": user("coverage-bot[bot]", "Bot"),
                   "body": "Coverage: 92.4% (+0.3%). No uncovered lines in `invoice.ts`."}]
THREADS = [
    {"id": "PRRT_rounding", "isResolved": False, "comments": {"nodes": [{"databaseId": 7001}, {"databaseId": 7003}]}},
    {"id": "PRRT_currency", "isResolved": True, "comments": {"nodes": [{"databaseId": 7002}]}},
]
PRS = [
    {"number": 12, "title": "Round invoice totals to cents", "author": {"login": "demo-reviewer"},
     "headRefName": "round-invoice-totals", "updatedAt": "2026-10-05T18:00:00Z", "isDraft": False, "state": "OPEN"},
    {"number": 11, "title": "Add Euro formatting", "author": {"login": "aria-reviews"},
     "headRefName": "euro-format", "updatedAt": "2026-10-04T09:30:00Z", "isDraft": True, "state": "OPEN"},
    {"number": 8, "title": "Initial invoice API", "author": {"login": "demo-reviewer"},
     "headRefName": "initial-api", "updatedAt": "2026-09-30T12:00:00Z", "isDraft": False, "state": "MERGED"},
]

if args == ["--version"]:
    print("gh version 2.62.0 (offline screenshot fixture)")
    raise SystemExit(0)
if args[:2] == ["auth", "status"]:
    raise SystemExit(0)
if "--method" in args or any("mutation" in a for a in args):
    refuse_write()
if args[:2] == ["pr", "view"]:
    fields = args[args.index("--json") + 1]
    if fields == "title,body,files":
        out({"title": "Round invoice totals to cents",
             "body": "Totals were formatted without rounding. This rounds to cents and formats as USD.",
             "files": [{"path": "invoice.ts", "additions": 9, "deletions": 2},
                       {"path": "invoice.test.ts", "additions": 4, "deletions": 0}]})
    out({"number": NUMBER, "headRefOid": HEAD, "url": URL, "state": "OPEN", "headRefName": "round-invoice-totals"})
if args[:2] == ["pr", "list"]:
    everything = args[args.index("--state") + 1] == "all"
    out([p for p in PRS if everything or p["state"] == "OPEN"])
if args[:2] == ["api", "user"]:
    print("demo-reviewer")
    raise SystemExit(0)
if args[:3] == ["api", "--paginate", "--slurp"]:
    endpoint = args[3]
    if endpoint.endswith(f"/pulls/{NUMBER}/comments"):
        out([REVIEW_COMMENTS])
    if endpoint.endswith(f"/pulls/{NUMBER}/reviews"):
        out([REVIEWS])
    if endpoint.endswith(f"/issues/{NUMBER}/comments"):
        out([ISSUE_COMMENTS])
if args[:2] == ["api", "graphql"] and any("reviewThreads" in a for a in args):
    out({"data": {"repository": {"pullRequest": {"reviewThreads": {
        "pageInfo": {"hasNextPage": False, "endCursor": None}, "nodes": THREADS}}}}})
print(f"offline fixture: unsupported gh call {args}", file=sys.stderr)
raise SystemExit(1)
