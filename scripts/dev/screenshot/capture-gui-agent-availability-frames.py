#!/usr/bin/env python3
"""Assert native GUI states through WebKit IPC and capture its X11 window.

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



config_path = pathlib.Path(sys.argv[4])

def toggle(label):
    evaluate(f'Array.from(document.querySelectorAll("label")).find(l=>l.textContent.trim()==={json.dumps(label)}).querySelector("input").click()')

def checked():
    return evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).filter(e=>e.checked).map(e=>e.parentElement.textContent.trim())')

try:
    window.configure(x=0, y=0, width=1180, height=780)
    x.sync()
    click("Agent availability")
    wait('document.querySelectorAll("input[type=checkbox]").length===4 && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Claude Code", "Codex"]
    capture("001-agent-settings-loaded.png", "Global agent choices load from the shared config; project overrides and installed availability still apply.", ["Agent availability", "Claude Code", "Codex", "OpenCode", "Pi", "Project allowed-agent settings take precedence."])

    toggle("Pi")
    click("Cancel")
    wait("!!document.querySelector('[role=alertdialog][aria-label=\"Discard unsaved settings\"]')")
    capture("002-agent-discard-guard.png", "Cancel with unsaved agent choices asks before discarding the draft.", ["Discard unsaved settings and close?", "Keep editing", "Discard changes"])
    click("Keep editing")
    saved = json.loads(config_path.read_text())
    saved["future_setting"] = 43
    config_path.write_text(json.dumps(saved))
    click("Save settings")
    wait('document.body.innerText.includes("Global config changed. Reload settings before saving.")')
    assert checked() == ["Claude Code", "Codex", "Pi"]
    capture("003-agent-stale-save-refused.png", "An external config edit refuses the stale save while retaining the agent draft.", ["Global config changed. Reload settings before saving.", "Reload settings", "Save settings"], allow_alert=True)
    assert json.loads(config_path.read_text())["extension"]["allowed_agents"] == ["claude", "codex"]

    click("Reload settings")
    wait("!!document.querySelector('[role=alertdialog][aria-label=\"Discard unsaved settings\"]')")
    click("Discard changes")
    wait('!document.querySelector("[role=alertdialog]") && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Claude Code", "Codex"]
    toggle("Claude Code")
    toggle("Pi")
    click("Save settings")
    wait("!document.querySelector('[role=dialog][aria-label=\"Agent availability\"]')")
    saved = json.loads(config_path.read_text())
    assert saved["extension"]["allowed_agents"] == ["codex", "pi"], saved
    assert saved["future_setting"] == 43 and saved["extension"]["future_nested"]["keep"] is True and saved["dormant_idle_minutes"] == 15, saved
    click("Agent availability")
    wait('document.querySelectorAll("input[type=checkbox]").length===4 && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ["Codex", "Pi"]
    capture("004-agent-settings-saved.png", "Reopening verifies the explicitly saved Codex and Pi choices; unrelated config and existing sessions are preserved.", ["Agent availability", "Existing sessions keep running.", "Codex", "Pi"], 'Array.from(document.querySelectorAll("input[type=checkbox]")).map(e=>e.checked).join(",")==="false,true,false,true"')
    evaluate('window.__agentOptions=undefined;window.__TAURI_INTERNALS__.invoke("new_session_options",{target:{project_id:"shot-project",feature_id:"shot-feature"}}).then(v=>window.__agentOptions={ok:v},e=>window.__agentOptions={err:e})')
    wait('window.__agentOptions!==undefined')
    options = evaluate('window.__agentOptions')
    assert "err" not in options, options
    agent_kinds = [choice["kind"] for choice in options["ok"]["builtin"] if choice["kind"] in ["claude", "codex", "opencode", "pi"]]
    assert agent_kinds == ["codex", "pi"], options
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: agent choices load, discard protection, stale refusal, persistence and unrelated config preservation", flush=True)
finally:
    ws.close()
    x.close()
