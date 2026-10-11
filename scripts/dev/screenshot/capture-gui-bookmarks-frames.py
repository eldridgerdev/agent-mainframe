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



try:
    window.configure(x=0, y=0, width=1280, height=850)
    x.sync()
    wait('document.body.innerText.includes("Round invoice totals")')
    click("Session bookmarks")
    wait('document.body.innerText.includes("No bookmarked sessions yet.")')
    capture("001-bookmarks-empty.png", "The bookmark picker starts empty and lists existing sessions without launching agents.", ["Session bookmarks", "No bookmarked sessions yet.", "Bookmark session"])
    evaluate("""(()=>{const el=document.querySelector('[role=dialog] select');el.value=JSON.stringify(['shot-project','shot-feature','shot-codex']);el.dispatchEvent(new Event('change',{bubbles:true}));})()""")
    click("Bookmark session")
    wait('document.body.innerText.includes("1. Invoice API / Round invoice totals / Codex")')
    with sqlite3.connect(pathlib.Path(sys.argv[4]).parent / "amf.db") as db:
        assert db.execute("SELECT session_id FROM session_bookmarks ORDER BY rowid").fetchall() == [("shot-codex",)]
    capture("002-bookmark-added.png", "Codex is saved in shared slot 1; the picker exposes direct navigation and removal.", ["1. Invoice API / Round invoice totals / Codex", "Remove"])
    click("1. Invoice API / Round invoice totals / Codex")
    wait('!document.querySelector(\'[role=dialog][aria-label="Session bookmarks"]\') && !!document.querySelector(".composer textarea")')
    evaluate("""(()=>{const e=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,'Unsent bookmark navigation draft');e.dispatchEvent(new Event('input',{bubbles:true}));})()""")
    wait('document.querySelector(".composer textarea").value==="Unsent bookmark navigation draft"')
    wait('Array.from(document.querySelectorAll(".composer button")).some(b=>b.textContent.trim()==="Clear" && !b.disabled)')
    click("Session bookmarks")
    wait('document.body.innerText.includes("1. Invoice API / Round invoice totals / Codex")')
    click("Close")
    wait('!document.querySelector(\'[role=dialog][aria-label="Session bookmarks"]\')')
    capture("003-bookmark-navigation.png", "Opening the bookmark selects Codex, and reopening the picker preserves the unsent composer draft.", ["Round invoice totals", "Codex"], 'document.querySelector(".composer textarea").value==="Unsent bookmark navigation draft"')
    click("Session bookmarks")
    wait('!!document.querySelector(\'[aria-label="Remove bookmark 1"]\')')
    evaluate('document.querySelector(\'[aria-label="Remove bookmark 1"]\').click()')
    wait('document.body.innerText.includes("No bookmarked sessions yet.")')
    capture("004-bookmark-removed.png", "Removing the stable bookmark clears its shared slot without closing the session.", ["No bookmarked sessions yet.", "Bookmark session"])
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: bookmark add, navigation, unsent draft retention and removal verified", flush=True)
finally:
    ws.close()
    x.close()
