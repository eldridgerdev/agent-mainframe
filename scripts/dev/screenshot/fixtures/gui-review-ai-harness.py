#!/usr/bin/python3
"""Unpaid CLI fixtures for the native Final Review AI screenshot scenario."""
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
repo = pathlib.Path.cwd()
assert (repo / ".git/amf-gui-ai-fixture").is_file(), "Refusing a non-fixture checkout"
prompt = sys.stdin.read()
if "You are an AI co-reviewer" in prompt:
    kind = "co_review"
    answer = "9|Cover negative totals before relying on this rounding rule.\n16|Consider making the currency and locale configurable."
elif "triage a full changeset" in prompt:
    kind = "overview"
    answer = "## Changeset overview\n\nInvoice totals now round to cents and format as US dollars.\n\n- Calculation and display changes live in `invoice.ts`.\n- A new smoke test covers the positive-total example.\n\n### Risk factors\n\nNegative totals and locale-specific formatting need tests."
elif "understand a code change during final review" in prompt:
    kind = "walkthrough"
    answer = "## Invoice rounding walkthrough\n\nThe change rounds invoice totals to cents before formatting them as US dollars.\n\n- `invoiceTotal` computes tax, then rounds the combined total.\n- `formatTotal` uses `Intl.NumberFormat` with the en-US locale and USD currency.\n- Check negative totals and boundary values around a half cent."
elif "Why does this round" in prompt:
    kind = "question"
    assert harness == "codex" and "read-only" in sys.argv
    assert "invoice.ts" in prompt and "Intl.NumberFormat" in prompt
    answer = "## Rounding and currency formatting\n\nRounding happens in `invoiceTotal`; `Intl.NumberFormat` only controls presentation.\n\nI inspected the invoice diff and positive-total test. Add tests for negative totals and half-cent boundaries before relying on the rounding rule. The selected USD formatter also fixes the locale to en-US."
else:
    raise SystemExit("Unsupported fixture prompt")
with pathlib.Path(os.environ["AMF_GUI_AI_CALLS"]).open("a") as calls:
    calls.write(json.dumps({"harness": harness, "kind": kind}) + "\n")
# Leave a real worker in flight long enough for the ordinary GUI poll to run.
time.sleep(1.4)
print(answer)
