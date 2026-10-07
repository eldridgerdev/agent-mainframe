#!/usr/bin/env python3
"""Assert the native GUI's custom-session and VS Code flows through WebKit IPC
and capture its X11 window.

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
from gi.repository import Gdk, GdkX11  # noqa: E402,F401

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
dbpath = pathlib.Path(sys.argv[4])
tmux = ["tmux", "-S", sys.argv[5]]
worktree = sys.argv[6]
code_log = pathlib.Path(sys.argv[7])
window_binary = sys.argv[8]
foreign_pid = int(sys.argv[9])

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


def wait(expression, tries=150):
    for _ in range(tries):
        if evaluate(expression):
            return
        time.sleep(0.1)
    raise AssertionError((expression, evaluate("document.body.innerText")))


def click(text, scope="document"):
    found = evaluate(
        f'(()=>{{const b=Array.from({scope}.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)});'
        f'if(!b||b.disabled)return false;b.click();return true;}})()'
    )
    assert found, (text, evaluate("document.body.innerText"))


def choose(value):
    found = evaluate(
        f'(()=>{{const r=Array.from(document.querySelectorAll("input[name=new-session-kind]")).find(r=>r.value==={json.dumps(value)});'
        f'if(!r||r.disabled)return false;r.click();return true;}})()'
    )
    assert found, value


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


def capture(name, note, expects, expression=None, alert=False):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert alert == evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    time.sleep(0.4)
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
    notes.append({"file": name, "note": note})
    print("PASS:", name, note, flush=True)


def custom_sessions():
    with sqlite3.connect(dbpath) as db:
        return db.execute(
            "SELECT label,tmux_window,command,on_stop,pre_check FROM feature_sessions WHERE kind='custom'"
        ).fetchall()


def editors():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT id,pid,dedicated,command FROM launched_editors ORDER BY id").fetchall()


def standin_windows():
    found = []
    for proc in pathlib.Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            cmdline = (proc / "cmdline").read_bytes()
            state = (proc / "stat").read_text().rsplit(")", 1)[1].split()[0]
        except OSError:
            continue
        if cmdline.startswith(window_binary.encode() + b"\0") and state != "Z":
            found.append(int(proc.name))
    return found


def alive(pid):
    try:
        state = pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
    except OSError:
        return False
    return state != "Z"


dialog = 'document.querySelector("[role=dialog][aria-label=\\"New session\\"]")'


def open_picker():
    click("New session")
    wait(f"!!{dialog}")


try:
    window.configure(x=0, y=0, width=1280, height=860)
    x.sync()
    wait('Array.from(document.querySelectorAll("button.tree-name")).some(b=>b.textContent.trim()==="checkout-redesign")')
    evaluate('Array.from(document.querySelectorAll("button.tree-name")).find(b=>b.textContent.trim()==="checkout-redesign").click()')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="New session")')

    open_picker()
    choose("Dev server")
    capture(
        "001-session-picker.png",
        "New session lists the TUI picker's choices: the allowed agents, Terminal, Neovim, VS Code (enabled: a `code` stand-in is on PATH) and TODOs, then the two custom sessions configured in the project's amf.json, each with its icon, description, command, working directory, pre-check, on-stop command and autolaunch.",
        ["Configured sessions", "Dev server", "Vite with hot reload for the storefront", "npm run dev", "test -f package.json",
         "rm -f .vite-dev.pid", "opens on create", "Database", "docker info --format", "VS Code", "TODOs"],
        f"!{dialog}.querySelector('input[value=vscode]').disabled",
    )

    choose("Database")
    click("Create session", dialog)
    wait(f"!!{dialog}.querySelector('[role=alert]')")
    assert custom_sessions() == [], custom_sessions()
    capture(
        "002-pre-check-failed.png",
        "Database's pre_check (`docker info`, answered by a stand-in whose daemon is down) failed, so nothing was created. The dialog stays open with the check and its output, the TUI's warning text, and offers to run the check again.",
        ["Database was not created: its pre-check failed", "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
         "Run pre-check again"],
        alert=True,
    )

    choose("Dev server")
    wait(f"!{dialog}.querySelector('[role=alert]')")
    click("Create session", dialog)
    wait(f"!{dialog}")
    wait('Array.from(document.querySelectorAll("[role=tab]")).some(t=>t.textContent.includes("Dev server")&&t.getAttribute("aria-selected")==="true")')
    wait('(document.querySelector(".xterm-rows")?.innerText??"").includes("ready in 312 ms")', tries=200)
    rows = custom_sessions()
    assert rows == [("Dev server", "dev-server", "npm run dev", "rm -f .vite-dev.pid", "test -f package.json")], rows
    windows = subprocess.run(tmux + ["list-windows", "-t", "amf-checkout-redesign", "-F", "#{window_name}"],
                             capture_output=True, text=True, check=True).stdout.split()
    assert "dev-server" in windows, windows
    capture(
        "003-custom-session-attached.png",
        "Dev server was created through the TUI's engine in its configured web/ directory and, being autolaunch, opened at once: its tmux window is attached in the GUI terminal like any other, showing the (stand-in) dev server's output. The sidebar lists it with its configured icon.",
        ["Dev server", "Shell"],
        '(document.querySelector(".xterm-rows")?.innerText??"").includes("localhost:5173")',
    )

    open_picker()
    choose("vscode")
    click("Open VS Code", dialog)
    wait(f"!{dialog}")
    wait('Array.from(document.querySelectorAll("[role=tab]")).some(t=>t.textContent.includes("VS Code")&&t.getAttribute("aria-selected")==="true")')
    wait('!!document.querySelector(".vscode-state-open")', tries=200)
    calls = code_log.read_text().splitlines()
    assert calls == [f"--new-window {worktree}"], calls
    rows = editors()
    owned = [row for row in rows if row[0] != "editor-foreign"]
    assert len(owned) == 1 and owned[0][2] == 1, rows
    assert owned[0][1] in standin_windows(), (owned, standin_windows())
    assert "code --new-window" in owned[0][3], owned
    capture(
        "004-vscode-open.png",
        "VS Code was opened with the TUI's own launch (`code --new-window <worktree>`, here a stand-in). The background resolver found the new window process, so it is recorded as AMF's and closes with the feature. A window the user opened earlier is listed as not AMF's. The sidebar shows the feature's VS Code chip.",
        ["VS Code", "Open another VS Code window", "Close windows AMF opened", "closes it, with its language servers, when the feature stops",
         "NOT AMF'S", "AMF never closes it"],
    )

    click("Close windows AMF opened")
    wait("!!document.querySelector('.vscode-panel [role=alert]')")
    assert standin_windows(), "nothing closes before confirmation"
    capture(
        "005-vscode-close-confirm.png",
        "Closing asks first, and says which windows are left running.",
        ["Close this VS Code window?", "Unsaved changes in them are lost", "Close windows"],
        alert=True,
    )

    click("Close windows")
    wait("!!document.querySelector('[aria-label=\"Close result\"]')")
    for _ in range(50):
        if not standin_windows():
            break
        time.sleep(0.1)
    assert not standin_windows(), "the AMF-owned stand-in window should be closed"
    assert alive(foreign_pid), "the window AMF did not open must keep running"
    rows = editors()
    assert [row[0] for row in rows] == ["editor-foreign"], rows
    wait("!document.querySelector('.vscode-state-open')")
    capture(
        "006-vscode-closed.png",
        "The owned window and its child process were closed through the shared tracked-editor cleanup; the window AMF did not open was left running and is still listed.",
        ["Closed VS Code (2 processes ended)", "Left VS Code running: AMF did not open this window", "NOT AMF'S"],
    )
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
    print("PASS: Custom session picker, pre_check failure, attachment and VS Code open/close verified", flush=True)
finally:
    ws.close()
    x.close()
