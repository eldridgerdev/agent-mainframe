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

def fill_first(value):
    js = f"(()=>{{const e=document.querySelector('input[type=number]');const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));return e.value}})()"
    assert evaluate(js) == value
    wait(f"document.querySelector('input[type=number]').value==={json.dumps(value)}")

try:
    window.configure(x=0, y=0, width=1180, height=780)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Dormant features")')
    click("Dormant features")
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Settings")')
    click("Settings")
    wait('!!document.querySelector("[role=dialog][aria-label=\\"Dormancy settings\\"] input[type=number]") && !document.body.innerText.includes("Loading or saving settings…")')
    capture("001-settings-loaded.png", "Loaded current global thresholds; zero disables detection.", ["Dormancy settings", "Idle minutes", "Unattended hours", "Set either value to 0 to turn detection off."], 'Array.from(document.querySelectorAll("input[type=number]")).map(e=>e.value).join(",")==="15,4"')

    fill_first("20")
    click("Cancel")
    wait('!!document.querySelector(\'[role=alertdialog][aria-label="Discard unsaved settings"]\')')
    capture("002-discard-guard.png", "Cancel with a dirty draft asks before discarding or closing.", ["Discard unsaved settings and close?", "Keep editing", "Discard changes"])
    click("Keep editing")
    wait('!!document.querySelector(\'[role=dialog][aria-label="Dormancy settings"]\')')
    fill_first("25")
    config_path.write_text('{"dormant_idle_minutes":30,"dormant_last_accessed_hours":4,"kill_editor_on_stop":true}\n')
    click("Save settings")
    wait('document.body.innerText.includes("Global config changed. Reload settings before saving.")')
    capture("003-stale-save-refused.png", "A stale save is refused and the draft remains in the field.", ["Global config changed. Reload settings before saving.", "Reload settings", "Save settings"], 'document.querySelector("input[type=number]").value==="25"', allow_alert=True)
    assert evaluate('!!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save settings" && !b.disabled)')

    click("Reload settings")
    wait('!!document.querySelector(\'[role=alertdialog][aria-label="Discard unsaved settings"]\')')
    click("Discard changes")
    wait('document.querySelector(\'[role=dialog][aria-label="Dormancy settings"] input[type=number]\').value==="30"')
    fill_first("0")
    click("Save settings")
    wait('document.body.innerText.includes("Dormancy detection is off")')
    with config_path.open() as f:
        import json as json_module
        saved=json_module.load(f)
    assert saved["dormant_idle_minutes"] == 0 and saved["dormant_last_accessed_hours"] == 4, saved
    capture("004-detection-disabled.png", "Saving zero idle minutes immediately disables detection while preserving the other threshold.", ["Dormancy detection is off", "Open Settings and set both thresholds above 0"])
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: settings load, discard guard, stale-save refusal and successful disable verified", flush=True)
finally:
    ws.close()
    x.close()
