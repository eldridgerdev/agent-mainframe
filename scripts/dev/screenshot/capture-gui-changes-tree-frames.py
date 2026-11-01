#!/usr/bin/env python3
"""Assert native Changes file-tree states through WebKit IPC and capture its X11 window.

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


def click_row(label):
    evaluate(
        'Array.from(document.querySelectorAll("nav[aria-label=\'Changed files\'] button")).find(b=>b.getAttribute("aria-label")==='
        + json.dumps(label)
        + ").click()"
    )


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
    nav = 'document.querySelector("nav[aria-label=\'Changed files\']")'
    evaluate(
        'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.title==="src/billing/tax.ts").click()'
    )
    capture(
        "001-tree-amf-folded.png",
        "Changed files form a folder tree; AMF-generated files sit in a folded group, as in the TUI.",
        ["src/", "billing/", "AMF files", "generated by AMF", "tax.ts"],
        f'{nav}.querySelector("[aria-label=\'AMF files group\']").getAttribute("aria-expanded")==="false" && !{nav}.innerText.includes("change-tracker.js")',
    )
    click_row("Folder src/billing")
    capture(
        "002-folder-folded-selection-kept.png",
        "Folding a folder hides its rows while the selected file stays open in the diff pane.",
        ["src/billing/tax.ts", "ui/"],
        f'!{nav}.innerText.includes("round.ts") && document.querySelector(".diff-file-header strong").innerText==="src/billing/tax.ts"',
    )
    set_filter("plan")
    capture(
        "003-filter-finds-amf-file.png",
        "Filtering searches inside folded folders and the folded AMF group, showing only matching files.",
        ["AMF files", "plan.txt"],
        f'{nav}.innerText.includes("plan.txt") && !{nav}.innerText.includes("tax.ts")',
    )
    set_filter("")
    click("Show AMF files (2)")
    capture(
        "004-amf-group-shown.png",
        "Show AMF files expands the group beneath the reviewer's own files; the folded folder stays folded after the filter clears.",
        ["AMF files", "change-tracker.js", "plan.txt", "Hide AMF files (2)"],
        f'{nav}.innerText.includes("change-tracker.js")',
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
