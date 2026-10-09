#!/usr/bin/env python3
"""Assert the native GUI sidebar through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
import pathlib
import re
import sqlite3
import subprocess
import sys
import time
import urllib.request

import websocket
from Xlib import display

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
dbpath = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"

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
ws = websocket.create_connection(f"ws://{sys.argv[3]}{inspector_path}", timeout=15, suppress_origin=True)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def evaluate(expression):
    global seq
    seq += 1
    request = {"id": seq, "method": "Runtime.evaluate", "params": {"expression": expression, "returnByValue": True}}
    ws.send(json.dumps({"id": seq + 10000, "method": "Target.sendMessageToTarget",
                        "params": {"targetId": target, "message": json.dumps(request)}}))
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                assert not message["result"].get("wasThrown"), message
                return message["result"]["result"].get("value")


def wait(expression, tries=200):
    for _ in range(tries):
        if evaluate(expression):
            return
        time.sleep(0.1)
    raise AssertionError(expression + "\n" + str(evaluate("document.querySelector('.sidebar').innerText")))


NAV = "document.querySelector('nav[aria-label=Workspace]')"


def nav_has(text):
    return f"({NAV}?.innerText ?? '').includes({json.dumps(text)})"


def click_nav(name):
    """Click a tree button by its exact accessible text or aria-label."""
    evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('button')).find(b=>(b.getAttribute('aria-label')||b.textContent.trim())==={json.dumps(name)});if(!b)throw new Error('no button '+{json.dumps(name)});b.click();}})()""")


def db_value(sql):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql).fetchone()[0]


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


def capture(name, note, expects, expression=None):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    # Force native WebKit to repaint before saving the asserted DOM state.
    window.configure(width=1279)
    x.sync()
    time.sleep(0.5)
    window.configure(width=1280)
    x.sync()
    time.sleep(1.5)
    g = window.get_geometry()
    subprocess.run(
        ["/usr/bin/python3", "-c",
         """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
         str(window.id), str(g.width), str(g.height), str(out / name)],
        check=True,
    )
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name.replace(".png", ".ansi"), "note": note})
    print("PASS:", name, note, flush=True)


try:
    window.configure(x=0, y=0, width=1280, height=960)
    x.sync()
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE projects SET collapsed=0")
        db.execute("UPDATE features SET collapsed=1,mode='supervibe',review=1 WHERE id='shot-feature'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    wait(nav_has("Round invoice totals"))
    row = f"Array.from({NAV}.querySelectorAll('.tree-feature')).find(r=>r.querySelector('.tree-name')?.textContent.trim()==='Round invoice totals')"
    wait(f"!!({row}) && !({row}).querySelector('.tree-chip-mode') && !!({row}).querySelector('[aria-expanded=false]')")
    assert evaluate(f"({row}).querySelector('.tree-name').title.includes('Mode: supervibe')")
    click_nav("Round invoice totals")
    wait("document.querySelector('main')?.innerText.includes('Round invoice totals')")
    wait("!document.body.innerText.includes('Waiting for terminal connection') && document.body.innerText.includes('No plan selected') && document.body.innerText.includes('AMF capture fixture')")
    time.sleep(1)
    capture("001-collapsed-mode-hidden.png",
            "Collapsed feature: the supervibe badge is hidden, while review and session-count badges stay visible. The row tooltip still contains Mode: supervibe (asserted).",
            ["Round invoice totals", "review", "2 sessions"],
            f"!({row}).querySelector('.tree-chip-mode')")
    click_nav("Show sessions of Round invoice totals")
    wait(f"({row}).querySelector('.tree-chip-mode')?.textContent==='supervibe'")
    for _ in range(50):
        if db_value("SELECT collapsed FROM features WHERE id='shot-feature'") == 0:
            break
        time.sleep(0.1)
    assert db_value("SELECT collapsed FROM features WHERE id='shot-feature'") == 0
    assert evaluate(f"({row}).querySelector('.tree-name').title.includes('Mode: supervibe')")
    capture("002-expanded-mode-visible.png",
            "Expanded feature: the supervibe badge returns above the Claude and Codex session rows. Review and session-count badges remain visible, and expansion is persisted in the isolated database.",
            ["Round invoice totals", "supervibe", "review", "2 sessions", "Claude", "Codex"],
            f"!!({row}).querySelector('.tree-chip-mode') && ({row}).querySelectorAll('.tree-session').length===2")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(n)+"\n" for n in notes))
    print("PASS: collapsed and expanded mode badge, tooltip and persisted collapse verified", flush=True)
finally:
    ws.close()
    x.close()
