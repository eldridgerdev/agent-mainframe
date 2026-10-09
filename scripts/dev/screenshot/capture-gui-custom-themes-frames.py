#!/usr/bin/env python3
"""Assert catalog/custom theme selection, precedence and terminal continuity.

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
config_path = config_dir / "config.json"

def tui_theme(name):
    config = json.loads(config_path.read_text())
    config["theme"] = name
    config_path.write_text(json.dumps(config))

def choose_theme(value):
    evaluate(f"""(()=>{{const el=document.querySelector('select[aria-label=Theme]');el.value={json.dumps(value)};el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()""")
    wait(f"document.querySelector('select[aria-label=Theme]')?.value==={json.dumps(value)}")

def bg(value):
    wait(f'getComputedStyle(document.documentElement).getPropertyValue("--bg").trim()==={json.dumps(value)}')

def close_picker():
    evaluate('document.querySelector("[role=dialog] button[aria-label=Close]").click()')
    wait('!document.querySelector("[role=dialog]")')

def continuity():
    assert evaluate('window.__themeTerminal===document.querySelector(".xterm")'), "Terminal remounted"
    assert evaluate('document.querySelector(".composer textarea").value==="Keep this unsent draft while changing themes."'), "Draft was lost"

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
    wait('document.body.innerText.includes("Invoice checks")')
    evaluate("""(()=>{window.__themeTerminal=document.querySelector('.xterm');const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,'Keep this unsent draft while changing themes.');el.dispatchEvent(new Event('input',{bubbles:true}));})()""")
    tui_theme("nord")
    bg("#2e3440")
    click("Appearance")
    wait('document.querySelector("select[aria-label=Theme]")?.options.length===32')
    assert evaluate('document.querySelector("select[aria-label=Theme]").value==="follow-tui"')
    capture("001-follow-tui-nord.png", "Appearance follows the TUI’s Nord theme by default and offers independent GUI choices without altering the TUI setting.", ["Appearance", "Custom JSON themes"], 'document.documentElement.dataset.theme==="dark"')
    choose_theme("dracula")
    bg("#282a36")
    close_picker()
    continuity()
    tui_theme("gruvbox-light")
    time.sleep(4)
    bg("#282a36")
    assert json.loads(config_path.read_text())["theme"]=="gruvbox-light"
    capture("002-dracula-workspace.png", "Dracula applies to the workspace, sidebar and terminal live. The terminal and unsent draft remain intact while the GUI override wins over a later TUI theme change.", ["Round invoice totals", "Invoice checks", "Send prompt"], 'getComputedStyle(document.querySelector(".term-frame")).backgroundColor==="rgb(40, 42, 54)"')
    click("Appearance")
    choose_theme("catppuccin-latte")
    bg("#eff1f5")
    close_picker()
    continuity()
    capture("003-catppuccin-latte.png", "Catppuccin Latte supplies a light catalog palette for the desktop and terminal without restarting the session or discarding its draft.", ["Round invoice totals", "Invoice checks", "Send prompt"], 'document.documentElement.dataset.theme==="light" && getComputedStyle(document.querySelector(".term-frame")).backgroundColor==="rgb(239, 241, 245)"')
    themes = config_dir / "gui-themes"
    themes.mkdir()
    (themes / "ocean.json").write_text(json.dumps({"id":"ocean", "name":"Ocean (custom)", "mode":"dark", "tokens":{"accent":"#90dce5", "bg-sidebar":"#203847"}, "terminal":{"background":"#18313e", "cyan":"#90dce5"}}))
    click("Appearance")
    wait('Array.from(document.querySelector("select[aria-label=Theme]").options).some(o=>o.value==="custom:ocean")')
    choose_theme("custom:ocean")
    bg("#242932")
    wait('getComputedStyle(document.querySelector(".term-frame")).backgroundColor==="rgb(24, 49, 62)"')
    capture("004-custom-ocean-picker.png", "A partial Ocean JSON theme is discovered automatically. Its cyan accent and sidebar/terminal overrides sit on the readable built-in dark fallback palette.", ["Appearance", "Custom JSON themes"], 'document.querySelector("select[aria-label=Theme]").value==="custom:ocean"')
    close_picker()
    continuity()
    capture("005-custom-ocean-workspace.png", "The custom Ocean theme colors the live workspace and terminal while retaining session identity, terminal contents and the unsent composer draft.", ["Round invoice totals", "Invoice checks", "Send prompt"], 'getComputedStyle(document.querySelector(".sidebar")).backgroundColor==="rgb(32, 56, 71)"')
    # Wait for a new JS realm; old-page DOM can survive briefly after reload().
    evaluate('window.__themeReloadPending=true;location.reload()')
    wait('window.__themeReloadPending!==true && !!document.querySelector("nav[aria-label=Workspace]")')
    wait('getComputedStyle(document.documentElement).getPropertyValue("--accent").trim()==="#90dce5"')
    click("Appearance")
    wait('document.querySelector("select[aria-label=Theme]")?.value==="custom:ocean"')
    choose_theme("follow-tui")
    bg("#fbf1c7")
    assert json.loads(config_path.read_text())["theme"]=="gruvbox-light"
    assert evaluate('localStorage.getItem("amf.gui.theme")===null')
    for name in ["claude-received.json", "codex-received.json"]:
        assert json.loads((repo / name).read_text()) == [], "A prompt reached the fixture"
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(n)+"\n" for n in notes))
    print("PASS: persisted custom choice, Follow TUI clearing, unchanged TUI config, terminal/draft continuity, no prompts sent", flush=True)
finally:
    ws.close()
    x.close()
