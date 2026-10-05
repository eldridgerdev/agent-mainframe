#!/usr/bin/env python3
"""Assert native question-to-comment drafting states through WebKit IPC and capture its X11 window.

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


def select_line(number, extend=False):
    evaluate(f"document.querySelector('button[aria-label=\"Select line {number}\"]').dispatchEvent(new MouseEvent('click',{{bubbles:true,shiftKey:{str(extend).lower()}}}))")
    wait(f"document.querySelector('button[aria-label=\"Select line {number}\"]')?.getAttribute('aria-pressed')==='true'")


def approve_call():
    click("Continue AI call")
    wait('!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Continue AI call")')
    ready()



def focus(selector):
    evaluate(f"document.querySelector({json.dumps(selector)}).scrollIntoView({{block:'center'}})")
    time.sleep(0.2)


original = (repo / "invoice.ts").read_text()
existing = "Existing thread: keep calculation separate from display."
replacement = "  const total = invoice.subtotal + tax;\n  return Math.round(total * 100) / 100;"
edited = "Add negative-total and half-cent boundary tests before approving this rounding rule."
existing_general = "Existing review: verify refund totals."


def progress():
    return json.loads(progress_path.read_text())


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    select_line(8)
    select_line(9, extend=True)
    click("Comment on selection")
    fill("Line comment", existing)
    select("Severity", "nit")
    save("Save comment")
    open_detail("Saved line comments")
    click("Edit suggestion")
    fill("Suggested replacement", replacement)
    save("Save suggestion")
    click("Overall feedback")
    fill("Overall feedback draft", existing_general)
    save("Save overall feedback")
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    select_line(9)
    click("Ask about selection")
    fill("Review question", "Why does this round before formatting? Check negative totals.")
    select("Answering harness", "codex")
    click("Ask review question")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("Review questions (1)") && !document.querySelector(".review-editor")')
    open_detail("Review questions")
    focus(".review-note:has(button)")
    capture("001-answer-to-comment-actions.png", "An answered Codex question offers the new inline-comment and overall-feedback drafting actions.", ["Rounding and currency formatting", "Draft inline comment", "Draft overall feedback"], 'Array.from(document.querySelectorAll("button")).filter(b=>b.textContent.trim().startsWith("Draft ")).every(b=>!b.disabled)')
    assert calls() == [{"harness": "codex", "kind": "question"}]

    click("Draft inline comment")
    wait('document.body.innerText.includes("Headless AI call: Review: draft comment from answer")')
    focus(".review-confirm")
    capture("002-draft-call-confirmation.png", "Drafting is a separate Codex call requiring explicit prompt preview/confirmation; it has not run yet.", ["Review: draft comment from answer", "Codex", "View prompt", "Continue AI call", "Cancel AI call"])
    click("View prompt")
    wait('document.body.innerText.includes("Write a concise, constructive review comment")')
    assert evaluate('document.querySelector(".review-confirm pre").innerText.includes("Rounding and currency formatting")')
    click("Cancel AI call")
    ready()
    assert len(calls()) == 1
    click("Draft inline comment")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("AI comment draft") && !document.body.innerText.includes("Answering or drafting review question")')
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    fill("AI comment draft", edited)
    focus(".review-editor")
    capture("003-editable-generated-draft.png", "Generated feedback is edited locally before transfer; opening the comment editor checks context and requires another explicit save.", ["AI comment draft", "Open comment editor", "Discard AI draft", "Save the comment explicitly"], 'document.querySelector(".review-editor textarea").value===' + json.dumps(edited))
    assert len(calls()) == 2 and calls()[-1] == {"harness": "codex", "kind": "comment_draft"}
    assert progress()["line_comments"]["invoice.ts"][0]["text"] == existing

    click("Pause review")
    wait('document.body.innerText.includes("Discard unsaved edits and continue?")')
    focus(".review-confirm")
    capture("004-unsaved-draft-protection.png", "Pausing with an unsaved generated draft requires explicit discard; Keep editing retains the edited feedback.", ["Discard unsaved edits and continue?", "Discard and continue", "Keep editing", "AI comment draft"])
    click("Keep editing")
    ready()
    assert evaluate('document.querySelector(".review-editor textarea").value') == edited
    before = progress_path.read_bytes()
    click("Open comment editor")
    wait('document.body.innerText.includes("Line comment") && !document.body.innerText.includes("AI comment draft") && !document.body.innerText.includes("Checking comment draft context")')
    focus(".review-editor")
    capture("005-transferred-inline-editor.png", "Transfer appends the edited draft to the existing thread, preserves its nit severity and full line 8–9 span, and opens an unsaved editor without another AI call.", ["Line comment", "Severity", "Save comment", "Anchored to line 8 – line 9"], 'document.querySelector(".review-editor textarea").value===' + json.dumps(existing + "\n\n" + edited) + ' && document.querySelector(".review-editor select").value==="nit"')
    assert progress_path.read_bytes() == before
    assert len(calls()) == 2
    save("Save comment")
    select("Review layout", "split")
    open_detail("Saved line comments")
    focus(".diff-content")
    capture("006-saved-inline-feedback.png", "Explicit Save keeps the combined prose on the original range and retains its code suggestion beside the split diff.", ["line 8 – line 9 [nit]", existing, edited, "Edit suggestion", "Resolve thread"], '!!document.querySelector(".diff-split")')
    kept = progress()["line_comments"]["invoice.ts"][0]
    assert kept["text"] == existing + "\n\n" + edited
    assert kept["severity"] == "nit" and kept["suggestion"] == replacement
    assert kept["start"]["new_line"] == 8 and kept["location"]["new_line"] == 9
    assert (repo / "invoice.ts").read_text() == original

    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    open_detail("Review questions")
    click("Draft overall feedback")
    wait('document.body.innerText.includes("Continue AI call")')
    approve_call()
    wait('document.body.innerText.includes("AI comment draft") && !document.body.innerText.includes("Answering or drafting review question")')
    overall = "Release note: cover refunds and half-cent boundaries before merging."
    fill("AI comment draft", overall)
    before = progress_path.read_bytes()
    click("Open comment editor")
    wait('document.body.innerText.includes("Overall feedback draft") && !document.body.innerText.includes("AI comment draft")')
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    focus(".review-editor")
    capture("007-transferred-overall-feedback.png", "The same answer can draft overall feedback, appending to existing review feedback and waiting for explicit Save.", ["Overall feedback draft", "Save overall feedback", "Cancel edit"], 'document.querySelector(".review-editor textarea").value===' + json.dumps(existing_general + "\n\n" + overall))
    assert progress_path.read_bytes() == before
    assert len(calls()) == 3
    save("Save overall feedback")
    assert progress()["general_feedback"] == existing_general + "\n\n" + overall
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    open_detail("Saved line comments")
    evaluate('document.querySelector(".modal-body").scrollTop=0')
    capture("008-feedback-resumes.png", "Saved overall feedback and the combined inline thread survive pause/reopen through the TUI-compatible review progress file; no AI calls restart.", [existing_general, overall, "Saved line comments (1)", "line 8 – line 9 [nit]", edited], '!document.body.innerText.includes("Review questions") && !document.querySelector(".review-editor")')
    assert len(calls()) == 3
    assert (repo / "invoice.ts").read_text() == original
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
