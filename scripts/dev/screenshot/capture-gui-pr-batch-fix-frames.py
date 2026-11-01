#!/usr/bin/env python3
"""Assert native PR Triage states through WebKit IPC and capture its X11 window.

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


try:
    window.configure(x=0, y=0, width=1500, height=1000)
    x.sync()
    wait(has_button("Round invoice totals"))
    click("Round invoice totals")
    click("PR Triage")
    wait('!!document.querySelector(".pr-entry")')
    evaluate('document.querySelector(".pr-entry").click()')
    wait('!!document.querySelector(".pr-detail")')
    # Only unresolved incoming GitHub comments can be marked.
    wait('document.querySelectorAll(".pr-comments input[type=checkbox]").length>=2')
    evaluate('Array.from(document.querySelectorAll(".pr-comments input[type=checkbox]")).slice(0,2).forEach(c=>c.click())')
    wait('document.querySelector("[aria-label=\\"Batch fix\\"]")?.innerText.includes("2 selected")')
    capture("001-selection-needs-explicit-agent.png", "Two review comments are marked for a batch fix. No agent is chosen by default, so Prepare batch fix stays disabled until one is picked.", ["2 selected", "Batch fix agent", "Prepare batch fix…"], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Prepare batch fix…").disabled===true && Array.from(document.querySelectorAll("select")).some(el=>el.value===""&&el.options[0].text==="Choose an agent session…")')
    select("Batch fix agent", "shot-codex")
    click("Prepare batch fix…")
    wait(has_button("Preview send to agent…"))
    capture("002-batch-prompt.png", "One combined prompt covers both marked comments for the chosen Codex session. It is editable and nothing is sent.", ["Batch fix", "Preview send to agent…"], 'document.querySelector(".review-editor textarea").value.includes("Comment 1:") && document.querySelector(".review-editor textarea").value.includes("Comment 2:")')
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    click("Preview send to agent…")
    wait(has_button("Send batch fix to agent"))
    capture("003-send-confirmation.png", "The exact text to send, with one correlated reply-draft command per comment, is shown for explicit confirmation before any terminal delivery or Fixing mark.", ["Send batch fix to agent", "amf reply-draft --pr-number 12", "Cancel batch fix"])
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    (state / "head.txt").write_text("feed" + "1" * 36 + "\n")
    click("Send batch fix to agent")
    wait('!!document.querySelector("[role=alert]")')
    capture("004-stale-head-refusal.png", "A new PR head refuses the batch while retaining the exact confirmation. No agent receives the instruction and no reply draft is registered.", ["The PR has new commits", "Send batch fix to agent"], allow_alert=True)
    assert not pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists()
    (state / "head.txt").write_text("c0ffee" + "0" * 34 + "\n")
    click("Send batch fix to agent")
    wait('document.body.innerText.includes("comments sent to the agent.")')
    capture("005-batch-sent.png", "Confirmed delivery to the private offline Codex fixture marks both comments Fixing and keeps PR Triage open. GitHub remains untouched.", ["comments sent to the agent.", "fixing", "Reply: fixed"])
    for _ in range(100):
        if pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).exists(): break
        time.sleep(0.1)
    sent = pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).read_text()
    assert sent.startswith("Address these PR review comments.") and sent.count("amf reply-draft --pr-number 12") == 2, sent
    time.sleep(1.0)
    click("Reply: fixed")
    wait('document.querySelector(".review-editor textarea")?.value.includes("Fixed negative invoice rounding.")')
    capture("006-agent-reply-draft.png", "The offline agent returns a correlated reply per comment through the real amf CLI. Reply: fixed loads one as an attributed editable draft; posting still needs confirmation.", ["Drafted by the fixing agent", "Reply: fixed"], 'document.querySelector(".review-editor textarea").value.includes("Added regression coverage.")')
    assert gh_writes() == [] and ai_calls() == []
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
