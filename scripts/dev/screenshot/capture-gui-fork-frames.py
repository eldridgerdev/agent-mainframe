#!/usr/bin/env python3
"""Assert native GUI fork states through WebKit IPC and capture its X11 window.

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
repo = pathlib.Path(sys.argv[5])

dialog = "document.querySelector('[role=dialog][aria-label=\"Fork feature\"]')"
HARNESS = f"{dialog}.querySelector('select')"


def open_fork():
    evaluate("document.querySelector('button[aria-label=\"More feature actions\"]').click()")
    wait("Array.from(document.querySelectorAll('[role=menuitem]')).some(i=>i.textContent.includes('Fork feature'))")


def choose_fork():
    evaluate("Array.from(document.querySelectorAll('[role=menuitem]')).find(i=>i.textContent.includes('Fork feature')).click()")
    wait(f"!!{dialog} && {dialog}.innerText.includes('will be copied') && {HARNESS}.options.length>0 && !{HARNESS}.disabled")


def pick_available_harness():
    # Which harnesses are installed differs between machines; take the first offered.
    evaluate(f"""(()=>{{const s={HARNESS};const o=Array.from(s.options).find(o=>!o.disabled);
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(s,o.value);
      s.dispatchEvent(new Event('change',{{bubbles:true}}));}})()""")


def set_start(on):
    evaluate(f"""(()=>{{const c=Array.from({dialog}.querySelectorAll('input[type=checkbox]')).find(c=>c.closest('label').innerText.includes('Start the agent'));
      if(c.checked!=={json.dumps(on)}) c.click();}})()""")


def fork_button_enabled():
    return f"!{dialog}.querySelector('button[type=submit]').disabled"


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="billing-retry")')
    click("billing-retry")
    wait("!!document.querySelector('button[aria-label=\"More feature actions\"]')")
    open_fork()
    capture("001-fork-menu.png",
            "Fork feature is an explicit item in the feature's action menu. Nothing has been read or created yet.",
            ["Fork feature", "billing-retry"],
            "!document.querySelector('[role=dialog]')")
    choose_fork()
    capture("002-fork-preview.png",
            "The preview lists the three uncommitted changes (including the executable and the untracked file), states that staging is not copied, shows the source branch and HEAD, and says why the Claude session has no conversation to include. Starting the agent is a visible, default-on choice.",
            ["Fork feature", "3 uncommitted changes will be copied", "retry.py", "notes.md", "deploy.sh",
             "staging choices are not copied", "no recorded Claude conversation", "Start the agent after forking"],
            f"{dialog}.querySelectorAll('input[type=checkbox]')[0].disabled")
    pick_available_harness()
    # Edit the source after the preview: the confirmation must be refused, never retargeted.
    (repo / "retry.py").write_text("RETRIES = 99  # edited after preview\n")
    set_start(False)
    wait(fork_button_enabled())
    click("Fork feature")
    wait("!!document.querySelector('[role=dialog] [role=alert]')")
    capture("003-stale-refused.png",
            "The source changed after the preview, so the confirmation is refused and the form gives way to Try again. No worktree, branch or feature was created.",
            ["prepare the fork again", "Try again"],
            f"!Array.from({dialog}.querySelectorAll('button')).some(b=>b.textContent.trim()==='Fork feature')", allow_error=True)
    assert not (repo / ".worktrees").exists()
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT count(*) FROM features").fetchone() == (1,)
    (repo / "retry.py").write_text("RETRIES = 7  # unstaged on top of staged\n")
    click("Cancel")
    wait(f"!{dialog}")

    open_fork()
    choose_fork()
    pick_available_harness()
    set_start(False)
    wait(fork_button_enabled())
    capture("004-ready-stopped.png",
            "A fresh preview of the restored source. Start the agent is switched off, so the fork will be created stopped and no harness can launch.",
            ["3 uncommitted changes will be copied", "Start the agent after forking", "created stopped"],
            f"!{dialog}.querySelectorAll('input[type=checkbox]')[1].checked")
    click("Fork feature")
    wait(f"!!{dialog} && {dialog}.innerText.includes('Worktree')  && !!Array.from({dialog}.querySelectorAll('button')).find(b=>b.textContent.trim()==='Open fork')")
    capture("005-fork-created.png",
            "The fork is registered and stopped. The result names the new worktree, the three copied changes, and that the agent was not started. The runner independently verifies file bytes, the executable bit, an empty index and that no tmux session exists.",
            ["billing-retry-fork", "Not started (the feature is stopped)", "Copied", "3 working changes"],
            None)
    click("Open fork")
    wait("document.body.innerText.includes('billing-retry-fork') && !document.querySelector('[role=dialog]')")
    capture("006-fork-opened.png",
            "Open fork selects the new stopped feature from the sidebar.",
            ["billing-retry-fork"],
            None)

    click("billing-retry")
    wait("Array.from(document.querySelectorAll('[role=tab]')).some(t=>t.textContent.includes('Codex 1'))")
    evaluate("Array.from(document.querySelectorAll('[role=tab]')).find(t=>t.textContent.includes('Codex 1')).click()")
    open_fork()
    choose_fork()
    capture("007-codex-no-context.png",
            "Forking from a Codex session discloses that conversation export is Claude-only; the include switch is disabled and the working changes are still inherited.",
            ["Conversation export is only available for Claude Code sessions", "Codex 1"],
            f"{dialog}.querySelectorAll('input[type=checkbox]')[0].disabled")
    click("Cancel")
    wait(f"!{dialog}")
    with sqlite3.connect(dbpath) as db:
        assert db.execute("SELECT count(*) FROM features").fetchone() == (2,)
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"], "note": n["note"]}) + "\n" for n in notes))
    print("PASS: preview, stale refusal, stopped fork and context disclosure verified", flush=True)
finally:
    ws.close()
    x.close()
