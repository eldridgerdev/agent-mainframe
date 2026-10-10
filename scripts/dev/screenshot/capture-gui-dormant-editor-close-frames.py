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
    raise AssertionError(expression)


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
owned_pid = int(sys.argv[6])
foreign_pid = int(sys.argv[7])


def tmux_alive(session):
    return subprocess.run(tmux + ["has-session", "-t", session], stderr=subprocess.DEVNULL).returncode == 0


def pid_running(pid):
    try:
        state = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
    except OSError:
        return False
    return state != "Z"


def check(label):
    evaluate(f"""(()=>{{const box=Array.from(document.querySelectorAll('input[type=checkbox]')).find(b=>b.getAttribute('aria-label')==={json.dumps('Select ' + label)});box.click();}})()""")


panel = "document.querySelector('[role=dialog][aria-label=\"Dormant features\"]')"

extra_editor = None
try:
    window.configure(x=0, y=0, width=1180, height=820)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Dormant features")')
    click("Dormant features")
    for _ in range(40):
        wait(f"!!{panel} && !{panel}.innerText.includes('Checking tmux activity')")
        if evaluate(f"{panel}.querySelectorAll('.dormancy-row').length") == 3:
            break
        click("Refresh")
        time.sleep(1.5)
    else:
        raise AssertionError(evaluate(f"{panel}.innerText"))
    capture("001-editor-list.png",
        "Dormant features offers Close editors beside the feature with tracked windows. The feature is running and no cleanup has happened.",
        ["billing-retry", "Editor open", "Close editors", "Stop selected (0)"])
    click("Close editors")
    wait("!!document.querySelector('[aria-label=\"Confirm closing dormant editors\"]')")
    capture("002-editor-confirm.png",
        "Editor-only confirmation names the AMF-opened and unowned windows, warns about unsaved changes and states that feature sessions keep running. Stop cleanup is disabled in this fixture, but explicit editor close remains available.",
        ["The feature and its sessions keep running", "Unsaved changes", "AMF opened this window", "Not AMF's", "Close windows"])
    assert pid_running(owned_pid) and pid_running(foreign_pid)

    # A second real, runner-owned stand-in opens after the confirmation loaded.
    owned_command = pathlib.Path(f"/proc/{owned_pid}/cmdline").read_bytes().split(b"\0")
    fake_code = owned_command[0].decode()
    with sqlite3.connect(dbpath) as db:
        workdir = db.execute("SELECT workdir FROM features WHERE id='billing-retry'").fetchone()[0]
    extra_editor = int(subprocess.check_output(
        ["setsid", "sh", "-c", '\"$@\" >/dev/null 2>&1 </dev/null & echo $!', "sh",
         fake_code, "-c", "sleep 600 & wait", "--new-window", workdir], text=True).strip())
    with sqlite3.connect(dbpath) as db:
        db.execute("INSERT INTO launched_editors(id,feature_id,session_id,kind,pid,worktree_path,dedicated,command,proc_started_at,started_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
            ("editor-new", "billing-retry", None, "vscode", extra_editor, workdir, 1, "code --new-window", "", "2026-10-01T12:00:00Z"))
    click("Close windows")
    wait("!!document.querySelector('[role=alert]')")
    body = evaluate("document.body.innerText")
    assert "Another VS Code window was opened" in body, body
    assert pid_running(owned_pid) and pid_running(extra_editor) and pid_running(foreign_pid)
    # The intentional refusal is the subject of this frame.
    capture("003-editor-refused.png",
        "A stand-in window opened after confirmation, so Rust refuses the older window list before signalling any process. All editors and feature sessions still run; returning and refreshing is required.",
        ["Another VS Code window was opened", "Go back and refresh", "Close windows"],
        "Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==='Close windows').disabled", allow_error=True)
    click("Back")
    click("Refresh")
    wait(f"!{panel}.innerText.includes('Checking tmux activity') && Array.from(document.querySelectorAll('button')).some(b=>b.textContent.trim()==='Refresh'&&!b.disabled)")
    click("Close editors")
    wait("document.querySelector('[aria-label=\"Confirmed editor windows\"]').children.length===3")
    capture("004-editor-reconfirm.png",
        "After a fresh list, confirmation includes all three tracked windows: two AMF-opened stand-ins and the unowned one. Nothing has closed yet.",
        ["AMF opened this window", "Not AMF's", "The feature and its sessions keep running"])
    click("Close windows")
    wait("!!document.querySelector('[aria-label=\"Editor close results\"]')")
    for _ in range(50):
        if not pid_running(owned_pid) and not pid_running(extra_editor):
            break
        time.sleep(0.1)
    assert not pid_running(owned_pid) and not pid_running(extra_editor)
    assert pid_running(foreign_pid)
    assert all(tmux_alive(f"amf-{name}") for name in ["billing-retry", "docs-refresh", "search-index", "checkout-flow"])
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT count(*) FROM feature_sessions").fetchone()[0] == 4
        assert db.execute("SELECT count(*) FROM features WHERE status='idle'").fetchone()[0] == 4
    capture("005-editor-results.png",
        "The shared cleanup closes both AMF-opened stand-ins and their children, reports the unowned window left running, and leaves all four feature tmux sessions and saved sessions intact.",
        ["the feature and its sessions keep running", "Closed VS Code (2 processes ended)", "Left VS Code running: AMF did not open this window"])
    click("Back to dormant features")
    wait(f"{panel}.querySelectorAll('.dormancy-row').length===3")
    capture("006-feature-still-dormant.png",
        "The feature remains in Dormant features after its editor cleanup. Its unowned editor is still open, and editor-only cleanup has neither stopped nor opened the feature.",
        ["billing-retry", "docs-refresh", "search-index", "Stop selected (0)"])
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: Editor-only close, stale confirmation refusal and running session preservation verified", flush=True)
finally:
    if extra_editor and pid_running(extra_editor):
        subprocess.run(["pkill", "-TERM", "-P", str(extra_editor)], check=False)
        import os, signal
        os.kill(extra_editor, signal.SIGTERM)
    ws.close()
    x.close()
