#!/usr/bin/env python3
"""Assert native PR Triage companion-feature creation (stopped, cancellable) through WebKit IPC and capture its X11 window.

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




def branch_exists(name):
    return subprocess.run(["git", "show-ref", "--verify", "--quiet", f"refs/heads/{name}"], cwd=pathlib.Path(sys.argv[4])).returncode == 0

def in_form(text):
    """Click a button inside the companion-feature form only."""
    evaluate(f'Array.from(document.querySelector("[aria-label=\\"New triage feature\\"]").querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()')


def form():
    return 'document.querySelector("[aria-label=\\"New triage feature\\"]")'


def form_text():
    return evaluate(f'{form()}?.innerText ?? ""')


def create_enabled():
    return f'(!!{form()} && Array.from({form()}.querySelectorAll("button")).some(b=>b.textContent.trim()==="Create (stopped)" && !b.disabled))'


repo = pathlib.Path(sys.argv[4])


def companions():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT id, name, branch, workdir, status, triage_source FROM features WHERE triage_source IS NOT NULL AND id NOT LIKE 'shot-%'").fetchall()


def companion_sessions(feature_id):
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT label, kind, stopped FROM feature_sessions WHERE feature_id=?", (feature_id,)).fetchall()


def forget_companion(feature_id, workdir, branch):
    """Fixture-only reset so a second creation can follow the cancelled one."""
    subprocess.run(["git", "worktree", "remove", "--force", workdir], cwd=repo, check=True, capture_output=True)
    subprocess.run(["git", "branch", "-D", branch], cwd=repo, check=True, capture_output=True)
    with sqlite3.connect(dbpath) as db:
        db.execute("DELETE FROM feature_sessions WHERE feature_id=?", (feature_id,))
        db.execute("DELETE FROM features WHERE id=?", (feature_id,))
        version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
        db.execute("INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)", ((int(version[0]) if version else 0) + 1,))


def git_out(*args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait(has_button("Round invoice totals"))
    click("Round invoice totals")
    click("PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    before_windows = tmux_windows()
    head = git_out("rev-parse", "round-invoice-totals")
    assert companions() == [] and not branch_exists("round-invoice-totals-triage")
    wait(has_button("New triage feature…"))
    click("New triage feature…")
    wait(f'!!{form()} && {form()}.innerText.includes("Setup script") && {has_button("Create (stopped)")}')
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    capture("001-form-needs-the-hook-choice.png", "The New triage feature form for PR #12 after its read-only preview: a new branch started from the PR head, its checkout path, the PR branch on the remote, and the exact setup hook that would run. Create (stopped) is disabled until the hook's question is answered. The agent is stopped; nothing is pushed or posted.", ["New triage feature", "PR head", "PR branch (on the remote)", "Setup script", "Create (stopped)", "The agent is stopped"], f'!{create_enabled()}')
    assert companions() == [] and not branch_exists("round-invoice-totals-triage")
    select("Environment", "full")
    wait(create_enabled())
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    capture("002-preview-ready-to-create.png", "With the hook answered (Environment: full) the preview names the choice that will be passed to the hook and the bound PR head. Still nothing is created.", ["Environment: full", "Create (stopped)", "Nothing is pushed and nothing is posted to GitHub"], create_enabled())
    assert companions() == [] and not branch_exists("round-invoice-totals-triage")
    in_form("Create (stopped)")
    wait(f'{form()}.innerText.includes("Running the setup script") && {has_button("Cancel creation")}')
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    capture("003-creation-running-cancel-offered.png", "Creation is a background operation: it reports the phase (the setup hook is running in the new checkout) and offers Cancel creation. The form cannot be closed while it runs.", ["Running the setup script", "Cancel creation"])
    in_form("Cancel creation")
    wait(f'{form()}.innerText.includes("Creation cancelled")')
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    kept = companions()
    assert len(kept) == 1 and kept[0][4] == "stopped", kept
    assert [row[:2] for row in companion_sessions(kept[0][0])] == [("PR Triage", "claude")]
    assert pathlib.Path(kept[0][3]).is_dir(), "the checkout must be kept after a cancel"
    assert tmux_windows() == before_windows, "cancelling must not start anything"
    capture("004-cancelled-checkout-kept.png", "Cancelling stopped the hook. The worktree already existed, so it was kept and registered as a stopped feature, and the message names its path. Nothing is deleted automatically and no agent was started.", ["Creation cancelled", "was kept at", "no agent was started"])
    forget_companion(kept[0][0], kept[0][3], kept[0][2])
    assert companions() == []
    in_form("Edit and try again")
    wait(create_enabled())
    select("Environment", "offline")
    wait(f'{create_enabled()} && {form()}.innerText.includes("Environment: offline")')
    in_form("Create (stopped)")
    wait(f'{form()}.innerText.includes("Created") && {has_button("Close")}')
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    made = companions()
    assert len(made) == 1 and made[0][4] == "stopped" and made[0][2] == "round-invoice-totals-triage", made
    link = json.loads(made[0][5])
    assert link["pr_number"] == 12 and link["source_feature_id"] == "shot-feature" and link["base_sha"] == head and link["pr_branch"] == "round-invoice-totals", link
    sessions = companion_sessions(made[0][0])
    assert [row[:2] for row in sessions] == [("PR Triage", "claude")], sessions
    assert git_out("-C", made[0][3], "rev-parse", "HEAD") == head
    assert (pathlib.Path(made[0][3]) / "setup-choice.txt").read_text() == "offline"
    assert tmux_windows() == before_windows, "creating the companion must not start any tmux window"
    capture("005-created-stopped.png", "The companion was created stopped at the PR head: a registered feature linked to PR #12 and its source feature, one triage session, no tmux window and no agent process. The hook ran once with the chosen option. Start it from its session tab.", ["Created 'round-invoice-totals-triage'", "stopped", "Nothing is running", "session tab"])
    in_form("Close")
    picker = 'document.querySelector(".pr-detail select[aria-label=\\"Fix agent\\"]")'
    wait(f'!!{picker} && Array.from({picker}.options).some(o=>o.textContent.includes("(stopped)") && o.textContent.includes("PR Triage"))')
    evaluate(f'{picker}.scrollIntoView({{block:"center"}})')
    time.sleep(0.3)
    capture("006-companion-in-destination-picker.png", "The new companion appears in the shared destination picker as a stopped triage companion with its commits-ahead summary and an Integrate link, and its session is the chosen fix agent. Preparing a fix is a separate click; nothing was prepared or sent.", ["Fix agent", "Triage companion: round-invoice-totals-triage", "Integrate PR #12 fixes", "New triage feature…"])
    wait(has_button("New triage feature…"))
    click("New triage feature…")
    wait(f'!!document.querySelector("[role=alert]") && {form()}.innerText.includes("already exists")')
    evaluate(f'{form()}.scrollIntoView({{block:"end"}})')
    assert len(companions()) == 1
    capture("007-second-companion-for-this-head-refused.png", "A second companion for the same PR head is refused with the existing one's name; the person is pointed at the destination list. No branch, worktree or feature was added.", ["already exists", "Create (stopped)"], allow_alert=True)
    assert gh_writes() == [] and ai_calls() == []
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
