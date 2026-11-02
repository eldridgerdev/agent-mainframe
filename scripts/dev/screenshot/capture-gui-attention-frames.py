#!/usr/bin/env python3
"""Assert native GUI states through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import pathlib
import re
import subprocess
import sqlite3
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
    raise AssertionError(f"Timed out: {expression}; body={evaluate('document.body.innerText')}")


def click(text):
    # Listing dormancy loads Rust state asynchronously. Its Settings button
    # is present but disabled until that load completes on slower runners.
    wait(f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled)')
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



def press(js_predicate):
    wait(f'Array.from(document.querySelectorAll("button")).some(b=>({js_predicate}) && !b.disabled)')
    evaluate(f'Array.from(document.querySelectorAll("button")).find(b=>({js_predicate})).click()')


scratch = pathlib.Path(sys.argv[4]).parent.parent.parent
stop_file = scratch / "receipts/.claude/notifications/stop-1.json"
try:
    window.configure(x=0, y=0, width=1280, height=850)
    x.sync()
    wait('document.body.innerText.includes("Round invoice totals")')
    wait('!!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.includes("Needs attention") && b.textContent.includes("4"))')
    # The edit-review fixture file also triggers the existing Supervised edits
    # auto-popup (a surface this feature leaves untouched). Close it first so the
    # new list is the subject of the frame.
    wait('!!document.querySelector(\'[role=dialog][aria-label^="Supervised edits"]\')')
    evaluate('Array.from(document.querySelectorAll(\'[role=dialog][aria-label^="Supervised edits"] button\')).find(b=>b.textContent.trim()==="Close").click()')
    wait('!document.querySelector(\'[role=dialog][aria-label^="Supervised edits"]\')')
    press('b.textContent.includes("Needs attention")')
    wait('document.body.innerText.includes("Waiting for input")')
    capture(
        "001-attention-list.png",
        "Four on-disk requests with reason, age and message; the unowned one explains why it cannot be opened, the edit review explains why it cannot be dismissed, and the stopped feature's leftover wait is counted but hidden. The limit note states that Question/Completed labels exist only in a running TUI.",
        ["Needs attention", "Waiting for input", "Turn ended", "Edit waiting for review", "could not be matched", "Supervised edits", "only a running TUI", "stopped features"],
        'document.querySelectorAll(".attention-row").length===4 && Array.from(document.querySelectorAll("button")).find(b=>(b.getAttribute("aria-label")||"").startsWith("Dismiss Add receipt export: Edit")).disabled',
    )
    assert stop_file.exists()
    press('(b.getAttribute("aria-label")||"")==="Dismiss Add receipt export: Turn ended"')
    wait('document.querySelectorAll(".attention-row").length===3')
    assert not stop_file.exists(), "dismiss must delete exactly the notification file"
    assert (stop_file.parent / "edit-1.json").exists(), "the edit review must be untouched"
    assert (stop_file.parent.parent.parent.parent / "invoice-api/.claude/notifications/ask-1.json").exists()
    capture("002-attention-dismissed.png", "Dismissing the Turn-ended request deleted only its notification file (the TUI's x); the other three rows remain.", ["Needs attention", "Waiting for input", "Edit waiting for review"], '!document.body.innerText.includes("Turn ended")')
    press('(b.getAttribute("aria-label")||"")==="Open session for Round invoice totals"')
    wait('!document.querySelector(\'[role=dialog][aria-label="Needs attention"]\') && !!document.querySelector(".composer textarea")')
    capture("003-attention-jump.png", "Open session selected the Round invoice totals agent session; nothing was started or answered.", ["Round invoice totals"], '!!document.querySelector(".composer textarea")')
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: needs-attention list, dismiss and jump verified", flush=True)
finally:
    ws.close()
    x.close()
