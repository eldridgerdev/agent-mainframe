#!/usr/bin/env python3
"""Assert literal GUI input survives the real persistent tmux control parser.

Private Git/SQLite/tmux fixtures and an offline raw byte receiver; no AI.
Only the isolated GUI PID's window is captured after forced native repaint.
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
from gi.repository import Gdk, GdkX11  # noqa: E402,F401

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
repo = pathlib.Path(sys.argv[4])

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
    f"ws://{sys.argv[3]}{inspector_path}", timeout=30, suppress_origin=True
)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def send(method, params):
    global seq
    seq += 1
    request = {"id": seq, "method": method, "params": params}
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
    # Say what the page showed instead, so a CI failure is diagnosable.
    state = evaluate(
        'JSON.stringify({header:document.querySelector(".diff-file-header,.learning-code-header")?.innerText,'
        'code:(document.querySelector(".diff-code,.learning-code")?.innerHTML||"").slice(0,1500),'
        'alerts:Array.from(document.querySelectorAll("[role=alert],[role=status]")).map(e=>e.innerText)})'
    )
    raise AssertionError(f"{expression}\nPage: {state}")


def click(text):
    found = evaluate(
        f'(()=>{{const b=Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)});if(b)b.click();return !!b;}})()'
    )
    assert found, f"No button {text!r}"






def tauri(command, args=None):
    """Runs a Tauri command from the page and returns its JSON result."""
    evaluate(
        f"window.__amfShot=undefined;window.__TAURI_INTERNALS__.invoke({json.dumps(command)},{json.dumps(args or {})}).then(v=>window.__amfShot={{ok:v}},e=>window.__amfShot={{err:e}})"
    )
    wait("window.__amfShot!==undefined")
    result = evaluate("window.__amfShot")
    assert "err" not in result, result
    return result["ok"]


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
    wait('!document.body.innerText.includes("Loading changes…") && !document.body.innerText.includes("Updating review…")')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    window.configure(width=1499)
    x.sync()
    time.sleep(0.5)
    window.configure(width=1500)
    x.sync()
    time.sleep(1.5)
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
config_dir = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf"
try:
    window.configure(x=0, y=0, width=1500, height=980)
    x.sync()
    wait('typeof window.__TAURI_INTERNALS__?.invoke === "function"')
    wait('!!document.querySelector("nav[aria-label=Workspace]")')
    with sqlite3.connect(config_dir / "amf.db") as db:
        db.execute("UPDATE projects SET collapsed=0")
        db.execute("UPDATE features SET collapsed=0")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('!!document.querySelector(".composer textarea") && !!document.querySelector(".xterm")')
    wait('document.body.innerText.includes("Ready for literal input.")')
    capture("001-literal-input-ready.png", "An isolated desktop terminal shows the expected literal text in an offline raw byte receiver before input is sent.", ["Literal terminal input proof", "Ready for literal input.", "$HOME", "${x}", "#{pane_id}"])
    payload = '$HOME ${x} \\ "double" \'single\' #{pane_id}'
    tauri("terminal_input", {"key":"shot-feature:shot-claude", "text":payload+"\r"})
    wait('document.body.innerText.includes("PASS: every byte matches the expected text.")')
    commands = [json.loads(line) for line in (pathlib.Path(os.environ["XDG_STATE_HOME"]) / "tmux-commands.jsonl").read_text().splitlines()]
    assert not any("send-keys" in args for args in commands), "Input fell back to direct send-keys"
    flags = subprocess.check_output([os.environ["AMF_GUI_CAPTURE_REAL_TMUX"], "-S", os.environ["AMF_TMUX_SOCKET"], "list-clients", "-t", "amf-gui-composer-proof", "-F", "#{client_flags}"], text=True)
    assert any("control-mode" in line and "no-output" in line for line in flags.splitlines()), flags
    assert json.loads((repo / "claude-received.json").read_text()) == [payload]
    assert json.loads((repo / "codex-received.json").read_text()) == []
    capture("002-literal-input-preserved.png", "The desktop terminal delivers $HOME, ${x}, backslashes, quotes and #{pane_id} unchanged through the real Rust terminal-input command. The offline receiver confirms an exact byte match.", ["Received literal bytes:", "PASS: every byte matches the expected text.", "$HOME", "${x}", "#{pane_id}"], '!document.body.innerText.includes("FAIL: input changed in transit.")')
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(n)+"\n" for n in notes))
    print("PASS: real GUI input IPC, exact receiver bytes, untouched second terminal", flush=True)
finally:
    ws.close()
    x.close()
