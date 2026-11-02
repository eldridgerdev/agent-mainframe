#!/usr/bin/env python3
"""Assert native GUI issue browser states through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import pathlib
import re
import sqlite3
import subprocess
import sys
import time
import urllib.request

import gi
import websocket
from Xlib import display

gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gdk, GdkX11

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])

for attempt in range(100):
    try:
        with urllib.request.urlopen(f"http://{sys.argv[3]}/", timeout=2) as response:
            inspector_page = response.read().decode()
        inspector_path = re.search(r"/socket/[^\']+/WebPage", inspector_page).group(0)
        break
    except (OSError, AttributeError):
        time.sleep(0.1)
else:
    raise RuntimeError("The native WebKit inspector did not become ready")
ws = websocket.create_connection(
    f"ws://{sys.argv[3]}{inspector_path}", timeout=15, suppress_origin=True
)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def evaluate(expression):
    global seq
    seq += 1
    request = {
        "id": seq,
        "method": "Runtime.evaluate",
        "params": {"expression": expression, "returnByValue": True},
    }
    ws.send(
        json.dumps(
            {
                "id": seq + 10000,
                "method": "Target.sendMessageToTarget",
                "params": {"targetId": target, "message": json.dumps(request)},
            }
        )
    )
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                assert not message["result"].get("wasThrown"), message
                return message["result"]["result"].get("value")


def wait(expression):
    for _ in range(100):
        if evaluate(f"!!({expression})"):
            return
        time.sleep(0.1)
    raise AssertionError(f"{expression}: {evaluate('document.body.innerText')}")


def click(text):
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
    )


x = display.Display()
atom = x.intern_atom("_NET_WM_PID")
windows = []


def visit(w):
    try:
        p = w.get_full_property(atom, 0)
        g = w.get_geometry()
        if p and int(p.value[0]) == pid and g.width > 500:
            windows.append(w)
        for child in w.query_tree().children:
            visit(child)
    except Exception:
        pass


for attempt in range(100):
    visit(x.screen().root)
    if windows:
        break
    time.sleep(0.1)
assert windows, "The isolated GUI has no X11 window"
window = max(windows, key=lambda w: w.get_geometry().width * w.get_geometry().height)
notes = []
captured_frames = set()


def capture(name, note, expects, expression=None, allow_error=False):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_error:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(f"!!({expression})"), expression
    time.sleep(0.25)
    g = window.get_geometry()
    subprocess.run(
        [
            "/usr/bin/python3",
            "-c",
            """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
            str(window.id),
            str(g.width),
            str(g.height),
            str(out / name),
        ],
        check=True,
    )
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)


dbpath = pathlib.Path(sys.argv[4])
gh_state = pathlib.Path(sys.argv[6])
repo = pathlib.Path(sys.argv[7])

browser = "document.querySelector('[role=dialog][aria-label=\"GitHub issues\"]')"
fixer = "document.querySelector('[role=dialog][aria-label=\"Fix GitHub issue\"]')"


def set_mode(mode):
    (gh_state / "mode.txt").write_text(mode + "\n")


def fill(selector, value):
    evaluate(
        "(()=>{const el=document.querySelector(%s);const proto=Object.getPrototypeOf(el);"
        "Object.getOwnPropertyDescriptor(proto,'value').set.call(el,%s);"
        "el.dispatchEvent(new Event(%s,{bubbles:true}));return true})()"
        % (json.dumps(selector), json.dumps(value), json.dumps("change" if selector.startswith("select") or "select" in selector else "input"))
    )


def feature_rows():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT name, branch, status, issue_source FROM features").fetchall()


def issue_buttons():
    return evaluate(f"{browser} ? Array.from({browser}.querySelectorAll('button[aria-label^=\"Open issue\"]')).length : -1")


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    wait('Array.from(document.querySelectorAll("button, a, div, span")).some(b=>b.textContent.trim()==="Billing API")')
    evaluate('Array.from(document.querySelectorAll("button, a, div, span")).filter(b=>b.textContent.trim()==="Billing API").pop().click()')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Issues")')
    click("Issues")
    wait(f"!!{browser} && {browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length>0")
    assert evaluate(f"{browser}.innerText.includes('Pull request masquerading')") is False
    assert issue_buttons() == 24, issue_buttons()  # 25 rows on page one minus the pull request
    capture("001-issue-list.png",
            "The Issues button on a project opens its open GitHub issues from the offline gh fixture. The pull request in the response is hidden, labels and linked-feature badges show on rows, and the footer states the browser is read-only.",
            ["GitHub issues", "github.com/acme/billing-api", "Retry failed invoice webhooks", "Round refund totals to cents",
             "Read-only: AMF never writes to GitHub from here.", "Page 1"],
            f"!{browser}.innerText.includes('Pull request masquerading')")

    fill("input[aria-label='Search issues']", "cent")
    wait(f"{browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length===2")
    capture("002-search.png",
            "Searching this page for 'cent' narrows it locally to the two matching issues; no additional GitHub request is made.",
            ["Round refund totals to cents", "Tax rate rounding differs in the PDF"],
            f"!{browser}.innerText.includes('Retry failed invoice webhooks')")
    fill("input[aria-label='Search issues']", "")
    fill("select[aria-label='Filter by label']", "docs")
    wait(f"{browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length===1")
    capture("003-label-filter.png",
            "Filtering by the docs label leaves the single documentation issue.",
            ["Document the CSV export", "good first issue"],
            f"!{browser}.innerText.includes('Retry failed invoice webhooks')")
    fill("select[aria-label='Filter by label']", "")
    click("Next")
    wait(f"{browser}.innerText.includes('Page 2') && {browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length===4")
    capture("004-next-page.png",
            "Next loads the second GitHub page, four further issues, and Next is no longer offered.",
            ["Page 2", "Housekeeping task"],
            f"Array.from({browser}.querySelectorAll('button')).find(b=>b.textContent.trim()==='Next').disabled")
    click("Previous")
    wait(f"{browser}.innerText.includes('Page 1')")

    evaluate(f"{browser}.querySelector('button[aria-label^=\"Open issue #101\"]').click()")
    wait(f"!!{fixer} && {fixer}.querySelector('textarea[aria-label=\"Agent prompt\"]')")
    wait("!Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Create feature').disabled")
    assert feature_rows() == []
    capture("005-preview.png",
            "Previewing issue #101 shows its text as plain data, a suggested feature name and branch, and the editable agent prompt built from the TUI's labeled-section prompt. 'Start the agent now' is off and nothing has been created.",
            ["Fix issue #101", "Retry failed invoice webhooks", "Feature name", "Agent prompt", "Start the agent now",
             "Nothing is posted to GitHub."],
            f"{fixer}.querySelector('textarea').value.includes('Fix GitHub issue #101') && {fixer}.querySelectorAll('input.mono')[0].value==='fix-issue-101' && {fixer}.querySelectorAll('input.mono')[1].value==='issue/101-retry-failed-invoice-webhooks'")

    # The issue changes on GitHub after the preview.
    changed = json.loads((gh_state / "issues.json").read_text())
    for issue in changed:
        if issue["number"] == 101:
            issue["body"] = "REVISED: also retry deliveries that return HTTP 500."
            issue["updated_at"] = "2026-11-01T08:00:00Z"
    (gh_state / "issues.json").write_text(json.dumps(changed))
    click("Create feature")
    wait("document.body.innerText.includes('changed on GitHub since you previewed it')")
    assert feature_rows() == []
    capture("006-changed-refused.png",
            "The issue was edited on GitHub after the preview. Creation is refused with the reason and no feature or worktree exists; Reload issue is offered.",
            ["changed on GitHub since you previewed it", "Reload issue", "Retry failed invoice webhooks"],
            f"!!{fixer}", allow_error=True)
    click("Reload issue")
    wait(f"{fixer}.innerText.includes('REVISED: also retry deliveries')")
    wait("!Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Create feature').disabled")
    capture("007-reloaded.png",
            "Reloading re-reads the issue: the revised description and updated prompt are shown, bound to the new content, and confirmation is possible again.",
            ["REVISED: also retry deliveries", "Create feature"],
            f"{fixer}.querySelector('textarea').value.includes('REVISED: also retry')")

    click("Create feature")
    wait("document.body.innerText.includes('Created')  || " + "!" + fixer)
    for _ in range(100):
        if feature_rows():
            break
        time.sleep(0.1)
    rows = feature_rows()
    assert len(rows) == 1, rows
    name, branch, status, source = rows[0]
    source = json.loads(source)
    assert (name, branch) == ("fix-issue-101", "issue/101-retry-failed-invoice-webhooks"), rows
    assert status == "stopped", status
    assert (source["owner"], source["repository"], source["number"]) == ("acme", "billing-api", 101), source
    assert source.get("comment_opt_out") is True, source
    worktree = repo / ".worktrees"
    assert any(worktree.iterdir()), "no worktree created"
    wait(f"!{fixer} && document.body.innerText.includes('fix-issue-101')")
    capture("008-created-stopped.png",
            "Confirming creates the fix-issue-101 feature on its own branch and worktree, bound to acme/billing-api#101, stopped: no agent or tmux session was started and nothing was posted to GitHub.",
            ["fix-issue-101", "issue/101-retry-failed-invoice-webhooks"],
            "!document.querySelector('[role=dialog]')", allow_error=True)

    evaluate('Array.from(document.querySelectorAll("button, a, div, span")).filter(b=>b.textContent.trim()==="Billing API").pop().click()')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Issues")')
    click("Issues")
    wait(f"!!{browser} && {browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length>0")
    wait(f"{browser}.innerText.includes('Feature exists: fix-issue-101')")
    capture("009-linked-feature.png",
            "Back in the list, issue #101 shows that a feature already fixes it.",
            ["Feature exists: fix-issue-101"], f"!!{browser}")
    evaluate(f"{browser}.querySelector('button[aria-label^=\"Open issue #101\"]').click()")
    wait(f"!!{fixer} && {fixer}.innerText.includes('This issue already has a feature')")
    assert evaluate("Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Create feature').disabled===true")
    capture("010-duplicate-needs-confirm.png",
            "Opening the same issue again warns that a feature exists and keeps Create feature disabled until 'Create another feature for this issue' is switched on.",
            ["This issue already has a feature: fix-issue-101", "Create another feature for this issue"], f"!!{fixer}", allow_error=True)
    click("Back to issues")
    wait(f"!!{browser}")

    set_mode("offline")
    click("Refresh")
    wait(f"{browser}.innerText.includes('GitHub could not be reached')")
    capture("011-offline.png",
            "With the fixture gh offline, the list states the reason (GitHub cannot be reached) and offers Retry instead of showing an empty list.",
            ["GitHub could not be reached", "Retry"], f"!!{browser}", allow_error=True)
    set_mode("unauth")
    click("Retry")
    wait(f"{browser}.innerText.includes('not signed in to GitHub')")
    capture("012-unauthenticated.png",
            "When gh is not authenticated, the browser says so and names the fix (gh auth login in a terminal).",
            ["not signed in to GitHub", "gh auth login"], f"!!{browser}", allow_error=True)
    set_mode("ratelimit")
    click("Retry")
    wait(f"{browser}.innerText.includes('rate limit')")
    capture("013-rate-limited.png",
            "A GitHub rate limit is reported as such, with advice to wait and retry.",
            ["rate limit was reached"], f"!!{browser}", allow_error=True)
    set_mode("ok")
    click("Retry")
    wait(f"{browser}.querySelectorAll('button[aria-label^=\"Open issue\"]').length>0")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: list, search, filter, paging, preview, stale refusal, stopped creation, duplicate guard and failure reasons verified", flush=True)
finally:
    ws.close()
    x.close()
