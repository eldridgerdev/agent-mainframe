#!/usr/bin/env python3
"""Offline `gh` for the sidebar-parity capture: answers only the dashboard
PR sweep's two read-only GraphQL queries and refuses everything else.

Every invocation is appended to $AMF_GUI_GH_CALLS so the capture can prove
that no other GitHub command (and no write) ran.
"""

import json
import os
import re
import sys

args = sys.argv[1:]
with open(os.environ["AMF_GUI_GH_CALLS"], "a") as log:
    log.write(json.dumps(args) + "\n")

if args[:2] != ["api", "graphql"]:
    sys.stderr.write("gui-sidebar-gh fixture: only `gh api graphql` reads are offline\n")
    sys.exit(1)

fields = {}
for flag, value in zip(args, args[1:]):
    if flag in ("-f", "-F") and "=" in value:
        key, _, rest = value.partition("=")
        fields[key] = rest
query = fields.get("query", "")
if fields.get("owner") != "acme" or fields.get("repo") != "invoice-api":
    sys.stderr.write("gui-sidebar-gh fixture: unknown repository\n")
    sys.exit(1)

if "pullRequests(states:OPEN" in query:
    print(json.dumps({"data": {"repository": {"pullRequests": {
        "pageInfo": {"hasNextPage": False, "endCursor": None},
        "nodes": [{
            "number": 321,
            "headRefName": "round-totals",
            "headRefOid": "0123456789abcdef0123456789abcdef01234567",
            "reviewThreads": {"nodes": [
                {"isResolved": False}, {"isResolved": False}, {"isResolved": True},
            ]},
        }],
    }}}}))
    sys.exit(0)

terminal = {
    "release-prep": {"number": 12, "state": "MERGED", "mergedAt": "2026-10-01T12:00:00Z", "closedAt": "2026-10-01T12:00:00Z"},
    "closed-experiment": {"number": 7, "state": "CLOSED", "mergedAt": None, "closedAt": "2026-09-28T09:00:00Z"},
}
if "states:[MERGED,CLOSED]" in query:
    repository = {}
    for alias in re.findall(r"(a\d+):pullRequests", query):
        branch = fields.get("b" + alias[1:], "")
        node = terminal.get(branch)
        repository[alias] = {"nodes": [node] if node else []}
    print(json.dumps({"data": {"repository": repository}}))
    sys.exit(0)

sys.stderr.write("gui-sidebar-gh fixture: unexpected query\n")
sys.exit(1)
