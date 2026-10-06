#!/usr/bin/python3
"""Unpaid CLI fixtures for the native PR Triage screenshot scenario."""
import json
import os
import pathlib
import sys
import time

harness = pathlib.Path(sys.argv[0]).name
if "--version" in sys.argv:
    print(harness + " 2.1.289 (offline screenshot fixture)")
    raise SystemExit(0)
if "--help" in sys.argv:
    print("--sandbox --ephemeral --skip-git-repo-check --color --json --output-format --tools --safe-mode --permission-mode --no-session-persistence --pure")
    raise SystemExit(0)
assert (pathlib.Path.cwd() / ".git/amf-gui-pr-triage-fixture").is_file(), "Refusing a non-fixture checkout"
prompt = sys.stdin.read()
assert prompt.startswith("Investigate this PR review comment."), "Unsupported fixture prompt"
assert "You have read-only access to this repository." in prompt
if "--- Their follow-up question ---" in prompt:
    kind = "follow_up"
    answer = "Yes: `invoice.test.ts` only checks a positive total. Add `invoiceTotal({ subtotal: -19.99, taxRate: 0.0825 })` and assert `-21.64`; the current code returns `-21.63`."
else:
    kind = "investigation"
    assert "Does this round negative totals" in prompt
    answer = (
        "**Verdict: the concern is valid.**\n\n"
        "`invoiceTotal` in `invoice.ts` rounds with `Math.round(total * 100) / 100`. "
        "`Math.round` rounds halves toward positive infinity, so `-21.635` becomes `-21.63`, not `-21.64`.\n\n"
        "I checked `invoice.ts` and `invoice.test.ts`; no test covers a negative total.\n\n"
        "A fix would round the magnitude and restore the sign (or use a decimal library), plus a refund test."
    )
with pathlib.Path(os.environ["AMF_GUI_AI_CALLS"]).open("a") as calls:
    calls.write(json.dumps({"harness": harness, "kind": kind}) + "\n")
# Keep the worker in flight long enough for the GUI's ordinary poll to show it.
time.sleep(1.4)
print(answer)
