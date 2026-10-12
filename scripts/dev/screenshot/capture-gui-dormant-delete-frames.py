#!/usr/bin/env python3
"""Assert native GUI dormancy states through WebKit IPC and capture its X11 window.

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
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
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


def capture(name, note, expects, expression=None, allow_error=False):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_error:
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


dbpath = pathlib.Path(sys.argv[4])
tmux = ["tmux", "-S", sys.argv[5]]

panel = "document.querySelector('[role=dialog][aria-label=\"Dormant features\"]')"
dialog = "document.querySelector('[role=dialog][aria-label=\"Delete feature\"]')"


def delete_row(name):
    wait(f"!!{panel} && !{panel}.innerText.includes('Checking tmux activity')")
    evaluate(f"{panel}.querySelector('button[aria-label=\"Delete {name}\"]').click()")
    wait(f"!!{dialog}")


def alive(name):
    return subprocess.run(tmux + ["has-session", "-t", f"amf-{name}"], stderr=subprocess.DEVNULL).returncode == 0


def preserved():
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT count(*) FROM features").fetchone() == (3,)
        assert db.execute("SELECT list_id FROM todos WHERE id='worktree-list-todo'").fetchone() == ("worktree-list",)
        assert db.execute("SELECT feature_id FROM todo_lists WHERE id='project-list'").fetchone() == ("billing-retry",)
    assert all(alive(name) for name in ["billing-retry", "docs-refresh", "ordinary-checkout"])


def todo_choices():
    click("Delete feature")
    wait(f"{dialog}?.innerText.includes('1 unfinished TODO')")
    evaluate("document.querySelector('input[value=move_to_global]').click()")
    click("Delete feature")
    wait(f"{dialog}?.innerText.includes('This feature hosts the project list')")
    evaluate("Array.from(document.querySelectorAll('label')).find(l=>l.textContent.trim()==='Keep on docs-refresh').querySelector('input').click()")


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Dormant features")')
    click("Dormant features")
    wait(f"{panel}?.querySelectorAll('.dormancy-row').length===3")
    capture("001-delete-list.png",
            "Each dormant row offers Delete without opening its feature. Three private sessions are running; the ordinary checkout is also listed.",
            ["Dormant features", "billing-retry", "docs-refresh", "ordinary-checkout", "Delete", "Stop selected (0)"],
            f"{panel}.querySelectorAll('button[aria-label^=\"Delete \"]').length===3")
    delete_row("billing-retry")
    preserved()
    capture("002-delete-confirm.png",
            "Deletion first asks for confirmation, warns that the worktree's uncommitted changes will be discarded, and explains fresh dormancy checks. No feature or TODO has changed.",
            ["Delete billing-retry?", "Uncommitted changes", "Each confirmation checks dormancy again", "Cancel", "Delete feature"])
    todo_choices()
    preserved()
    capture("003-todo-cancel.png",
            "Before deletion, the shared flow collects both the unfinished worktree TODO destination and a surviving host for the project list. Move to global and Keep on docs-refresh are selected; cancelling applies neither answer.",
            ["1 unfinished TODO", "Move them to the global list", "This feature hosts the project list", "Keep on docs-refresh", "Cancel"],
            "document.querySelector('input[value=move_to_global]').checked && Array.from(document.querySelectorAll('label')).find(l=>l.textContent.trim()==='Keep on docs-refresh').querySelector('input').checked")
    click("Cancel")
    wait(f"{panel}?.querySelectorAll('.dormancy-row').length===3")
    preserved()

    delete_row("docs-refresh")
    # Represent a second AMF interface opening this feature after confirmation.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE features SET last_accessed=? WHERE id='docs-refresh'", (time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),))
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    click("Delete feature")
    wait("document.body.innerText.includes('Not deleted:')")
    wait(f"{panel}?.querySelectorAll('.dormancy-row').length===2")
    preserved()
    capture("004-stale-refused.png",
            "An external open after confirmation makes the target stale. Rust refuses deletion, the refreshed list drops docs-refresh, and all three sessions and both TODOs remain intact.",
            ["Not deleted:", "It was opened in AMF after the list was loaded", "Dormant features", "billing-retry", "ordinary-checkout"],
            f"!{panel}.innerText.includes('docs-refresh') && !{dialog}", allow_error=True)
    evaluate("document.querySelector('.toast-error button[aria-label]').click()")
    wait("!document.querySelector('[role=alert]')")

    delete_row("billing-retry")
    todo_choices()
    click("Delete feature")
    wait(f"!{dialog} && {panel}?.querySelectorAll('.dormancy-row').length===1")
    assert not alive("billing-retry") and alive("docs-refresh") and alive("ordinary-checkout")
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT feature_id FROM todo_lists WHERE id='project-list'").fetchone() == ("docs-refresh",)
        assert db.execute("SELECT l.scope FROM todos t JOIN todo_lists l ON l.id=t.list_id WHERE t.id='worktree-list-todo'").fetchone() == ("global",)
    capture("005-worktree-deleted.png",
            "Confirmed deletion returns to Dormant features. billing-retry and its tmux session are gone, the worktree TODO has moved to global, and the project list now belongs to docs-refresh; the other sessions still run.",
            ["Feature deleted", "Dormant features", "ordinary-checkout"],
            f"!{panel}.innerText.includes('billing-retry')")
    delete_row("ordinary-checkout")
    capture("006-checkout-confirm.png",
            "An ordinary checkout gets the same explicit confirmation, with a promise that the repository checkout itself is left alone.",
            ["Delete ordinary-checkout?", "The repository checkout itself is left alone", "Each confirmation checks dormancy again"])
    click("Delete feature")
    wait(f"!{dialog} && {panel}?.innerText.includes('No running feature has been idle')")
    assert not alive("ordinary-checkout") and alive("docs-refresh")
    capture("007-checkout-kept.png",
            "The ordinary feature and its session are removed, leaving no dormant features. The runner independently verifies its Git checkout and uncommitted file still exist, the deleted worktree's branch is kept, and both TODOs survive.",
            ["Dormant features", "No running feature has been idle", "Feature deleted"])
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: cancellation, stale refusal, TODO disposition/host and both deletion kinds verified", flush=True)
finally:
    ws.close()
    x.close()
