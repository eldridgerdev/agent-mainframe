#!/usr/bin/env python3
"""Assert and capture populated, filtered, and refreshed debug-log states."""
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

import gi
import websocket
from Xlib import display

gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gdk, GdkX11

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
address = sys.argv[3]

for _ in range(100):
    try:
        with urllib.request.urlopen(f"http://{address}/", timeout=2) as response:
            page = response.read().decode()
        path = re.search(r"/socket/[^']+/WebPage", page).group(0)
        break
    except (OSError, AttributeError):
        time.sleep(0.1)
else:
    raise RuntimeError("Native WebKit inspector did not become ready")
ws = websocket.create_connection(f"ws://{address}{path}", timeout=15, suppress_origin=True)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def send(method, params):
    global seq
    seq += 1
    request = {"id": seq, "method": method, "params": params}
    ws.send(json.dumps({"id": seq + 10000, "method": "Target.sendMessageToTarget",
                        "params": {"targetId": target, "message": json.dumps(request)}}))
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                return message["result"]


def evaluate(expression):
    result = send("Runtime.evaluate", {"expression": expression, "returnByValue": True})
    assert not result.get("wasThrown"), result
    return result["result"].get("value")


def wait(expression, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if evaluate(expression):
            return
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {expression}; body={evaluate('document.body.innerText')}")


def click(label):
    found = evaluate(f"(()=>{{const b=Array.from(document.querySelectorAll('button')).find(x=>x.textContent.trim().includes({json.dumps(label)}));if(b)b.click();return !!b}})()")
    assert found, f"Missing button {label!r}; body={evaluate('document.body.innerText')}"


def fill(selector, value):
    js = f"(()=>{{const e=document.querySelector({json.dumps(selector)});const s=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;s.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));return e.value}})()"
    assert evaluate(js) == value


def select(selector, value):
    js = f"(()=>{{const e=document.querySelector({json.dumps(selector)});e.value={json.dumps(value)};e.dispatchEvent(new Event('change',{{bubbles:true}}));return e.value}})()"
    assert evaluate(js) == value


dbpath = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"
with sqlite3.connect(dbpath) as db:
    db.executemany("INSERT INTO debug_log(ts,level,context,message) VALUES(?,?,?,?)", [
        ("2026-10-09T14:02:00Z", "INFO", "startup", "GUI ready for review"),
        ("2026-10-09T14:03:00Z", "WARN", "sync", "Retrying invoice index after timeout"),
        ("2026-10-09T14:04:00Z", "ERROR", "database", "Invoice cache is unavailable"),
        ("2026-10-09T14:05:00Z", "WARN", "hooks", "Notification hook took too long"),
    ])

x = display.Display()
atom = x.intern_atom("_NET_WM_PID")
windows = []


def visit(window):
    try:
        prop = window.get_full_property(atom, 0)
        geometry = window.get_geometry()
        if prop and int(prop.value[0]) == pid and geometry.width > 500:
            windows.append(window)
        for child in window.query_tree().children:
            visit(child)
    except Exception:
        pass


for _ in range(100):
    visit(x.screen().root)
    if windows:
        break
    time.sleep(0.1)
assert windows, "Isolated GUI has no X11 window"
window = max(windows, key=lambda w: w.get_geometry().width * w.get_geometry().height)
captures = set()
notes = []


def capture(name, expected, note, allow_alert=False):
    wait('document.fonts.status==="loaded"')
    wait('document.querySelectorAll(".debug-log-entry").length>0')
    body = evaluate("document.body.innerText")
    for text in expected:
        assert text in body, (text, body)
    if not allow_alert:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    time.sleep(0.35)
    g = window.get_geometry()
    subprocess.run(["/usr/bin/python3", "-c", "import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])",
                    str(window.id), str(g.width), str(g.height), str(out / name)], check=True)
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captures, "Native GUI saved a stale frame"
    captures.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expected})
    print("PASS:", name, note, flush=True)


wait('Array.from(document.querySelectorAll("button")).some(x=>x.textContent.trim().includes("Debug log"))')
click("Debug log")
wait('!!document.querySelector(".debug-log-entry")')
capture("001-debug-log-populated.png", ["Debug log", "Showing 4 of 4 entries", "GUI ready for review", "Retrying invoice index", "Invoice cache is unavailable"], "The viewer shows recent entries with timestamps, levels, contexts, and messages.")
select(".debug-log-toolbar select", "WARN")
fill('.debug-log-toolbar input[type="search"]', "invoice")
wait('document.querySelectorAll(".debug-log-entry").length===1')
capture("002-debug-log-filtered.png", ["Showing 1 of 4 entries", "WARN", "Retrying invoice index"], "WARN level and invoice text filters reduce the list to the matching sync warning.")
assert "Notification hook" not in evaluate("document.body.innerText")

with sqlite3.connect(dbpath) as db:
    db.execute("INSERT INTO debug_log(ts,level,context,message) VALUES(?,?,?,?)",
               ("2026-10-09T14:06:00Z", "WARN", "sync", "Invoice index refreshed successfully"))
click("Refresh log")
wait('document.body.innerText.includes("Invoice index refreshed successfully")')
wait('document.querySelectorAll(".debug-log-entry").length===2')
capture("003-debug-log-refreshed.png", ["Showing 2 of 5 entries", "Retrying invoice index", "Invoice index refreshed successfully"], "Refresh loads the new database entry while retaining both active filters.")

with sqlite3.connect(dbpath) as db:
    db.execute("DROP TABLE debug_log")
click("Refresh log")
wait('document.body.innerText.includes("Could not load debug log:")')
capture("004-debug-log-refresh-error.png", ["Could not load debug log:", "Showing the previous load. Refresh to try again.", "Showing 2 of 5 entries", "Retrying invoice index", "Invoice index refreshed successfully"], "A failed refresh explains the error and keeps the previously loaded filtered results visible.", allow_alert=True)

(out / "capture-notes.jsonl").write_text("".join(json.dumps(note) + "\n" for note in notes))
