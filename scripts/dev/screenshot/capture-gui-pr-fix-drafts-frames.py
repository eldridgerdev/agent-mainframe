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
    wait(has_button("Round invoice totals"))
    click("Round invoice totals")
    wait(has_button("Codex 1"))
    click("Codex 1")
    wait('!!document.querySelector(".composer textarea")')
    evaluate("(()=>{const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,'Existing unsent reminder.');el.dispatchEvent(new Event('input',{bubbles:true}));})()")
    click("PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    capture("001-fix-targets.png", "An actionable PR comment offers existing feature agents as fix destinations, including stopped sessions. Preparing a fix starts no agent and writes nothing to GitHub.", ["PR #12", "Fix agent", "Prepare fix…", "Does this round negative totals"])
    assert ai_calls() == [] and gh_writes() == []
    click("Investigate…")
    select("Investigating harness", "codex")
    click("Preview AI call")
    wait(has_button("Continue AI call"))
    click("Continue AI call")
    wait('document.body.innerText.includes("Verdict: the concern is valid.")')
    select("Fix agent", "shot-codex")
    click("Prepare fix…")
    wait(has_button("Open in agent composer"))
    wait('document.querySelector(".review-editor textarea")?.value.includes("read-only investigation")')
    evaluate('(()=>{const el=document.querySelector(".review-editor textarea");el.scrollTop=el.scrollHeight;})()')
    capture("002-fix-preview.png", "The editable fix prompt uses the TUI's comment prompt and includes the completed read-only investigation as a starting point to verify. The confirmation names Codex 1 and explains that the handoff is an unsent composer draft.", ["Prepare an unsent fix prompt for Codex 1", "Fix prompt", "Open in agent composer", "Cancel fix draft"], 'document.querySelector(".review-editor textarea").value.includes("Verdict: the concern is valid.")')
    original = evaluate('document.querySelector(".review-editor textarea").value')
    fill("Fix prompt", original + "\nAdd regression coverage for negative invoice totals.")
    click("Cancel fix draft")
    wait('!!document.querySelector(".review-editor [role=alertdialog]")')
    capture("003-discard-protection.png", "Cancelling an edited fix requires an explicit discard. Keeping the edit preserves the operator's text and leaves both the agent and GitHub untouched.", ["Discard your edited fix prompt?", "Keep editing", "Discard fix prompt"], 'document.querySelector(".review-editor textarea").value.endsWith("Add regression coverage for negative invoice totals.")')
    click("Keep editing")
    (state / "head.txt").write_text("feed" + "1" * 36 + "\n")
    click("Open in agent composer")
    wait('!!document.querySelector("[role=alert]")')
    capture("004-stale-head-refusal.png", "The PR head moved after preparation. Confirmation rereads GitHub and refuses the handoff while keeping the edited prompt. No fix starts and no reply is posted.", ["The PR has new commits; cancel the fix draft and refresh", "Fix prompt", "Open in agent composer"], 'document.querySelector(".review-editor textarea").value.endsWith("Add regression coverage for negative invoice totals.")', allow_alert=True)
    (state / "head.txt").write_text("c0ffee" + "0" * 34 + "\n")
    click("Open in agent composer")
    wait('!document.querySelector(".pr-detail") && document.querySelector(".composer textarea")?.value.includes("Add regression coverage for negative invoice totals.")')
    evaluate('document.querySelector(".composer textarea").scrollTop=0')
    capture("005-unsent-composer-handoff.png", "Confirmation closes PR Triage and selects Codex 1. Its composer retains the existing reminder and appends the edited fix exactly once. Codex remains stopped; sending and starting remain explicit user actions.", ["Codex 1", "Stopped"], 'document.querySelector(".composer textarea").value.startsWith("Existing unsent reminder.") && document.querySelector(".composer textarea").value.split("Add regression coverage for negative invoice totals.").length===2')
    evaluate('(()=>{const el=document.querySelector(".composer textarea");el.scrollTop=el.scrollHeight;})()')
    capture("006-edited-composer-draft.png", "The same unsent composer draft is scrolled to its end to show the retained investigation and the operator's added regression instruction. Send remains disabled until the stopped session is started.", ["Codex 1", "Start this session to send your draft."], 'document.querySelector(".composer textarea").value.endsWith("Add regression coverage for negative invoice totals.")')
    assert ai_calls() == [{"harness": "codex", "kind": "investigation"}]
    assert gh_writes() == []
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
