#!/usr/bin/env python3
"""Assert native GUI TODO editing states through WebKit IPC and capture its X11 window.

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
    raise AssertionError(f"{expression}: {evaluate('document.body.innerText')}")


def click(text):
    button = f'Array.from(document.querySelectorAll("button")).find(b=>b.offsetParent!==null && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)}))'
    wait(f"!!({button}) && !({button}).disabled")
    evaluate(f"({button}).click()")


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


def capture(name, note, expects, expression=None, allow_error=False):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_error:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        wait(expression)
    time.sleep(0.4)
    for attempt in range(6):
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
        if digest not in captured_frames:
            break
        time.sleep(0.5)  # WebKit may not have repainted yet
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)


dbpath = pathlib.Path(sys.argv[4])


def rows(sql, values=()):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql, values).fetchall()


def set_value(selector, value):
    evaluate(f'''(()=>{{const e=document.querySelector({json.dumps(selector)});const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()''')


def row_action(title, item):
    evaluate(f'document.querySelector(\'button[aria-label="Actions for {title}"]\').click()')
    wait(f'Array.from(document.querySelectorAll("[role=menuitem]")).some(e=>e.textContent.trim().startsWith({json.dumps(item)}))')
    evaluate(f'Array.from(document.querySelectorAll("[role=menuitem]")).find(e=>e.textContent.trim().startsWith({json.dumps(item)})).click()')


dialog = "document.querySelector('[role=dialog]')"
panel = "document.querySelector('.todo-panel[aria-label=\"Project TODOs\"]')"
WORK = ("SELECT status,agent_session_id,linked_feature_id FROM todos WHERE id='work'", [("in_progress", "billing-retry-claude", None)])

try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    # TODO tables sit outside the store; seed them only after the GUI has loaded
    # and settled on the feature so its own store sync cannot race the session row.
    wait('document.body.innerText.includes("billing-retry")')
    time.sleep(3)
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT id FROM feature_sessions").fetchall() == [("billing-retry-claude",)]
        stamp = time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime())
        db.execute("INSERT INTO todo_lists(id,project_id,feature_id,scope,workdir,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
                   ("project-list", "shot-project", "billing-retry", "project", None, stamp, stamp))
        for order, (todo_id, title, body, status, session, linked) in enumerate([
            ("work", "Wire retry backoff", "Cap at five attempts.", "in_progress", "billing-retry-claude", None),
            ("doc", "Document backoff", "Describe the default delays.", "not_started", None, None),
            ("dead", "Port to v2", None, "not_started", None, "ghost-feature"),
        ]):
            db.execute("INSERT INTO todos(id,list_id,title,body,status,agent_session_id,linked_feature_id,sort_order,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                       (todo_id, "project-list", title, body, status, session, linked, order, stamp, stamp))
    click("Billing API")
    wait(f"!!{panel} && {panel}.querySelectorAll('.todo').length===3")
    capture("001-todo-list.png",
            "The project TODO list shows each item's notes, an in-progress item and one with a stale planned-feature link each offering Go to work, and an empty scratchpad. All three items come from the shared database.",
            ["Project TODOs", "Wire retry backoff", "Cap at five attempts.", "Document backoff", "Go to work", "Scratchpad", "Add note"],
            f"{panel}.querySelectorAll('button[aria-label^=\"Go to the work for\"]').length===2")

    # Edit, with the TODO changed from another interface before saving.
    row_action("Document backoff", "Edit")
    wait(f"!!{dialog}")
    set_value('[aria-label="TODO title"]', "Document retry delays")
    set_value('[aria-label="TODO notes"]', "Describe defaults and the five-attempt cap.")
    evaluate(f"Array.from({dialog}.querySelectorAll('[role=radio]')).find(e=>e.textContent.trim()==='High').click()")
    capture("002-edit-dialog.png",
            "Editing opens title, notes and priority together. High is selected and Save is enabled; nothing is written until Save.",
            ["Edit TODO", "Title", "Notes", "Priority", "Save"],
            f"{dialog}.querySelector('[aria-label=\"TODO title\"]').value==='Document retry delays'")
    assert rows("SELECT title,priority FROM todos WHERE id='doc'") == [("Document backoff", "med")]
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE todos SET title='Document backoff (TUI edit)', priority='low', updated_at=datetime('now','+1 second') WHERE id='doc'")
    click("Save")
    wait(f"{dialog}?.innerText.includes('changed since you opened it')")
    assert rows("SELECT title,priority FROM todos WHERE id='doc'") == [("Document backoff (TUI edit)", "low")]
    capture("003-stale-refused.png",
            "Another interface changed the TODO first. Save is refused, the stored title and priority are shown, my text is kept, and nothing was overwritten.",
            ["This TODO changed since you opened it", "Document backoff (TUI edit)", "Discard my edits and use this", "Overwrite with my edits"],
            f"{dialog}.querySelector('[aria-label=\"TODO title\"]').value==='Document retry delays'", allow_error=True)
    click("Overwrite with my edits")
    wait(f"!{dialog}")
    assert rows("SELECT title,body,priority FROM todos WHERE id='doc'") == [("Document retry delays", "Describe defaults and the five-attempt cap.", "high")]
    assert rows(WORK[0]) == WORK[1]
    capture("004-edit-saved.png",
            "After the explicit overwrite the list shows the new title, notes and High tag. The in-progress item's status and session link are unchanged in the database.",
            ["Document retry delays", "Describe defaults and the five-attempt cap.", "High"],
            f"!{panel}.innerText.includes('(TUI edit)')")

    # Scratchpad.
    click("Add note")
    wait(f"!!{dialog}")
    set_value('[aria-label="Scratchpad text"]', "Left off: backoff constants")
    click("Save")
    wait(f"!{dialog} && {panel}.innerText.includes('Left off: backoff constants')")
    assert rows("SELECT carry_over FROM todo_lists WHERE id='project-list'") == [("Left off: backoff constants",)]
    capture("005-scratchpad-saved.png",
            "The scratchpad note is saved on the shared project list and shown under the header.",
            ["Scratchpad", "Left off: backoff constants", "Edit"],
            f"!{dialog}")
    click("Edit")
    wait(f"!!{dialog}")
    set_value('[aria-label="Scratchpad text"]', "Mine")
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE todo_lists SET carry_over='Left off: TUI note' WHERE id='project-list'")
    click("Save")
    wait(f"{dialog}?.innerText.includes('scratchpad changed since you opened it')")
    assert rows("SELECT carry_over FROM todo_lists WHERE id='project-list'") == [("Left off: TUI note",)]
    capture("006-scratchpad-stale.png",
            "The scratchpad was changed elsewhere. The save is refused and the stored text is shown; my draft stays in the editor.",
            ["The scratchpad changed since you opened it", "Left off: TUI note", "Overwrite with my edits"],
            f"{dialog}.querySelector('textarea').value==='Mine'", allow_error=True)
    click("Discard my edits and use this")
    click("Cancel")
    wait(f"!{dialog}")
    assert rows("SELECT carry_over FROM todo_lists WHERE id='project-list'") == [("Left off: TUI note",)]

    # A dead feature link is cleared with an explanation; status is untouched.
    evaluate("document.querySelector('.toast-error button[aria-label]')?.click()")
    assert rows("SELECT status,linked_feature_id FROM todos WHERE id='dead'") == [("not_started", "ghost-feature")]
    evaluate(f"{panel}.querySelector('button[aria-label=\"Go to the work for Port to v2\"]').click()")
    wait("document.body.innerText.includes('link was cleared')")
    assert rows("SELECT status,linked_feature_id FROM todos WHERE id='dead'") == [("not_started", None)]
    capture("007-dead-link-cleared.png",
            "Go to work on a TODO whose planned feature no longer exists says so and clears the dead link. The item stays not started, and the button is gone.",
            ["The feature planned for this TODO no longer exists", "Port to v2"],
            f"!{panel}.querySelector('button[aria-label=\"Go to the work for Port to v2\"]')", allow_error=True)
    evaluate("document.querySelectorAll('.toast-error button[aria-label]').forEach(b=>b.click())")

    # Jump to the live session.
    evaluate(f"{panel}.querySelector('button[aria-label=\"Go to the work for Wire retry backoff\"]').click()")
    wait("!!document.querySelector('[role=tab][aria-selected=true]') && document.body.innerText.includes('billing-retry')")
    capture("008-jumped-to-session.png",
            "Go to work leaves the project page for the feature that holds the TODO's agent session, with that session's tab selected. Nothing was launched and the TODO's status is unchanged.",
            ["billing-retry", "Claude"],
            "document.querySelector('[role=tab][aria-selected=true]').textContent.includes('Claude')")
    assert rows(WORK[0]) == WORK[1]
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: edit, stale refusal, scratchpad, dead-link heal and jump verified", flush=True)
finally:
    ws.close()
    x.close()
