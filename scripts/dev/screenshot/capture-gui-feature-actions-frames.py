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



db_path = pathlib.Path(sys.argv[4]).parent / "amf.db"
repo_received = None


def db(sql, *args):
    with sqlite3.connect(db_path) as conn:
        return conn.execute(sql, args).fetchall()


def set_input(selector, value):
    evaluate(
        f"""(()=>{{const e=document.querySelector({json.dumps(selector)});Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()"""
    )


def click_selector(selector):
    wait(f"!!document.querySelector({json.dumps(selector)}) && !document.querySelector({json.dumps(selector)}).disabled")
    evaluate(f"document.querySelector({json.dumps(selector)}).click()")


def menu_item(label, name):
    evaluate(f"""document.querySelector('[aria-label={json.dumps(label)}]').click()""")
    wait(f'Array.from(document.querySelectorAll("[role=menuitem]")).some(b=>b.textContent.includes({json.dumps(name)}))')
    evaluate(
        f'Array.from(document.querySelectorAll("[role=menuitem]")).find(b=>b.textContent.includes({json.dumps(name)})).click()'
    )


def tmux_windows():
    import os
    socket = os.environ["AMF_TMUX_SOCKET"]
    return subprocess.check_output(
        ["tmux", "-S", socket, "list-windows", "-a", "-F", "#{window_name}"], text=True
    ).split()


def bump_store_version():
    with sqlite3.connect(db_path) as conn:
        version = conn.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
        conn.execute(
            "INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
            ((int(version[0]) if version else 0) + 1,),
        )


try:
    window.configure(x=0, y=0, width=1280, height=850)
    x.sync()
    wait('document.body.innerText.includes("Round invoice totals")')
    click("Round invoice totals")
    wait('!!document.querySelector(\'[aria-label="More feature actions"]\')')
    windows_before = tmux_windows()
    menu_item("More feature actions", "Rename, ready and summary")
    wait('!!document.querySelector(\'[role=dialog][aria-label="Feature actions"]\') && !!document.querySelector(\'[aria-label="Name of session Claude"]\')')
    capture("001-actions-panel.png", "The feature menu opens one panel for nickname, session names, ready state and summary; nothing has run.", ["Rename, ready and summary", "Save nickname", "Generate summary…", "Not marked ready."])

    set_input('section[aria-label="Nickname"] input', "Totals polish")
    click("Save nickname")
    wait('document.body.innerText.includes("Totals polish")')
    assert db("SELECT nickname, name, branch FROM features WHERE id='shot-feature'") == [("Totals polish", "Round invoice totals", "round-invoice-totals")]
    capture("002-nickname-saved.png", "The nickname is stored and shown in the sidebar; the feature name and branch are untouched.", ["Totals polish"])

    set_input('[aria-label="Name of session Claude"]', "Main agent")
    click_selector('[aria-label="Rename session Claude"]')
    wait('!!document.querySelector(\'[aria-label="Name of session Main agent"]\')')
    assert db("SELECT label, tmux_window FROM feature_sessions WHERE id='shot-claude'") == [("Main agent", "claude")]
    assert tmux_windows() == windows_before, "renaming a session must not touch tmux"
    capture("003-session-renamed.png", "The Claude session label changed; its tmux window keeps its name.", ["Main agent"])

    click("Mark ready")
    wait('document.body.innerText.includes("Marked ready.")')
    assert db("SELECT ready FROM features WHERE id='shot-feature'") == [(1,)]
    capture("004-marked-ready.png", "The ready flag is stored in the same column the TUI uses.", ["Marked ready.", "Mark not ready"])

    click("Generate summary…")
    wait('!!document.querySelector(\'[role=group][aria-label="Headless AI call"]\')')
    assert db("SELECT summary FROM features WHERE id='shot-feature'") == [(None,)]
    capture("005-summary-notice.png", "The pre-call notice names the prompt and harness first; no harness call has been made.", ["This runs a headless AI call", "session.summary", "Run with Claude", "Cancel"])
    click("Cancel")
    wait('!document.querySelector(\'[role=group][aria-label="Headless AI call"]\')')
    assert db("SELECT summary FROM features WHERE id='shot-feature'") == [(None,)]

    with sqlite3.connect(db_path) as conn:
        conn.execute("UPDATE features SET nickname='Renamed in the TUI' WHERE id='shot-feature'")
    bump_store_version()
    wait('document.body.innerText.includes("changed elsewhere")')
    capture("006-changed-elsewhere.png", "A change made elsewhere is flagged with a Reload instead of being silently adopted.", ["changed elsewhere", "Reload"])
    set_input('section[aria-label="Nickname"] input', "Stale overwrite")
    click("Save nickname")
    wait('!!document.querySelector("[role=alert]")')
    assert db("SELECT nickname FROM features WHERE id='shot-feature'") == [("Renamed in the TUI",)]
    capture("007-stale-refused.png", "Saving over the stale nickname is refused with an explicit message; the other change survives.", ["The nickname changed since you opened this"], allow_alert=True)

    for filename in ["claude-received.json", "codex-received.json"]:
        assert json.loads((pathlib.Path(sys.argv[4]).parent.parent.parent / "demo-api" / filename).read_text()) == []
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: feature rename, session rename, ready, summary notice and stale refusal verified", flush=True)
finally:
    ws.close()
    x.close()
