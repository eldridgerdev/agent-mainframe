#!/usr/bin/env python3
"""Assert native PR Triage companion fix targets through WebKit IPC and capture its X11 window.

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
        assert not evaluate('!!document.querySelector("[role=alert]:not([data-confirm-callout])")'), body
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


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait(has_button("Round invoice totals"))
    click("Round invoice totals")
    header_menu("Feature tools", "PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    catalog = 'document.querySelector(".pr-detail .destination-catalog")'
    wait(f'!!{catalog} && {catalog}.innerText.includes("Integrate PR #12 fixes")')
    text = evaluate(f"{catalog}.innerText")
    assert "Triage feature Invoice fixes (triage) (2 commits ahead)" in text, text
    assert "Triage feature Invoice fixes (second triage) (commits ahead unavailable)" in text, text
    assert "Commits ahead unknown:" in text, text
    assert "Unrelated feature" not in text, text
    picker = 'document.querySelector(".pr-detail select[aria-label=\\"Fix agent\\"]")'
    assert evaluate(f"{picker}.value") == "shot-companion-claude"
    capture("001-destination-catalog.png", "The read-only destination catalog lists this feature's sessions, the running PR #12 triage companion (real linked worktree, 2 commits ahead of its recorded base) and a stopped second companion whose checkout cannot prove a count, so it reads 'commits ahead unavailable' with the reason, never zero. The running companion session is preselected; nothing is sent. The unrelated feature is not offered.", ["Destinations (this feature and 2 triage companions)", "Integrate PR #12 fixes…"], f'{picker}.value==="shot-companion-claude"')
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    # The stopped companion's session row is listed but cannot be chosen.
    assert evaluate(f'Array.from({catalog}.querySelectorAll(".destination-session")).find(r=>r.innerText.includes("second companion")).querySelector("button").disabled')
    # Choosing a row feeds the same fix-target select; Prepare fix stays a separate click.
    evaluate(f'Array.from({catalog}.querySelectorAll(".destination-session")).find(r=>r.innerText.includes("Codex 1")).querySelector("button").click()')
    wait(f'{picker}.value==="shot-codex"')
    capture("002-row-choice-feeds-fix-target.png", "Using this feature's Codex row from the catalog changes the Fix agent select, replacing the preselected companion. The row shows Selected; no fix was prepared or sent.", ["Fix agent", "Selected"], f'{picker}.value==="shot-codex"')
    click("Integrate PR #12 fixes…")
    wait('document.body.innerText.includes("Integrate companion commits")')
    capture("003-integrate-link-opens-existing-dialog.png", "The banner's Integrate link opens the existing companion integration dialog for the companion feature. Only its status was read; nothing was previewed, pushed or cherry-picked.", ["Integrate companion commits", "Preview integration"])
    click("Close")
    wait('!document.body.innerText.includes("Integrate companion commits")')
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    assert gh_writes() == [] and ai_calls() == []
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
