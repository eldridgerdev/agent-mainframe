#!/usr/bin/env python3
"""Assert native GUI dormancy states through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
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


def capture(name, note, expects, expression=None):
    wait('document.fonts.status==="loaded"')
    wait('!document.body.innerText.includes("Loading changes…")')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
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



import os
import sqlite3

dbpath = pathlib.Path(sys.argv[4])
tmux = ["tmux", "-S", sys.argv[5]]
owned_pid = int(sys.argv[6])
foreign_pid = int(sys.argv[7])


def tmux_alive(session):
    return subprocess.run(tmux + ["has-session", "-t", session], stderr=subprocess.DEVNULL).returncode == 0


def pid_running(pid):
    try:
        state = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
    except OSError:
        return False
    return state != "Z"


def check(label):
    evaluate(f"""(()=>{{const box=Array.from(document.querySelectorAll('input[type=checkbox]')).find(b=>b.getAttribute('aria-label')==={json.dumps('Select ' + label)});box.click();}})()""")


panel = "document.querySelector('[role=dialog][aria-label=\"Dormant features\"]')"

try:
    window.configure(x=0, y=0, width=1180, height=820)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Dormant features")')
    click("Dormant features")
    # tmux activity is real: refresh until every silent window has crossed the
    # one-minute threshold (the runner already waited for it).
    for _ in range(40):
        wait(f"!!{panel} && !{panel}.innerText.includes('Checking tmux activity')")
        if evaluate(f"{panel}.querySelectorAll('.dormancy-row').length") == 3:
            break
        click("Refresh")
        time.sleep(1.5)
    else:
        raise AssertionError(evaluate(f"{panel}.innerText"))
    listed = evaluate(f"Array.from({panel}.querySelectorAll('.dormancy-row-title > strong')).map(e=>e.textContent)")
    assert listed == ["billing-retry", "docs-refresh", "search-index"] or set(listed) == {"billing-retry", "docs-refresh", "search-index"}, listed
    capture(
        "001-dormant-list.png",
        "Three running features are idle (no agent output for over a minute) and unopened for over an hour, each with how long and since when. checkout-flow is just as unattended but its agent is printing output, so it is not listed. One row has an AMF-tracked editor open.",
        ["Dormant features", "billing-retry", "docs-refresh", "search-index", "no agent output since", "last opened", "Editor open", "Stop selected (0)"],
        f"!{panel}.innerText.includes('checkout-flow')",
    )

    for name in ["billing-retry", "docs-refresh", "search-index"]:
        check(name)
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Stop selected (3)")')
    click("Stop selected (3)")
    wait("!!document.querySelector('[role=alertdialog]')")
    assert all(tmux_alive(f"amf-{name}") for name in ["billing-retry", "docs-refresh", "search-index", "checkout-flow"])
    capture(
        "002-confirm-stop.png",
        "Nothing stops until this explicit confirmation. It names each tmux session, explains which editor windows AMF will and will not close, and says every feature is re-checked first.",
        ["Stop 3 dormant features?", "(amf-billing-retry)", "Editor windows AMF opened", "left running and reported", "Each one is checked again first", "Stop 3 features"],
    )

    # search-index is opened in another AMF window after the list loaded.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE features SET last_accessed=? WHERE id='search-index'",
                   (time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),))
        version = int(db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()[0])
        db.execute("UPDATE store_meta SET value=? WHERE key='store_version'", (version + 1,))
    click("Stop 3 features")
    wait("!!document.querySelector('[aria-label=\"Stop results\"]')")
    for _ in range(50):
        if not pid_running(owned_pid):
            break
        time.sleep(0.1)
    assert not pid_running(owned_pid), "the AMF-owned stand-in editor should be closed"
    assert pid_running(foreign_pid), "the editor AMF did not open must keep running"
    assert not tmux_alive("amf-billing-retry") and not tmux_alive("amf-docs-refresh")
    assert tmux_alive("amf-search-index") and tmux_alive("amf-checkout-flow")
    capture(
        "003-stop-results.png",
        "Per-feature results after the confirm-time re-check: billing-retry stopped and its AMF-opened stand-in editor was closed with its child process; docs-refresh stopped but the editor AMF did not open was left running; search-index was opened elsewhere after the list loaded, so it was refused and its session still runs.",
        ["billing-retry", "Closed VS Code (2 processes ended)", "Left VS Code running: AMF did not open this window", "not stopped: It was opened in AMF after the list was loaded"],
    )

    click("Back to dormant features")
    wait(f"{panel}.innerText.includes('Nothing is dormant right now')")
    capture(
        "004-nothing-dormant.png",
        "Back on the list, nothing is dormant: the two stopped features hold no agent, search-index was just opened and checkout-flow is still producing output.",
        ["Nothing is dormant right now", "idle over 1m and unopened over 1h"],
        "!Array.from(document.querySelectorAll('button')).some(b=>b.textContent.includes('Stop selected'))",
    )
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: Dormancy listing, confirmation, confirm-time refusal and editor ownership verified", flush=True)
finally:
    ws.close()
    x.close()
