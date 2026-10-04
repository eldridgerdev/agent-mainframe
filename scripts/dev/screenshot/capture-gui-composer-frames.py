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



repo = pathlib.Path(sys.argv[4])
draft = "Please update invoiceTotal to round consistently at cent boundaries.\n\nAdd coverage for negative totals and preserve Unicode: café 世界."
second = "Review the currency formatter and propose tests for other locales."

def fill(value):
    evaluate(f"""(()=>{{const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")
    wait(f"document.querySelector('.composer textarea')?.value==={json.dumps(value)}")

def tab(name):
    evaluate(f"Array.from(document.querySelectorAll('[role=tab]')).find(b=>b.textContent.trim()==={json.dumps(name)}).click()")
    wait("!!document.querySelector('.composer textarea') && Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Send prompt')")
    refit()

def refit():
    # Give the freshly attached view a real resize event, as a person
    # resizing the capture window would, so tmux and xterm agree on rows.
    window.configure(height=800)
    x.sync()
    time.sleep(0.4)
    window.configure(height=780)
    x.sync()
    time.sleep(0.4)

try:
    window.configure(x=0, y=0, width=1180, height=780)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('!!document.querySelector(".composer textarea")')
    fill(draft)
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Send prompt" && !b.disabled)')
    refit()
    assert json.loads((repo / "claude-received.json").read_text()) == []
    capture("001-compose-locally.png", "The composer accepts a multiline prompt locally; the attached test harness has received nothing.", ["Compose prompt", "Send prompt", "Enter", "Ctrl/Cmd", "Ready. Nothing is sent while you compose in the GUI."], f"document.querySelector('.composer textarea').value==={json.dumps(draft)}")
    tab("Codex")
    wait("document.querySelector('.composer textarea').value===''")
    fill(second)
    assert json.loads((repo / "codex-received.json").read_text()) == []
    capture("002-separate-session-draft.png", "The Codex tab has its own draft; switching tabs sends neither message.", ["Compose prompt", "Codex", "Send prompt"], f"document.querySelector('.composer textarea').value==={json.dumps(second)}")
    tab("Claude")
    wait(f"document.querySelector('.composer textarea').value==={json.dumps(draft)}")
    evaluate("(()=>{const el=document.querySelector('.composer textarea');el.focus();el.select();})()")
    capture("003-draft-restored.png", "Returning to Claude restores its original multiline draft unchanged.", ["Compose prompt", "Claude", "Send prompt"], f"document.querySelector('.composer textarea').value==={json.dumps(draft)}")
    click("Send prompt")
    wait("document.querySelector('.composer textarea').value===''")
    for _ in range(100):
        if json.loads((repo / "claude-received.json").read_text()) == [draft]:
            break
        time.sleep(0.1)
    else:
        print("Received fixture payload:", (repo / "claude-received.json").read_text(), flush=True)
        raise AssertionError("The harness did not receive exactly one complete multiline prompt")
    assert json.loads((repo / "codex-received.json").read_text()) == []
    time.sleep(0.5)
    capture("004-sent-and-ready.png", "Send prompt delivers exactly one complete multiline message through real tmux IPC, clears only Claude's draft, and leaves the composer ready for another message.", ["Compose prompt", "Send prompt", "Received one complete prompt:", "Ready for the next prompt."], "document.querySelector('.composer textarea').value==='' && Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Send prompt' && b.disabled)")
    tab("Codex")
    wait(f"document.querySelector('.composer textarea').value==={json.dumps(second)}")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: Both session identities, unsent drafts and exact multiline delivery verified", flush=True)
finally:
    ws.close()
    x.close()
