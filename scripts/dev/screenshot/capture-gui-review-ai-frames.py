#!/usr/bin/env python3
"""Assert native Final Review states through WebKit IPC and capture its X11 window.

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
        f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');el.value={json.dumps(value)};el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()"""
    )
    wait(
        '!document.body.innerText.includes("Loading changes…") && !!document.querySelector(".diff-reader")'
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


def ready():
    wait('!document.body.innerText.includes("Updating review…")')


def choose_file(path):
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    ready()


def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def save(label):
    click(label)
    wait('!document.querySelector(".review-editor")')
    ready()


repo = pathlib.Path(sys.argv[4])
progress_path = repo / ".claude/final-review-progress.json"
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])


def calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def open_detail(summary):
    evaluate(f"Array.from(document.querySelectorAll('details')).find(d=>d.querySelector('summary')?.textContent.startsWith({json.dumps(summary)})).open=true")


def select_line(number):
    evaluate(f"document.querySelector('button[aria-label=\"Select line {number}\"]').click()")
    wait(f"document.querySelector('button[aria-label=\"Select line {number}\"]')?.getAttribute('aria-pressed')==='true'")


def approve_call():
    click("Continue AI call")
    wait('!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Continue AI call")')
    ready()


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    capture("001-review-ai-controls.png", "A stopped feature offers walkthroughs, a changeset overview, co-review and questions without starting a terminal agent.", ["Final Review · Round invoice totals", "Generate walkthrough", "AI co-review file", "Ask about file", "Changeset overview"], 'document.querySelectorAll(".diff-lines:not(.diff-split)").length>0')
    assert calls() == [], "Opening the review must not launch an AI call"

    # The short smoke-test diff makes the entire prompt preview readable.
    choose_file("invoice.test.ts")
    click("Generate walkthrough")
    wait('document.body.innerText.includes("Continue AI call")')
    click("View prompt")
    wait('document.body.innerText.includes("You are helping a reviewer understand")')
    capture("002-review-ai-prompt-preview.png", "Before generation, the GUI shows the real resolved prompt and explicit continue/cancel controls; no fixture call has run yet.", ["Headless AI call: Final Review: file walkthrough", "You are helping a reviewer understand", "File: invoice.test.ts", "Continue AI call", "Cancel AI call"], '!document.querySelector(".review-confirm pre").hidden')
    assert calls() == []
    click("Cancel AI call")
    wait('!document.body.innerText.includes("Continue AI call")')
    assert calls() == []

    choose_file("invoice.ts")
    click("Generate walkthrough")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("AI walkthrough") && !document.body.innerText.includes("Generating walkthrough for")')
    click("Changeset overview")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("AI changeset overview") && !document.body.innerText.includes("Generating changeset overview") && !document.body.innerText.includes("Changeset overview running")')
    open_detail("AI changeset overview")
    open_detail("AI walkthrough")
    capture("003-review-walkthrough-and-overview.png", "Completed fixture output reaches the GUI through Rust worker polling: a cached changeset risk summary and per-file walkthrough sit above the reviewed diff.", ["Risk factors", "Negative totals and locale-specific formatting need tests.", "Invoice rounding walkthrough", "Intl.NumberFormat"], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Generate walkthrough").disabled')
    assert [c["kind"] for c in calls()] == ["walkthrough", "overview"]
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')

    click("AI co-review file")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("Saved line comments (2)")')
    open_detail("Saved line comments")
    capture("004-co-review-drafts.png", "Co-review adds two explicitly unaccepted AI drafts. Each has accept/dismiss controls and the file remains undecided.", ["Cover negative totals before relying on this rounding rule.", "Consider making the currency and locale configurable.", "(AI draft)", "Accept AI draft", "Dismiss AI draft", "0 approved · 0 rejected"], 'document.querySelectorAll("button").length>0')
    progress = json.loads(progress_path.read_text())
    assert all(c["draft"] for c in progress["line_comments"]["invoice.ts"])
    assert "invoice.ts" not in progress["decisions"]

    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    select_line(9)
    click("Ask about selection")
    fill("Review question", "Why does this round before formatting?\nCheck negative totals and the selected formatter.")
    select("Answering harness", "codex")
    capture("005-local-review-question.png", "A multiline question is drafted locally for a selected source line, with Codex chosen from the four allowed answering harnesses. Editing does not launch a call.", ["Question about invoice.ts, line 9 – line 9", "Answering harness", "Ask review question", "The answering harness reads the repository"], 'document.querySelector(".review-editor textarea").value.includes("Check negative totals") && Array.from(document.querySelectorAll("select")).find(s=>s.value==="codex").options.length===4')
    assert len(calls()) == 3
    click("Ask review question")
    wait('document.body.innerText.includes("Continue AI call")')
    assert "Codex" in evaluate('document.querySelector(".review-confirm").innerText')
    click("Cancel AI call")
    wait('!document.body.innerText.includes("Continue AI call")')
    assert evaluate('document.querySelector(".review-editor textarea").value').startswith("Why does this round")
    assert len(calls()) == 3
    click("Ask review question")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("Review questions (1)") && !document.body.innerText.includes("Answering review question") && !document.querySelector(".review-editor")')
    open_detail("Review questions")
    capture("006-repository-aware-answer.png", "The Codex fixture answer arrives through the real question engine and appears in review history. Cancelling its first pre-call retained the unsent question; confirming the retry ran it exactly once.", ["Rounding and currency formatting", "Rounding happens in", "Add tests for negative totals and half-cent boundaries", "Question context"], 'document.querySelector(".review-note details")!==null')
    assert calls()[-1] == {"harness": "codex", "kind": "question"}
    assert len(calls()) == 4

    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    open_detail("Saved line comments")
    click("Accept AI draft")
    ready()
    wait('document.body.innerText.includes("1 rejected")')
    click("Dismiss AI draft")
    ready()
    wait('document.body.innerText.includes("Saved line comments (1)")')
    progress = json.loads(progress_path.read_text())
    kept = progress["line_comments"]["invoice.ts"]
    assert len(kept) == 1 and not kept[0]["draft"]
    assert "Reject" in progress["decisions"]["invoice.ts"]
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    open_detail("Saved line comments")
    capture("007-kept-finding-resumes.png", "Accepting one draft and dismissing the other leaves one human-confirmed finding and a rejection that survive pause/reopen in the shared TUI progress format.", ["1 rejected", "Saved line comments (1)", "Cover negative totals before relying on this rounding rule.", "Resolve thread"], '!document.body.innerText.includes("(AI draft)") && !document.body.innerText.includes("Consider making the currency") && !document.body.innerText.includes("Review questions")')
    assert len(calls()) == 4, "Reopening must not generate notes or restart a question"

    click("AI co-review file")
    wait('document.body.innerText.includes("Continue AI call")')
    with (repo / "invoice.ts").open("a") as source:
        source.write("\n// Externally changed after the preview opened.\n")
    click("Continue AI call")
    wait('!!document.querySelector("[role=alert]")')
    capture("008-stale-preview-refused.png", "An externally changed patch is refused again when the pending AI call is confirmed, before the fixture executes.", ["Review changes changed; refresh changes before running AI", "Cancel AI call"], allow_alert=True)
    assert len(calls()) == 4
    click("Cancel AI call")
    ready()
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
