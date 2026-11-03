#!/usr/bin/env python3
"""Assert native PR Triage dedicated-session creation through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
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
    (out / "failed-state.txt").write_text(evaluate("document.body.innerText"))
    raise AssertionError(expression)


def click(text):
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
    )

def header_menu(menu, label):
    """Choose `label` from the page-header menu whose trigger is named `menu`
    (the feature header keeps Plan, review and PR tools in "Feature tools")."""
    trigger = json.dumps('[aria-label="%s"]' % menu)
    wait(f"!!document.querySelector({trigger})")
    evaluate(f"document.querySelector({trigger}).click()")
    item = ("Array.from(document.querySelectorAll('[role=menuitem]')).find(b=>"
            f"b.querySelector('.menu-item-label')?.textContent==={json.dumps(label)} && !b.disabled)")
    wait(f"!!{item}")
    evaluate(f"{item}.click()")


def select(label, value):
    evaluate(
        f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');const set=Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set;set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()"""
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


def capture(name, note, expects, expression=None, allow_alert=False):
    wait('document.fonts.status==="loaded"')
    wait('!document.body.innerText.includes("Loading changes…")')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_alert:
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



def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent.startsWith({json.dumps(label)}));const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def has_button(text):
    return f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)})'


def idle():
    """No command in flight: the panel's buttons are enabled again."""
    wait('!!document.querySelector(".pr-detail, .pr-picker") && !Array.from(document.querySelectorAll(".modal .btn")).every(b=>b.disabled)')


def choose_comment(author_prefix):
    evaluate(f'Array.from(document.querySelectorAll(".pr-comment")).find(b=>b.textContent.startsWith({json.dumps(author_prefix)})).click()')


state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])


def ai_calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def gh_writes():
    return (state / "writes.jsonl").read_text().splitlines()


def detail():
    return evaluate('document.querySelector(".pr-detail")?.innerText ?? ""')


import sqlite3

dbpath = pathlib.Path(os.environ["AMF_GUI_DB"])


def rows():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT label, kind, stopped, tmux_window FROM feature_sessions WHERE feature_id='shot-feature' ORDER BY sort_order, created_at").fetchall()


def tmux_windows():
    out = subprocess.run(["tmux", "-S", os.environ["AMF_TMUX_SOCKET"], "list-windows", "-a", "-F", "#{session_name}:#{window_name}"], capture_output=True, text=True)
    return sorted(out.stdout.split())


def in_form(text):
    """Click a button inside the dedicated-session form only."""
    evaluate(f'Array.from(document.querySelector("[aria-label=\\"Dedicated triage session\\"]").querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()')


def form():
    return 'document.querySelector("[aria-label=\\"Dedicated triage session\\"]")'


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait(has_button("Round invoice totals"))
    click("Round invoice totals")
    header_menu("Feature tools", "PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    before_rows = rows()
    before_windows = tmux_windows()
    assert [r[0] for r in before_rows] == ["Claude 1", "Codex 1"], before_rows
    click("New dedicated session…")
    wait(f'!!{form()} && {has_button("Create (stopped)")}')
    capture("001-preview-before-create.png", "The dedicated-session form for PR #12 after its read-only preview. It says the agent is created stopped, nothing runs or is sent, and how to start it; Create (stopped) is the only action that writes.", ["Dedicated triage session", "stopped", "Create (stopped)", "Nothing is posted to GitHub."])
    assert rows() == before_rows and tmux_windows() == before_windows
    in_form("Create (stopped)")
    wait(f'{form()}.innerText.includes("Created")')
    after = rows()
    assert len(after) == 3 and after[2][0] == "PR Triage" and after[2][1] == "claude" and after[2][2] == 1, after
    assert tmux_windows() == before_windows, "creating the session must not start any tmux window"
    picker = 'document.querySelector(".pr-detail select[aria-label=\\"Fix agent\\"]")'
    wait(f'Array.from({picker}.options).some(o=>o.textContent==="PR Triage (stopped)")')
    capture("002-created-stopped.png", "The session was created stopped: a saved row with no tmux window and no agent process. The form says so and how to start it. Nothing was prepared or sent.", ["Created 'PR Triage' (Claude), stopped", "Nothing is running", "session tab"], f'{picker}.selectedOptions[0].textContent==="PR Triage (stopped)"')
    evaluate(f'{picker}.scrollIntoView({{block:"center"}})')
    time.sleep(0.3)
    capture("003-selected-as-fix-agent.png", "The created session joins the Fix agent picker as PR Triage (stopped) and is selected there. Prepare fix is a separate click; nothing is prepared, sent or started.", ["Fix agent", "Prepare fix…", "New dedicated session…"], f'{picker}.selectedOptions[0].textContent==="PR Triage (stopped)"')
    evaluate(f'{form()}.scrollIntoView({{block:"start"}})')
    in_form("Close")
    click("New dedicated session…")
    wait(f'{form()}.innerText.includes("already exists") && {has_button("Use existing session")}')
    evaluate(f'{form()}.scrollIntoView({{block:"start"}})')
    assert rows() == after
    capture("004-reuses-existing-session.png", "Reopening the form finds the labelled session and says so: it is reused, nothing new is created, and the button changes to Use existing session.", ["already exists", "Use existing session"])
    select("Agent", "codex")
    wait(f'!!document.querySelector("[role=alert]") && {form()}.innerText.includes("already runs Claude")')
    capture("005-other-harness-conflict.png", "Asking for Codex under the same name is a conflict, not a silent reuse: the PR Triage label already runs Claude. No session is created.", ["already runs Claude"], allow_alert=True)
    assert rows() == after
    assert gh_writes() == [] and ai_calls() == []
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
