#!/usr/bin/env python3
"""Assert native PR Triage states through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
import pathlib
import re
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
        if evaluate(expression):
            return
        time.sleep(0.1)
    (out / "failed-state.txt").write_text(evaluate("document.body.innerText"))
    raise AssertionError(expression)


def click(text):
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
    )


def select(label, value):
    evaluate(
        f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');const set=Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set;set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()"""
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


def capture(name, note, expects, expression=None, allow_alert=False):
    wait('document.fonts.status==="loaded"')
    wait('!document.body.innerText.includes("Loading changes…")')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_alert:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
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



def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent.startsWith({json.dumps(label)}));const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def has_button(text):
    return f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)})'


def idle():
    """No command in flight: the panel's buttons are enabled again."""
    wait('!!document.querySelector(".pr-detail, .pr-picker") && !Array.from(document.querySelectorAll(".modal .btn")).every(b=>b.disabled)')


def choose_comment(author_prefix):
    evaluate(f'Array.from(document.querySelectorAll(".pr-comment")).find(b=>b.textContent.startsWith({json.dumps(author_prefix)})).click()')


state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])


def ai_calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def gh_writes():
    return (state / "writes.jsonl").read_text().splitlines()


def detail():
    return evaluate('document.querySelector(".pr-detail")?.innerText ?? ""')


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && ' + has_button("PR Triage"))
    click("PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    capture("001-pr-list.png", "PR Triage opens on the repository's open pull requests. The branch's own PR is marked, drafts are labelled, and a PR can also be opened by number. Nothing has been fetched beyond the list.", ["PR Triage · Round invoice totals", "#12", "Round invoice totals to cents", "This branch", "Yours", "Draft", "Include closed and merged", "Open by number"], '!document.body.innerText.includes("Initial invoice API")')
    assert ai_calls() == [] and gh_writes() == []

    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    idle()
    capture("002-review-thread.png", "The shared fetch normalizes inline comments, a changes-requested review summary and a bot comment. The selected thread shows its windowed diff context and the AMF follow-up reply collated beneath it, with resolve, reply, investigate and local triage actions.", ["PR #12", "3 open of 5", "aria-reviews", "invoice.ts:9", "Does this round negative totals", "Looking into it before changing anything.", "via AMF", "Resolve thread…", "Investigate…", "Reply: not needed", "coverage-bot[bot]", "resolved"], 'document.querySelectorAll(".pr-comment").length===4 && document.querySelector(".pr-hunk").innerText.includes("Math.round(total * 100)")')
    assert ai_calls() == [] and gh_writes() == []

    click("Investigate…")
    select("Investigating harness", "codex")
    fill("What do you suspect?", "Refunds (negative totals) may round the wrong way.")
    click("Preview AI call")
    wait(has_button("Continue AI call"))
    click("View prompt")
    wait('!!document.querySelector(".review-confirm pre")')
    idle()
    capture("003-investigation-precall.png", "An investigation never starts from the button. The pre-call notice names the read-only call and Codex, and shows the exact prompt built from the PR, the comment and the operator's hypothesis. No harness has run yet.", ["Headless AI call: PR Triage: read-only investigation · Codex", "Investigate this PR review comment.", "PR #12: Round invoice totals to cents", "Refunds (negative totals) may round the wrong way.", "Continue AI call", "Cancel AI call"])
    assert ai_calls() == [], "The pre-call notice must not run the harness"

    click("Continue AI call")
    wait('document.body.innerText.includes("Investigating comment read-only")')
    wait('document.body.innerText.includes("Verdict: the concern is valid.")')
    idle()
    capture("004-investigation-answer.png", "After explicit confirmation the offline Codex fixture answers once through the shared read-only investigation engine. The finding is stored with the comment and offers a follow-up, dismissal or a reply with the findings.", ["Investigation (read-only)", "complete", "codex", "Verdict: the concern is valid.", "Ask a follow-up…", "Dismiss investigation", "Reply with findings"])
    assert ai_calls() == [{"harness": "codex", "kind": "investigation"}]

    click("Reply with findings")
    wait('!!document.querySelector(".pr-detail textarea")')
    click("Review reply…")
    wait('!!document.querySelector(".pr-posted")')
    idle()
    capture("005-reply-confirmation.png", "Replying is two explicit steps. The editable draft is seeded from the investigation; reviewing it shows where it will post and the exact body, including AMF's attribution footer. Nothing has been sent to GitHub.", ["Reply in the inline review thread on PR #12", "From a read-only investigation of this comment", "\u2014 posted via AMF", "Post reply to GitHub", "This writes to GitHub as your gh account"])
    assert gh_writes() == []

    (state / "head.txt").write_text("feed" + "1" * 36 + "\n")
    click("Post reply to GitHub")
    wait('!!document.querySelector("[role=alert]")')
    idle()
    capture("006-stale-head-refused.png", "The PR gained a commit after the comments loaded. Confirming re-reads the PR head first and refuses: nothing is posted and the unsent draft stays in the editor.", ["has new commits since these comments were loaded", "nothing was posted and your draft is kept", "Review reply…", "Discard reply"], 'document.querySelector(".pr-detail textarea").value.includes("From a read-only investigation")', allow_alert=True)
    assert gh_writes() == [], "A stale reply must not reach gh"

    click("Discard reply")
    wait('!document.querySelector(".pr-detail textarea")')
    idle()
    evaluate('Array.from(document.querySelectorAll("label")).find(l=>l.textContent.includes("Hide resolved")).querySelector("input").click()')
    wait('document.body.innerText.includes("1 hidden")')
    idle()
    click("Resolve thread…")
    wait('document.body.innerText.includes("Resolve the review thread on PR #12")')
    idle()
    capture("007-resolve-confirmation.png", "Resolving a thread is also a confirmed GitHub write. Resolved threads are filtered out here (one hidden); confirming would re-check the thread's state on GitHub before writing. The capture cancels, so its offline gh recorded no write.", ["Resolve the review thread on PR #12", "Resolve thread on GitHub", "Hide resolved", "(1 hidden)"], 'document.querySelectorAll(".pr-comment").length===3', allow_alert=True)
    click("Cancel")
    wait('!document.body.innerText.includes("Resolve thread on GitHub")')
    assert gh_writes() == []
    assert ai_calls() == [{"harness": "codex", "kind": "investigation"}]
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
