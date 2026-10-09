#!/usr/bin/env python3
"""Assert native before/after dark views and live system appearance switching.

Private Git/SQLite/tmux fixtures, offline ANSI terminal, no prompts sent.
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


def install_parser(key):
    """Installs a parser through the GUI's command, as the badge's button does."""
    status = tauri("syntax_install_status")
    if status["running"]:
        raise AssertionError(f"An install is already running: {status}")
    tauri("syntax_install", {"language": key})
    deadline = time.time() + 600
    while time.time() < deadline:
        status = tauri("syntax_install_status")
        if not status["running"]:
            assert status["error"] is None and status["message"], status
            print("INSTALLED:", status["message"], flush=True)
            return
        time.sleep(1)
    raise AssertionError(f"{key} parser install timed out")


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


def choose_file(path):
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    wait('!document.body.innerText.includes("Updating review…")')








def set_dark(dark):
    send(
        "Page.overrideUserPreference",
        {"name": "PrefersColorScheme", "value": "Dark" if dark else "Light"},
    )
    wait(f'matchMedia("(prefers-color-scheme: dark)").matches==={str(dark).lower()}')


def open_changes():
    click("Changes")
    wait('!!document.querySelector(".diff-file")')
    wait('!document.body.innerText.includes("Loading changes…")')



import os
import sqlite3
mode = os.environ["AMF_PROOF_MODE"]

def choose_rust():
    choose_file("invoice.rs")
    wait('!!document.querySelector(".syn-keyword")')

try:
    window.configure(x=0, y=0, width=1500, height=980)
    x.sync()
    wait('typeof window.__TAURI_INTERNALS__?.invoke === "function"')
    wait('!!document.querySelector("nav[aria-label=Workspace]")')
    set_dark(True)
    (repo / "invoice.rs").write_text("// Round currency at the boundary.\npub fn rounded_total(subtotal: f64, tax_rate: f64) -> f64 {\n    let total = subtotal * (1.0 + tax_rate);\n    (total * 100.0).round() / 100.0\n}\n")
    install_parser("rust")
    with sqlite3.connect(pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db") as db:
        db.execute("UPDATE projects SET collapsed=0")
        db.execute("UPDATE features SET collapsed=0")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('!!document.querySelector(".composer textarea") && !!document.querySelector(".agent-sidebar")')
    wait('document.body.innerText.includes("Invoice checks")')
    wanted = "#111418" if mode == "before" else "#242932"
    wait(f'getComputedStyle(document.documentElement).getPropertyValue("--bg").trim()==={json.dumps(wanted)}')
    before_canvas = evaluate('getComputedStyle(document.querySelector(".term-frame")).backgroundColor')
    assert before_canvas == ("rgb(13, 16, 20)" if mode == "before" else "rgb(36, 41, 50)"), before_canvas
    capture("001-workspace-terminal.png", f"{mode.capitalize()}: feature workspace, expanded projects sidebar, agent sidebar and live terminal with ANSI status colors.", ["Invoice API", "Round invoice totals", "Invoice checks", "rounding preserves cents", "Send prompt"])
    click("New session")
    wait('!!document.querySelector("[role=dialog]")')
    capture("002-new-session-dialog.png", f"{mode.capitalize()}: New session dialog shows panel separation, borders, labels and secondary text.", ["New session", "Open another agent, terminal, editor or configured session in this feature."])
    evaluate('document.querySelector("[role=dialog] button[aria-label=Close]").click()')
    wait('!document.querySelector("[role=dialog]")')
    open_changes()
    choose_rust()
    capture("003-highlighted-diff.png", f"{mode.capitalize()}: Rust syntax colors on added diff rows with file navigation and controls.", ["Changes · Round invoice totals", "invoice.rs", "Round currency at the boundary.", "rounded_total"])
    click("Close")
    wait('!document.querySelector(".diff-reader")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_rust()
    capture("004-final-review.png", f"{mode.capitalize()}: Final Review uses the same readable text, syntax colors, surfaces and controls.", ["Final Review", "invoice.rs", "rounded_total", "Pause review"])
    click("Pause review")
    wait('!document.querySelector(".diff-reader")')
    if mode == "after":
        set_dark(False)
        wait('getComputedStyle(document.querySelector(".term-frame")).backgroundColor==="rgb(13, 16, 20)"')
        capture("005-light-appearance.png", "Light appearance retains its original page and terminal palette when system appearance switches live.", ["Round invoice totals", "Invoice checks", "Send prompt"])
        set_dark(True)
        wait('getComputedStyle(document.querySelector(".term-frame")).backgroundColor==="rgb(36, 41, 50)"')
    for name in ["claude-received.json", "codex-received.json"]:
        assert json.loads((repo / name).read_text()) == [], "A prompt reached the fixture"
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(n)+"\n" for n in notes))
    print("PASS: dark appearance, real IPC views, live appearance switching, no prompts sent", flush=True)
finally:
    ws.close()
    x.close()
