#!/usr/bin/env python3
"""Assert native GUI terminal-scrolling states through WebKit IPC and capture its X11 window.

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



workdir = pathlib.Path(sys.argv[4])


def received(mode):
    return json.loads((workdir / f"{mode}-received.json").read_text())


def wheel(delta_y, selector=".term-surface"):
    # A trackpad/mouse wheel over the terminal. Dispatched on the container,
    # where TerminalPane's capture-phase listener routes the gesture; while
    # reading history, on xterm's own scrollable element, which scrolls its
    # scrollback natively.
    evaluate(
        f"""document.querySelector({json.dumps(selector)}).dispatchEvent(new WheelEvent('wheel',{{deltaY:{delta_y},deltaMode:0,bubbles:true,cancelable:true}}))"""
    )


def key(name, shift=False):
    # Keys reach xterm through its hidden textarea, as real typing does.
    evaluate(
        f"""(()=>{{const t=document.querySelector('.xterm-helper-textarea');t.focus();t.dispatchEvent(new KeyboardEvent('keydown',{{key:{json.dumps(name)},shiftKey:{'true' if shift else 'false'},bubbles:true,cancelable:true}}));}})()"""
    )


def rows_text():
    return evaluate("document.querySelector('.xterm-rows').innerText")


def fill(value):
    evaluate(f"""(()=>{{const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")
    wait(f"document.querySelector('.composer textarea')?.value==={json.dumps(value)}")


def tab(name):
    evaluate(f"Array.from(document.querySelectorAll('[role=tab]')).find(b=>b.textContent.trim()==={json.dumps(name)}).click()")
    refit()


def refit():
    # Give the freshly attached view a real resize event, as a person
    # resizing the capture window would, so tmux and xterm agree on rows.
    window.configure(height=800)
    x.sync()
    time.sleep(0.4)
    window.configure(height=780)
    x.sync()
    time.sleep(0.6)


OVERLAY = "document.querySelector('.term-scrollback')"
draft = "Once the run finishes, summarize which steps touched invoiceTotal."

try:
    window.configure(x=0, y=0, width=1180, height=780)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait("!!document.querySelector('.xterm-rows') && document.querySelector('.xterm-rows').innerText.includes('Waiting for the next instruction')")
    refit()
    wait("document.querySelector('.xterm-rows').innerText.includes('Waiting for the next instruction')")
    fill(draft)
    capture(
        "001-live-transcript.png",
        "Live view of a long agent transcript (offline fixture): only the latest steps fit; there is no scroll-back overlay.",
        ["step 120", "Waiting for the next instruction", "Send prompt"],
        f"!{OVERLAY} && !document.querySelector('.xterm-rows').innerText.includes('step 001')",
    )

    # Wheel up over the terminal: loads tmux history into xterm's scrollback.
    wheel(-600)
    wait(f"{OVERLAY}?.innerText.includes('Viewing earlier output')")
    # Then scroll further with the keyboard (PageUp while reading).
    key("PageUp")
    key("PageUp")
    time.sleep(0.5)
    wait("document.querySelector('.xterm-rows').innerText.includes('step 0')")
    assert received("transcript") == [], received("transcript")
    capture(
        "002-scrolled-back.png",
        "Wheel and PageUp scroll back through earlier output loaded from tmux history; the overlay says so and offers Jump to latest. The fixture received no input.",
        ["Viewing earlier output", "Jump to latest", "Esc"],
        f"!document.querySelector('.xterm-rows').innerText.includes('Waiting for the next instruction') && document.querySelector('.composer textarea').value==={json.dumps(draft)}",
    )
    before = rows_text()

    # New output arrives while the user reads (the fixture prints a burst on
    # its own; nothing is typed into it).
    (workdir / "transcript-more").write_text("")
    wait(f"{OVERLAY}?.innerText.includes('New output below')")
    time.sleep(0.6)
    assert not (workdir / "transcript-more").exists()
    assert rows_text() == before, "the reader's position moved under new output"
    assert received("transcript") == [], received("transcript")
    capture(
        "003-position-kept-under-new-output.png",
        "The fixture printed 30 new lines while the user was reading: the view stays exactly where it was and the overlay flags new output below.",
        ["Viewing earlier output", "New output below", "Jump to latest"],
        "!document.querySelector('.xterm-rows').innerText.includes('live 1.')",
    )

    click("Jump to latest")
    wait(f"!{OVERLAY} && document.querySelector('.xterm-rows').innerText.includes('Burst 1 done.')")
    time.sleep(0.4)
    assert received("transcript") == [], received("transcript")
    capture(
        "004-back-at-live.png",
        "Jump to latest returns to the live view, now showing the burst that arrived while reading. The unsent composer draft is unchanged and the fixture still received nothing.",
        ["live 1.30", "Burst 1 done.", "Send prompt"],
        f"!{OVERLAY} && document.querySelector('.composer textarea').value==={json.dumps(draft)}",
    )

    # Native xterm scrolling while reading: wheel back up, then wheel down on
    # xterm's own viewport until the end -- reaching the bottom resumes live.
    wheel(-300)
    wait(f"{OVERLAY}?.innerText.includes('Viewing earlier output')")
    for _ in range(60):
        if not evaluate(f"!!{OVERLAY}"):
            break
        wheel(400, ".xterm-screen")
        time.sleep(0.15)
    wait(f"!{OVERLAY} && document.querySelector('.xterm-rows').innerText.includes('Burst 1 done.')")
    assert received("transcript") == [], received("transcript")
    print("PASS: scrolling xterm's own viewport to the bottom resumed the live view", flush=True)

    tab("OpenCode")
    wait("!!document.querySelector('.xterm-rows') && document.querySelector('.xterm-rows').innerText.includes('full-screen program')")
    wait("document.querySelector('.xterm-rows').innerText.includes('wheel reports received: 0')")
    for _ in range(3):
        wheel(-51)
        time.sleep(0.15)
    wait("/wheel reports received: [1-9]/.test(document.querySelector('.xterm-rows').innerText)")
    time.sleep(0.5)
    reports = "".join(received("fullscreen"))
    assert reports and re.fullmatch(r"(\x1b\[<64;\d+;\d+M)+", reports), repr(reports)
    capture(
        "005-full-screen-program-wheel.png",
        "A full-screen program that asked for mouse reporting (an OpenCode-like fixture) scrolls its own view: the GUI forwarded wheel-up reports only, never keystrokes, and showed no history overlay.",
        ["full-screen program", "wheel reports received"],
        f"!{OVERLAY}",
    )
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: history scroll-back, retained position, jump to latest and full-screen wheel routing verified", flush=True)
finally:
    ws.close()
    x.close()
