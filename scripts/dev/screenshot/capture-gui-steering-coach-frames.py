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
    time.sleep(1.0)
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



repo = pathlib.Path(sys.argv[4]).parent.parent.parent / "demo-api"


def set_textarea(value):
    evaluate(f"""(()=>{{const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")
    wait(f"document.querySelector('.composer textarea')?.value==={json.dumps(value)}")


def nothing_sent():
    for filename in ["claude-received.json", "codex-received.json"]:
        assert json.loads((repo / filename).read_text()) == [], f"{filename} received a prompt"


thin = "Please update invoiceTotal to round consistently at cent boundaries."
fuller = thin + " Touch only src/invoice.py. Do not change the public API. Verify with `pytest`."
try:
    window.configure(x=0, y=0, width=1180, height=980)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("New feature")
    wait('!!document.querySelector("[role=dialog][aria-label=\\"New feature\\"]")')
    wait('!!document.querySelector("[role=dialog] input[type=checkbox]")')
    box = '''Array.from(document.querySelectorAll("[role=dialog] label")).find(l=>l.textContent.includes("Steering coach (experimental)")).querySelector("input")'''
    assert evaluate(f"!{box}.checked"), "the coach must be off by default"
    evaluate(f"{box}.click()")
    wait(f"{box}.checked")
    capture("001-creation-checkbox.png", "The New feature form offers 'Steering coach (experimental)', off by default; ticking it only changes where the new feature lands after creation.", ["New feature", "Steering coach (experimental)", "get feedback on your first message"], f"{box}.checked", allow_alert=True)  # a runner without Claude shows a harness alert here
    click("Cancel")
    wait('!document.querySelector("[role=dialog][aria-label=\\"New feature\\"]")')

    click("Round invoice totals")
    wait('!!document.querySelector(".composer textarea")')
    set_textarea(thin)
    click("Steering coach")
    wait('!!document.querySelector("[aria-label=\\"Steering coach\\"][data-stale=false]") && document.body.innerText.includes("Score:")')
    nothing_sent()
    capture("002-advisory-panel.png", "Checklist for the unsent draft in the composer: only some constraints are covered, with the TUI's explanations and tips. Nothing was sent.", ["Steering Coach", "Advisory only. Your draft stays unsent.", "Constraint checklist", "File scope", "Validation commands", "How to strengthen it", "Send prompt"], f"document.querySelector('.composer textarea').value==={json.dumps(thin)}")

    set_textarea(fuller)
    wait('!!document.querySelector("[aria-label=\\"Steering coach\\"][data-stale=true]")')
    nothing_sent()
    capture("003-stale-after-edit.png", "Editing the draft marks the checklist stale and dims it until it is re-checked; the displayed result no longer claims to describe the edited text.", ["The draft changed since this check", "Re-check draft"], f"document.querySelector('.composer textarea').value==={json.dumps(fuller)}")

    click("Re-check draft")
    wait('!!document.querySelector("[aria-label=\\"Steering coach\\"][data-stale=false]") && !document.body.innerText.includes("The draft changed since")')
    nothing_sent()
    capture("004-rechecked.png", "Re-checking binds the checklist to the edited text (higher score); the harness still has received nothing.", ["Steering Coach", "Score:", "Constraint checklist"], f"document.querySelector('.composer textarea').value==={json.dumps(fuller)}")

    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: creation checkbox, advisory panel, stale and re-check verified; nothing was sent", flush=True)
finally:
    ws.close()
    x.close()
