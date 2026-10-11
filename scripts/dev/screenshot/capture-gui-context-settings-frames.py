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

def fill(label, value):
    evaluate(f"(()=>{{const e=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)}).querySelector('input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()")
    wait(f'Array.from(document.querySelectorAll("label")).find(l=>l.querySelector("span")?.textContent==={json.dumps(label)}).querySelector("input").value==={json.dumps(value)}')

def values():
    return evaluate('Array.from(document.querySelectorAll(".modal input")).map(e=>e.value)')

try:
    window.configure(x=0, y=0, width=1180, height=840)
    x.sync()
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["500000", "70", "85"]
    capture("001-context-settings-loaded.png", "Loaded global context limit and warning/critical thresholds share the TUI settings; a blank token limit restores harness defaults.", ["Context settings", "Context window (tokens)", "Warning percent", "Critical percent", "Leave blank to restore harness defaults."])
    fill("Warning percent", "90")
    wait('!!document.querySelector("[role=alert]")')
    capture("002-context-invalid-order.png", "Critical must exceed warning; the invalid order blocks Save without writing configuration.", ["Percentages must be 1–100, with critical greater than warning."], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save settings").disabled', allow_alert=True)
    assert json.loads(config_path.read_text())["context_warning_percent"] == 70
    fill("Warning percent", "70")
    fill("Context window (tokens)", "18446744073709551615")
    click("Cancel")
    wait('!!document.querySelector("[role=alertdialog]")')
    capture("003-context-discard-guard.png", "Cancel protects the unsaved full-width token-limit draft until explicit discard.", ["Discard unsaved settings and close?", "Keep editing", "Discard changes"])
    click("Keep editing")
    saved = json.loads(config_path.read_text())
    saved["future_setting"] = 43
    saved["context_window_override"] = 300000
    saved["context_warning_percent"] = 60
    saved["context_critical_percent"] = 90
    config_path.write_text(json.dumps(saved))
    click("Save settings")
    wait('document.body.innerText.includes("Global config changed. Reload settings before saving.")')
    assert values() == ["18446744073709551615", "70", "85"]
    capture("004-context-stale-save-refused.png", "An external edit refuses a stale save and retains the complete token-limit draft.", ["Global config changed. Reload settings before saving.", "Reload settings"], allow_alert=True)
    assert json.loads(config_path.read_text())["context_window_override"] == 300000
    click("Reload settings")
    wait('!!document.querySelector("[role=alertdialog]")')
    click("Discard changes")
    wait('!document.querySelector("[role=alertdialog]") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["300000", "60", "90"]
    fill("Context window (tokens)", "18446744073709551615")
    click("Save settings")
    wait('!document.querySelector(".modal")')
    assert json.loads(config_path.read_text())["context_window_override"] == 18446744073709551615
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["18446744073709551615", "60", "90"]
    capture("005-context-full-u64-saved.png", "Saving and reopening retains the maximum u64 token limit exactly across native IPC and JSON persistence.", ["Context settings", "Context window (tokens)"])
    fill("Context window (tokens)", "")
    fill("Warning percent", "80")
    fill("Critical percent", "95")
    click("Save settings")
    wait('!document.querySelector(".modal")')
    saved = json.loads(config_path.read_text())
    assert "context_window_override" not in saved and saved["context_warning_percent"] == 80 and saved["context_critical_percent"] == 95, saved
    assert saved["future_setting"] == 43 and saved["extension"]["future_nested"]["keep"] is True and saved["dormant_idle_minutes"] == 15, saved
    click("Context settings")
    wait('!!document.querySelector(".modal input") && !document.body.innerText.includes("Loading or saving settings…")')
    assert values() == ["", "80", "95"]
    capture("006-context-defaults-restored.png", "A blank override restores harness defaults while new thresholds, unrelated config and existing sessions are preserved.", ["Context settings", "Leave blank to restore harness defaults.", "Existing sessions keep running."])
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: context loading, ordered validation, unsaved guard, stale refusal, exactu64 save and default restoration", flush=True)
finally:
    ws.close()
    x.close()
