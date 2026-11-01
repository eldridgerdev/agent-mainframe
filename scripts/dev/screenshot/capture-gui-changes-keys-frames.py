#!/usr/bin/env python3
"""Assert native Changes keyboard-navigation states through WebKit IPC and capture its X11 window.

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



def set_filter(value):
    evaluate(
        f"""(()=>{{const el=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==='Filter files').querySelector('input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()"""
    )


def key(name, target="document.body"):
    # Synthetic (untrusted) events: they reach the same window listener a real key does.
    evaluate(
        f"{target}.dispatchEvent(new KeyboardEvent('keydown',{{key:{json.dumps(name)},bubbles:true,cancelable:true}}))"
    )
    time.sleep(0.2)


config_file = pathlib.Path(sys.argv[5]) / "amf" / "config.json"
cursor = 'document.querySelector(".diff-cursor")?.innerText||""'
selected = 'document.querySelector(".diff-file-header strong")?.innerText'
nav = 'document.querySelector("nav[aria-label=\'Changed files\']")'

try:
    window.configure(width=1400, height=900)
    x.sync()
    time.sleep(0.5)
    wait(
        'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")'
    )
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped")')
    click("Changes")
    wait('!!document.querySelector(".diff-file")')
    click("Keyboard shortcuts")
    capture(
        "001-shortcuts-help.png",
        "The Changes view lists its keyboard shortcuts in an expandable help panel.",
        ["Keyboard shortcuts", "Next hunk (desktop only)", "Switch unified / side by side (remembered)"],
        'document.querySelector("#diff-key-help")?.getAttribute("aria-label")==="Keyboard shortcuts"',
    )
    key("?")
    assert evaluate('!document.querySelector("#diff-key-help")'), "? did not close help"
    assert "notes.md" in evaluate(selected), evaluate(selected)
    for _ in range(4):
        key("j")
    capture(
        "002-j-walks-tree-rows.png",
        "j moves the dashed cursor down the tree; landing on a file opens it in the diff pane.",
        ["notes.md"],
        f"{cursor}.length>0 && {selected}.length>0",
    )
    chosen = evaluate(selected)
    key("g")
    key("k")
    row = evaluate(cursor)
    assert row.startswith("▾") or row.startswith("▸"), row
    key("z")
    capture(
        "003-z-folds-folder.png",
        "z folds the folder under the cursor; the open file stays selected.",
        ["docs/", "notes.md"],
        f'{nav}.querySelector(".diff-cursor").getAttribute("aria-expanded")==="false"',
    )
    key("z")
    key("G")
    last = evaluate(selected)
    key("g")
    assert evaluate(selected) != last
    key("v")
    wait('!!document.querySelector("table.diff-split")')
    saved = json.loads(config_file.read_text())
    assert saved["diff_viewer_layout"] == "side-by-side", saved
    capture(
        "004-v-side-by-side-saved.png",
        "v switches to side by side and writes diff_viewer_layout=side-by-side to the shared config.",
        ["Layout"],
        '!!document.querySelector("table.diff-split")',
    )
    key(".")
    capture(
        "005-dot-shows-amf-group.png",
        ". opens the AMF files group beneath the reviewer's own files.",
        ["AMF files", "change-tracker.js"],
        f'{nav}.innerText.includes("change-tracker.js")',
    )
    layout_before = json.loads(config_file.read_text())["diff_viewer_layout"]
    evaluate('document.querySelector("input[type=checkbox]").focus()')
    key("v", "document.activeElement")
    key("W", "document.activeElement")
    assert json.loads(config_file.read_text())["diff_viewer_layout"] == layout_before
    assert evaluate('!document.querySelector("input[type=checkbox]").checked'), "W fired in a field"
    key("W")
    wait('document.querySelector("input[type=checkbox]").checked')
    capture(
        "006-w-ignore-whitespace-guard.png",
        "Keys pressed while a form control (the Ignore whitespace checkbox) has focus are ignored and the layout is unchanged; W on the page turns on Ignore whitespace.",
        ["Ignore whitespace"],
        'document.querySelector("input[type=checkbox]").checked',
    )
    (out / "capture-notes.jsonl").write_text(
        "".join(
            json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]})
            + "\n"
            for n in notes
        )
    )
finally:
    ws.close()
    x.close()
