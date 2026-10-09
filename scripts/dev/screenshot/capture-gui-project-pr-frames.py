#!/usr/bin/env python3
"""Assert project PR buttons and pickers through native WebKit IPC.

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
            str(pathlib.Path(os.environ.get("AMF_PROJECT_PR_PROOF_SCOPE", str(out))) / name),
        ],
        check=True,
    )
    digest = hashlib.sha256((pathlib.Path(os.environ.get("AMF_PROJECT_PR_PROOF_SCOPE", str(out))) / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)



def has_button(text):
    return f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)})'


state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])


def ai_calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def gh_writes():
    return (state / "writes.jsonl").read_text().splitlines()


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait('document.querySelector("h1")?.textContent==="Invoice API"')
    wait('document.body.innerText.includes("No features yet") && '+has_button("PR Triage")+' && '+has_button("PR Review"))
    capture("project-pr-buttons.png", "Project selection offers PR Triage and PR Review even with no features.", ["Invoice API", "No features yet", "PR Triage", "PR Review"])
    click("PR Triage")
    wait('document.querySelectorAll(".pr-entry").length===2')
    capture("project-pr-triage-picker.png", "PR Triage opens the repository PR picker directly, with no current-branch PR lookup.", ["PR Triage · Invoice API", "#12", "Round invoice totals to cents", "#11", "Add Euro formatting", "Open by number"], '!document.body.innerText.includes("This branch")')
    # Repository-wide entry must not resolve the currently checked-out branch.
    gh_calls=[json.loads(line) for line in (state / "calls.jsonl").read_text().splitlines()]
    assert not any(call[:2]==["pr", "view"] for call in gh_calls), gh_calls
    evaluate('document.querySelector("[role=dialog] button[aria-label=Close]").click()')
    wait('!document.querySelector("[role=dialog]")')
    click("PR Review")
    wait('document.querySelectorAll(".pr-entry").length===2')
    capture("project-pr-review-picker.png", "PR Review opens the project's list of open PRs without opening the checkout's current PR.", ["PR Review · Invoice API", "#12", "Round invoice totals to cents", "#11", "Add Euro formatting", "Refresh pull requests", "Draft PR"], '!document.querySelector("[role=alert]")')
    gh_calls=[json.loads(line) for line in (state / "calls.jsonl").read_text().splitlines()]
    assert not any(call[:2]==["pr", "view"] for call in gh_calls), gh_calls
    assert ai_calls()==[] and gh_writes()==[]
    evaluate('document.querySelector("[role=dialog] button[aria-label=Close]").click()')
    wait('!document.querySelector("[role=dialog]")')
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
