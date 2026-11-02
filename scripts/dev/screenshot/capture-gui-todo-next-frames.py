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
tmux = ["tmux", "-S", sys.argv[5]]


def rows(sql, values=()):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql, values).fetchall()


def set_value(selector, value):
    evaluate(f'''(()=>{{const e=document.querySelector({json.dumps(selector)});const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()''')


def tmux_windows():
    windows = subprocess.check_output(tmux + ["list-windows", "-a", "-F", "#{session_name}:#{window_name}"], text=True).split()
    # AMF's own observer session holds no agent.
    return [w for w in windows if not w.startswith("_amf-observer:")]


def agent_state():
    # Nothing may ever be reserved or launched by this proof.
    assert rows("SELECT id,status,agent_session_id FROM todos WHERE status<>'not_started' OR agent_session_id IS NOT NULL") in ([], [("p1", "not_started", "billing-retry-claude")]), rows("SELECT id,status,agent_session_id FROM todos")
    assert tmux_windows() == ["amf-billing-retry:claude"], tmux_windows()
    assert rows("SELECT count(*) FROM feature_sessions WHERE kind IN ('claude','codex','opencode','pi')") == [(1,)]


dialog = "document.querySelector('[role=dialog][aria-label=\"Implement next TODO\"]')"
card = f"{dialog}?.querySelector('[aria-label=\"Next TODO\"]')"
PANEL = "document.querySelector('.todo-panel[aria-label=\"Worktree TODOs\"]')"


def next_title(title):
    wait(f"{card}?.querySelector('strong')?.textContent==={json.dumps(title)}")


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    click("Billing API")
    wait('document.body.innerText.includes("billing-retry")')
    time.sleep(3)
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT id FROM feature_sessions").fetchall() == [("billing-retry-claude",)]
        stamp = time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime())
        workdir = db.execute("SELECT workdir FROM features WHERE id='billing-retry'").fetchone()[0]
        for list_id, scope, project, feature, wd in [("wt-list", "worktree", "shot-project", "billing-retry", workdir),
                                                      ("project-list", "project", "shot-project", "billing-retry", None),
                                                      ("global-list", "global", None, None, None)]:
            db.execute("INSERT INTO todo_lists(id,project_id,feature_id,scope,workdir,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
                       (list_id, project, feature, scope, wd, stamp, stamp))
        for order, (todo_id, list_id, title, body, priority) in enumerate([
            ("w1", "wt-list", "Tidy retry logs", "Drop the debug lines.", "med"),
            ("p1", "project-list", "Fix retry backoff", "Cap at five attempts.", "high"),
            ("g1", "global-list", "Rotate API keys", None, "high"),
            ("g2", "global-list", "Archive old docs", None, "low"),
        ]):
            db.execute("INSERT INTO todos(id,list_id,title,body,priority,status,sort_order,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)",
                       (todo_id, list_id, title, body, priority, "not_started", order, stamp, stamp))
    # Open the feature's TODOs.
    evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.offsetParent!==null && b.textContent.includes("billing-retry") && b.textContent.includes("1 session")).click()')
    wait('Array.from(document.querySelectorAll("[role=tab]")).some(t=>t.textContent.trim().includes("TODOs"))')
    evaluate('Array.from(document.querySelectorAll("[role=tab]")).find(t=>t.textContent.trim().includes("TODOs")).click()')
    wait(f"!!{PANEL} && {PANEL}.querySelectorAll('.todo').length===1")
    agent_state()
    click("Implement next")
    next_title("Fix retry backoff")
    capture("001-next-priority.png",
            "Implement next previews the highest-priority unstarted TODO across the worktree, project and global lists. The project item and a global item are both High; the nearer project scope wins the tie. The user still chooses where to run it, and nothing is reserved.",
            ["Implement next", "Fix retry backoff", "Project · Billing API", "High priority", "You choose which feature runs it", "Skip to next", "Start…"],
            f"{dialog}.querySelectorAll('input[type=checkbox]').length===3")
    agent_state()
    click("Skip to next")
    next_title("Rotate API keys")
    capture("002-next-skipped.png",
            "Skip to next passes over the project item and shows the other High item, the global one, without changing either.",
            ["Rotate API keys", "Global", "High priority"])
    click("Skip to next"); next_title("Tidy retry logs")
    evaluate(f"Array.from({dialog}.querySelectorAll('label')).find(l=>l.textContent.includes('Project list')).querySelector('input').click()")
    evaluate(f"Array.from({dialog}.querySelectorAll('label')).find(l=>l.textContent.includes('Global list')).querySelector('input').click()")
    next_title("Tidy retry logs")
    capture("003-next-worktree-only.png",
            "With the project and global lists switched off only the worktree list is scanned. A worktree TODO belongs to this checkout, so no feature choice is mentioned.",
            ["Tidy retry logs", "Worktree · billing-retry", "Medium priority"],
            f"!{card}.innerText.includes('You choose which feature runs it')")
    evaluate(f"Array.from({dialog}.querySelectorAll('label')).find(l=>l.textContent.includes('Project list')).querySelector('input').click()")
    evaluate(f"Array.from({dialog}.querySelectorAll('label')).find(l=>l.textContent.includes('Global list')).querySelector('input').click()")
    next_title("Fix retry backoff")
    click("Start…")
    wait("!!document.querySelector('[role=dialog][aria-label=\"Start TODO\"]')")
    capture("004-start-dialog.png",
            "Start… hands the picked TODO to the ordinary Start dialog. The user still picks the feature and how to start; no agent has launched and the TODO is unreserved.",
            ["Start work on a TODO", "Fix retry backoff", "Start agent"])
    agent_state()
    click("Cancel")
    wait("!document.querySelector('[role=dialog]')")
    agent_state()

    # Held in reserve: the High item already links a session, so every unstarted item outranks it.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE todos SET agent_session_id='billing-retry-claude' WHERE id='p1'")
    click("Implement next")
    next_title("Rotate API keys")
    for title in ["Tidy retry logs", "Archive old docs"]:
        click("Skip to next")
        next_title(title)
    click("Skip to next")
    wait(f"{card}?.querySelector('strong')?.textContent==='Fix retry backoff' && !!{card}.querySelector('[role=note]')")
    capture("005-next-started.png",
            "The High item already links a session, so it was held behind every unstarted item. With nothing unstarted left it is offered, not started: Go to work, or start another agent.",
            ["Fix retry backoff", "It already has a session.", "Nothing unstarted is left", "Go to work", "Start another agent…"])
    agent_state()
    click("Go to work")
    wait("!!document.querySelector('[role=tab][aria-selected=true]') && document.body.innerText.includes('Compose prompt')")
    capture("006-went-to-work.png",
            "Go to work opens the linked session's tab. No new agent launched and the TODO is still not reserved.",
            ["billing-retry", "Claude", "Quick TODO"],
            "document.querySelector('[role=tab][aria-selected=true]').textContent.includes('Claude')")
    agent_state()

    # Quick capture appends to the feature's default scope and creates its TODOs session.
    assert rows("SELECT count(*) FROM feature_sessions WHERE kind='todos'") == [(0,)]
    click("Quick TODO")
    wait("!!document.querySelector('[role=dialog][aria-label=\"Quick TODO\"]')")
    wait("document.body.innerText.includes('Adds to Worktree')")
    set_value('[aria-label="Quick TODO title"]', "Write the retry runbook")
    capture("007-quick-capture.png",
            "Quick TODO names the list it will append to (this feature's worktree list) before anything is typed, then takes one line.",
            ["Quick TODO", "Adds to Worktree · billing-retry", "Add TODO"],
            "document.querySelector('[aria-label=\"Quick TODO title\"]').value==='Write the retry runbook'")
    click("Add TODO")
    wait("!document.querySelector('[role=dialog][aria-label=\"Quick TODO\"]') && document.body.innerText.includes('Added to Worktree')")
    assert rows("SELECT t.title,t.status,t.priority,l.scope FROM todos t JOIN todo_lists l ON l.id=t.list_id WHERE t.title='Write the retry runbook'") == [("Write the retry runbook", "not_started", "med", "worktree")]
    assert rows("SELECT count(*) FROM feature_sessions WHERE kind='todos'") == [(1,)]
    evaluate('Array.from(document.querySelectorAll("[role=tab]")).find(t=>t.textContent.trim().includes("TODOs")).click()')
    wait(f"!!{PANEL} && {PANEL}.innerText.includes('Write the retry runbook')")
    capture("008-quick-captured.png",
            "The captured TODO is in the worktree list as a not-started Medium item, a toast says where it went, and the feature now has a TODOs session like the TUI creates. No agent launched.",
            ["Write the retry runbook", "Tidy retry logs", "Added to Worktree · billing-retry"],
            allow_error=False)
    assert tmux_windows() == ["amf-billing-retry:claude"], tmux_windows()
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: preview, tie-break, skip, scope toggles, held-in-reserve and quick capture verified; nothing launched", flush=True)
finally:
    ws.close()
    x.close()
