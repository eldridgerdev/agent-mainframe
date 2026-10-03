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


try:
    window.configure(width=1100, height=720)
    x.sync()
    time.sleep(0.5)
    wait(
        'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")'
    )
    click("Round invoice totals")
    wait(
        'document.body.innerText.includes("Stopped") && document.body.innerText.includes("Changes")'
    )
    capture(
        "001-feature-controls.png",
        "A stopped Git feature offers Changes without starting an agent.",
        ["Stopped", "Changes", "Round invoice totals"],
    )
    window.configure(width=1400, height=900)
    x.sync()
    time.sleep(0.5)
    click("Changes")
    wait('!!document.querySelector(".diff-file")')
    evaluate(
        'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==="invoice.ts").click()'
    )
    capture(
        "002-unified-current-changes.png",
        "Current changes combine committed rounding and uncommitted currency formatting, with file and hunk controls.",
        [
            "Changes · Round invoice totals",
            "4 files",
            "Math.round(total * 100) / 100",
            "Intl.NumberFormat",
        ],
        'document.querySelectorAll(".diff-lines:not(.diff-split)").length>0',
    )
    select("Layout", "split")
    capture(
        "003-side-by-side.png",
        "Side-by-side layout aligns the base and current source lines.",
        ["Math.round(total * 100) / 100", "Intl.NumberFormat"],
        'document.querySelectorAll(".diff-split").length>0',
    )
    select("Context", "full")
    capture(
        "004-whole-file-context.png",
        "Whole-file context adds the surrounding invoice type and formatting function.",
        ["export type Invoice", "export function formatTotal", "Intl.NumberFormat"],
        'Array.from(document.querySelectorAll("select")).some(s=>s.value==="full")',
    )
    hash = evaluate(
        'Array.from(document.querySelectorAll("label")).find(l=>l.querySelector("span")?.textContent==="Diff scope").querySelector("select").options[1].value'
    )
    select("Diff scope", hash)
    capture(
        "005-single-commit.png",
        "Selecting the feature commit shows its rounding change alone and excludes the uncommitted currency formatter.",
        ["1 files", "Math.round(total * 100) / 100", "return total.toFixed(2)"],
        '!document.body.innerText.includes("Intl.NumberFormat")',
    )
    select("Diff scope", "")
    select("Context", "standard")
    evaluate(
        'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==="receipt.bin").click()'
    )
    capture(
        "006-binary-file.png",
        "Binary changes have an explicit notice rather than an empty text diff.",
        ["receipt.bin", "Binary file changed; no text diff is available."],
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
