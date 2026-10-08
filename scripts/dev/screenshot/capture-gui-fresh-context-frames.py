#!/usr/bin/env python3
"""Assert the native GUI sidebar through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
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
dbpath = pathlib.Path(sys.argv[4])

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
    raise AssertionError(expression + "\n" + str(evaluate("document.querySelector('.sidebar').innerText")))


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


def capture(name, note, expects, expression=None, allow_alert=False):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_alert:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    time.sleep(0.4)
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
    notes.append({"file": name, "note": note})
    print("PASS:", name, note, flush=True)


def sidebar_button(section, name):
    evaluate(f"Array.from(document.querySelector('.sb-{section}').querySelectorAll('button')).find(b=>b.textContent.trim()==={json.dumps(name)}).click()")


def button(name):
    evaluate(f"Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==={json.dumps(name)}).click()")


def edit_prompt(text):
    evaluate(f"""(()=>{{const el=document.querySelector('textarea[aria-label="Continuation prompt"]');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(text)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


try:
    window.configure(x=0, y=0, width=1440, height=960)
    x.sync()
    wait(nav_has("Round totals"))
    click_nav("Round totals")
    wait("!!document.querySelector('.sb-context button')")
    assert 'The TUI offers a fresh context here' not in evaluate('document.body.innerText')
    capture('001-fresh-context-action.png',
        'The native Context section offers Fresh context without a running TUI; the former TUI-only message is absent.',
        ['Claude Sidebar', 'Fresh context', 'Context is filling up.'])
    sidebar_button('context', 'Fresh context')
    wait("document.querySelector('textarea[aria-label=\"Continuation prompt\"]')?.value.includes('AMF_PLAN.md')")
    seed = evaluate("document.querySelector('textarea[aria-label=\"Continuation prompt\"]').value")
    assert 'Grill me' in seed
    capture('002-editable-continuation.png',
        'The editable continuation comes from the shared TUI builder, including the plan, feature summary and conservative clarification request. Nothing has launched.',
        ['Fresh context', 'Continuation prompt', 'Start fresh context', 'unsent composer draft'])
    edit_prompt(seed + '\nCheck negative invoice totals first.')
    button('Cancel')
    wait("!!document.querySelector('[aria-label=\"Discard continuation draft\"]')")
    capture('003-protected-draft.png',
        'Cancelling an edited prompt asks before discarding it; Keep editing retains the continuation and creates no session.',
        ['Discard your edited continuation prompt?', 'Discard draft', 'Keep editing'])
    button('Keep editing')
    # Switch the harness the new session would launch externally, then drive
    # the real conflict path.
    def shared_write(sql):
        with sqlite3.connect(dbpath) as db:
            db.execute(sql)
            db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    shared_write("UPDATE features SET agent='codex' WHERE id='f-round'")
    button('Start fresh context')
    wait("document.querySelector('[role=alert]')?.innerText.includes('source changed')")
    assert db_value("SELECT COUNT(*) FROM feature_sessions WHERE feature_id='f-round'") == 2
    capture('004-stale-source-refused.png',
        'Switching the feature harness through the shared database is refused before launch. The edited prompt remains for reload and review.',
        ['fresh-context source changed', 'Reload context (keep draft)'], allow_alert=True)
    # The fixture only has a Claude stand-in, so switch back before relaunching.
    shared_write("UPDATE features SET agent='claude' WHERE id='f-round'")
    button('Reload context (keep draft)')
    time.sleep(1)
    assert evaluate("document.querySelector('textarea[aria-label=\"Continuation prompt\"]').value") == seed + '\nCheck negative invoice totals first.'
    # Seed inputs moving while the source agent works must not refuse the draft.
    shared_write("UPDATE features SET summary='Updated rounding summary' WHERE id='f-round'")
    button('Start fresh context')
    wait("document.querySelector('.composer textarea')?.value.includes('Check negative invoice totals first.')")
    wait("document.querySelector('.xterm-rows')?.innerText.includes('Waiting for the next instruction')")
    assert db_value("SELECT COUNT(*) FROM feature_sessions WHERE feature_id='f-round'") == 3
    assert db_value("SELECT COUNT(*) FROM feature_sessions WHERE feature_id='f-round' AND label='Fresh Context'") == 1
    assert db_value("SELECT COUNT(*) FROM feature_sessions WHERE id='s-claude'") == 1
    evaluate("(()=>{const el=document.querySelector('.composer textarea');el.scrollTop=el.scrollHeight;})()")
    capture('005-fresh-session-unsent-draft.png',
        'A new Fresh Context tab runs only the offline stand-in. The edited continuation is selected as an unsent draft, while the original Claude and Codex tabs remain.',
        ['Fresh Context', 'Claude 1', 'Codex 1', 'Waiting for the next instruction'],
        "document.querySelector('.composer textarea').value.includes('Check negative invoice totals first.')")
    (out / 'native-notes.json').write_text(json.dumps(notes, indent=2))
finally:
    ws.close()
    x.close()
