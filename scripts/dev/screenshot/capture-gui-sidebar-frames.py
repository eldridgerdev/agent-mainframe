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


def click_session(label):
    evaluate(f"""Array.from({NAV}.querySelectorAll('.tree-session-label')).find(l=>l.textContent==={json.dumps(label)}).closest('button').click()""")


def click(text):
    evaluate(f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()')


def glyph(feature):
    return evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('.tree-name')).find(b=>b.textContent.trim()==={json.dumps(feature)});return b.closest('.tree-row').querySelector('.tree-glyph').getAttribute('aria-label');}})()""")


def marker(feature):
    return evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('.tree-name')).find(b=>b.textContent.trim()==={json.dumps(feature)});return !!b.closest('.tree-row').querySelector('.tree-pending');}})()""")


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


try:
    window.configure(x=0, y=0, width=1280, height=960)
    x.sync()
    # The shared session-status collector (usage, context, custom status) and
    # the offline PR sweep both run in this GUI process on their own threads.
    wait(nav_has("Round totals"))
    wait(nav_has("PR #321 · 2 open"))
    wait(nav_has("Ctx ~74% WARNING"))
    wait(nav_has("listening on :5173"))
    assert glyph("Round totals") == "Agent working", glyph("Round totals")
    assert glyph("fix-rounding-bug") == "Waiting for input" and marker("fix-rounding-bug")
    # A supervised-edit review waits, but is not the TUI's `?` request marker.
    assert glyph("supervised-edit") == "Waiting for input" and not marker("supervised-edit")
    assert glyph("release-prep") == "Ready"
    assert glyph("closed-experiment") == "Stopped"
    capture(
        "001-tree-overview.png",
        "The sidebar shows the TUI tree's information: shortened repo paths, nickname with branch, `repo`, issue source, "
        "open/merged/closed PR badges from the shared sweep and SQLite, usage, mode/review/plan badges, ages, session "
        "counts, stopped sessions, the thinking/waiting/ready/stopped glyphs, the `?` request marker and the AI summary. "
        "Round totals is expanded from its persisted collapse flag to show its sessions.",
        ["~/code/invoice-api", "(round-totals)", "github.com/acme/invoice-api#42", "PR #321 · 2 open",
         "PR #12 merged", "PR #7 closed", "supervibe", "review", "plan", "1 stopped", "6 sessions",
         "— Investigating rounding for negative totals", "No features yet. Add one", "repo"],
        f"{NAV}.innerText.includes('Sandbox') && !{NAV}.innerText.includes('landing-copy')",
    )

    # Session rows: kind icons, run state, context bands, status text, and the
    # long label truncating before its critical context indicator.
    long_label = "Claude 2 · refactor currency formatting across every invoice view"
    truncation = f"""(()=>{{const l=Array.from({NAV}.querySelectorAll('.tree-session-label')).find(e=>e.textContent==={json.dumps(long_label)});const c=l.nextElementSibling;return l.scrollWidth>l.clientWidth && l.clientWidth>=40 && c.textContent==='Ctx ~91% CRITICAL' && c.title.startsWith('Ctx ~91% CRITICAL · ') && c.scrollWidth<=c.clientWidth+1 && c.className.includes('ctx-critical');}})()"""
    wait(truncation)
    assert evaluate(f"Array.from({NAV}.querySelectorAll('.tree-session-label')).every(l=>l.clientWidth>=40)")
    click_session("Shell")
    wait("Array.from(document.querySelectorAll('[role=tab]')).some(t=>t.textContent.includes('Shell') && t.getAttribute('aria-selected')==='true')")
    capture(
        "002-session-row-opens-tab.png",
        "Session rows carry their kind icon, running/stopped state (Codex 1 was stopped on its own), the agent context "
        "indicator with its warning/critical band, and the status line. The long Claude 2 label truncates before its "
        "critical indicator. Selecting the Shell row opened that session's tab.",
        ["Ctx ~74% WARNING", "Ctx ~91% CRITICAL", "listening on :5173", "Codex 1", "Dev server", "TODOs"],
        f"{NAV}.querySelector('.tree-session-active')?.textContent.includes('Shell')",
    )

    click_nav("Collapse Invoice API")
    wait(f"!{NAV}.innerText.includes('Round totals')")
    for _ in range(50):
        if db_value("SELECT collapsed FROM projects WHERE id='p-invoice'") == 1:
            break
        time.sleep(0.1)
    assert db_value("SELECT collapsed FROM projects WHERE id='p-invoice'") == 1
    click_nav("Expand Docs site")
    wait(nav_has("landing-copy"))
    for _ in range(50):
        if db_value("SELECT collapsed FROM projects WHERE id='p-docs'") == 0:
            break
        time.sleep(0.1)
    assert db_value("SELECT collapsed FROM projects WHERE id='p-docs'") == 0
    capture(
        "003-collapse-persisted.png",
        "Collapsing Invoice API and expanding Docs site writes Project::collapsed to the shared SQLite store (asserted "
        "in the database), so the TUI tree opens the same way.",
        ["Invoice API", "Docs site", "landing-copy"],
        f"!{NAV}.innerText.includes('Round totals')",
    )

    # A TUI process toggles rows: write the store as it would, bumping the
    # version the GUI polls for.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE projects SET collapsed=0 WHERE id='p-invoice'")
        db.execute("UPDATE projects SET collapsed=1 WHERE id='p-docs'")
        db.execute("UPDATE features SET collapsed=1 WHERE id='f-round'")
        db.execute("UPDATE features SET collapsed=0 WHERE id='f-release'")
        version = int(db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()[0])
        db.execute("UPDATE store_meta SET value=? WHERE key='store_version'", (version + 1,))
    wait(f"{nav_has('Round totals')} && !{nav_has('landing-copy')} && !{nav_has('Ctx ~74% WARNING')}")
    wait(f"Array.from({NAV}.querySelectorAll('.tree-session-label')).some(l=>l.textContent==='Shell' && l.closest('.tree-feature').textContent.includes('release-prep'))")
    capture(
        "004-tui-change-adopted.png",
        "Collapse changes another AMF process writes to the shared store reach the open GUI on its next poll: Invoice API "
        "is expanded again, Round totals is collapsed over its sessions, release-prep shows its stopped sessions, and "
        "Docs site is collapsed.",
        ["Round totals", "release-prep"],
        f"!{NAV}.innerText.includes('landing-copy')",
    )

    click_nav("fix-rounding-bug")
    wait("Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Plan')")
    click("Plan")
    wait("Array.from(document.querySelectorAll('[role=menuitem]')).some(b=>b.textContent.includes('Quick Plan'))")
    evaluate("Array.from(document.querySelectorAll('[role=menuitem]')).find(b=>b.textContent.includes('Quick Plan')).click()")
    wait("!!document.querySelector('[role=dialog]')")
    click("Minimize")
    wait(nav_has("plan paused · Resume"))
    capture(
        "005-plan-paused.png",
        "A minimized Quick Plan is this window's parked interview: the feature row says `plan paused · Resume`. No AI "
        "call is made before the interview's own consent step.",
        ["plan paused · Resume", "fix-rounding-bug"],
        "!!document.querySelector('[role=dialog]')?.closest('[hidden]')",
    )
    click_nav("plan paused · Resume")
    wait("!!document.querySelector('[role=dialog]') && !document.querySelector('[role=dialog]').closest('[hidden]')")
    capture(
        "006-plan-resumed.png",
        "Resume on the row reopens the same interview where it was left.",
        ["Quick Plan", "fix-rounding-bug"],
    )

    (out / "capture-notes.jsonl").write_text("".join(
        json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: sidebar parity, collapse round trip and plan resume verified", flush=True)
finally:
    ws.close()
    x.close()
