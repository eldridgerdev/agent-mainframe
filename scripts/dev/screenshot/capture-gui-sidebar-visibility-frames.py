#!/usr/bin/env python3
"""Assert independent GUI sidebar visibility through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
import pathlib
import re
import sqlite3
import subprocess
import sys
import time
import urllib.request

import websocket
from Xlib import display

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
dbpath = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"

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
ws = websocket.create_connection(f"ws://{sys.argv[3]}{inspector_path}", timeout=15, suppress_origin=True)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def evaluate(expression):
    global seq
    seq += 1
    request = {"id": seq, "method": "Runtime.evaluate", "params": {"expression": expression, "returnByValue": True}}
    ws.send(json.dumps({"id": seq + 10000, "method": "Target.sendMessageToTarget",
                        "params": {"targetId": target, "message": json.dumps(request)}}))
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                assert not message["result"].get("wasThrown"), message
                return message["result"]["result"].get("value")


def wait(expression, tries=200):
    for _ in range(tries):
        if evaluate(expression):
            return
        time.sleep(0.1)
    raise AssertionError(expression + "\n" + str(evaluate("document.body.innerText")))


NAV = "document.querySelector('nav[aria-label=Workspace]')"


def nav_has(text):
    return f"({NAV}?.innerText ?? '').includes({json.dumps(text)})"


def click_nav(name):
    """Click a tree button by its exact accessible text or aria-label."""
    evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('button')).find(b=>(b.getAttribute('aria-label')||b.textContent.trim())==={json.dumps(name)});if(!b)throw new Error('no button '+{json.dumps(name)});b.click();}})()""")


def db_value(sql):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql).fetchone()[0]


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
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    # Force native WebKit to repaint before saving the asserted DOM state.
    window.configure(width=1279)
    x.sync()
    time.sleep(0.5)
    window.configure(width=1280)
    x.sync()
    time.sleep(1.5)
    g = window.get_geometry()
    subprocess.run(
        ["/usr/bin/python3", "-c",
         """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
         str(window.id), str(g.width), str(g.height), str(out / name)],
        check=True,
    )
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name.replace(".png", ".ansi"), "note": note})
    print("PASS:", name, note, flush=True)


def click(label):
    evaluate(f"""(()=>{{const b=document.querySelector('button[aria-label={json.dumps(label)}]');if(!b)throw new Error('Missing '+{json.dumps(label)});b.click();}})()""")


def assert_layout(projects, agent):
    wait(f"document.querySelector('.sidebar').classList.contains('sidebar-collapsed') === {str(not projects).lower()}")
    wait(f"document.querySelector('.agent-sidebar').classList.contains('agent-sidebar-collapsed') === {str(not agent).lower()}")
    wait("document.querySelector('.term-surface').getBoundingClientRect().width > 0")
    assert evaluate("document.querySelector('.term-surface') === window.proofTerminal")
    assert evaluate("document.querySelector('textarea[aria-label=\"Draft prompt\"]') === window.proofDraft")
    assert evaluate("window.proofDraft.value === 'Keep this draft while toggling sidebars.'")
    assert evaluate(f"localStorage.getItem('amf.gui.collapsed.projectsSidebar') === '{0 if projects else 1}'")
    assert evaluate(f"localStorage.getItem('amf.gui.collapsed.sessionSidebar') === '{0 if agent else 1}'")
    return evaluate("document.querySelector('.term-surface').getBoundingClientRect().width")


try:
    window.configure(x=0, y=0, width=1280, height=960)
    x.sync()
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE projects SET collapsed=0")
        db.execute("UPDATE features SET collapsed=0 WHERE id='shot-feature'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    wait(nav_has("Round invoice totals"))
    # Clear only this throwaway window's viewer preferences.
    evaluate("localStorage.setItem('amf.gui.collapsed.projectsSidebar','0'); localStorage.setItem('amf.gui.collapsed.sessionSidebar','0'); window.proofReloadPending=true; location.reload()")
    wait("typeof window.proofReloadPending === 'undefined' && " + nav_has("Round invoice totals"))
    click_nav("Round invoice totals")
    wait("!!document.querySelector('.agent-sidebar-head') && !!document.querySelector('.composer textarea')")
    wait("!!document.querySelector('textarea[aria-label=\"Draft prompt\"]')")
    evaluate("""(()=>{window.proofTerminal=document.querySelector('.term-surface');window.proofDraft=document.querySelector('textarea[aria-label="Draft prompt"]');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(window.proofDraft,'Keep this draft while toggling sidebars.');window.proofDraft.dispatchEvent(new Event('input',{bubbles:true}));})()""")
    wait("Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Send prompt' && !b.disabled)")
    # Use buttons first so both expanded preferences are explicitly persisted.
    click("Hide projects sidebar")
    click("Show projects sidebar")
    click("Hide agent sidebar")
    click("Show agent sidebar")
    narrow = assert_layout(True, True)
    capture("001-both-sidebars-open.png", "Both sidebars are open. Each has its own hide button; the terminal and unsent prompt sit between them.", ["Invoice API", "Round invoice totals", "Claude Sidebar"], "!!document.querySelector('.agent-sidebar-head') && !document.querySelector('.sidebar-content').hidden")

    click("Hide projects sidebar")
    width_projects_hidden = assert_layout(False, True)
    assert width_projects_hidden > narrow + 200
    capture("002-projects-hidden.png", "Hiding projects leaves a thin restore rail on the left. The agent sidebar stays open and the same terminal gains width without losing the draft.", ["Round invoice totals", "Claude Sidebar"], "document.querySelector('.sidebar-content').hidden && !!document.querySelector('button[aria-label=\"Show projects sidebar\"]')")

    # Shortcuts originate outside terminal and editable content.
    evaluate("document.activeElement.blur(); window.dispatchEvent(new KeyboardEvent('keydown',{key:'A',code:'KeyA',altKey:true,shiftKey:true,bubbles:true,cancelable:true}))")
    width_both_hidden = assert_layout(False, False)
    assert width_both_hidden > width_projects_hidden + 200
    capture("003-both-sidebars-hidden.png", "Both sidebars are hidden, leaving restore rails on both edges. Alt+Shift+A hides the agent sidebar independently, freeing the most terminal width.", ["Round invoice totals"], "!!document.querySelector('button[aria-label=\"Show projects sidebar\"]') && !!document.querySelector('button[aria-label=\"Show agent sidebar\"]')")

    evaluate("window.dispatchEvent(new KeyboardEvent('keydown',{key:'P',code:'KeyP',altKey:true,shiftKey:true,bubbles:true,cancelable:true}))")
    width_agent_hidden = assert_layout(True, False)
    assert width_agent_hidden > narrow + 200
    capture("004-agent-hidden.png", "Alt+Shift+P restores projects while the agent sidebar remains hidden. Both layout choices are saved independently; the terminal and unsent draft remain mounted.", ["Invoice API", "Round invoice totals"], "!document.querySelector('.sidebar-content').hidden && !!document.querySelector('button[aria-label=\"Show agent sidebar\"]')")
    # A fresh frontend mount adopts the two stored choices.
    evaluate("window.proofReloadPending=true; location.reload()")
    wait("typeof window.proofReloadPending === 'undefined' && " + nav_has("Round invoice totals"))
    wait("!!document.querySelector('button[aria-label=\"Hide projects sidebar\"]')")
    click_nav("Round invoice totals")
    wait("!!document.querySelector('button[aria-label=\"Show agent sidebar\"]')")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(n)+"\n" for n in notes))
    print("PASS: all four combinations, terminal identity/width, retained draft, shortcuts and reloaded viewer preferences", flush=True)
finally:
    ws.close()
    x.close()
