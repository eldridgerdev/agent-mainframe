#!/usr/bin/env python3
"""Assert PR Workspace Files-tab review comments through native WebKit IPC.

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
            str(pathlib.Path(os.environ.get("AMF_PR_FILES_LINES_PROOF_SCOPE", str(out))) / name),
        ],
        check=True,
    )
    digest = hashlib.sha256((pathlib.Path(os.environ.get("AMF_PR_FILES_LINES_PROOF_SCOPE", str(out))) / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)



def has_button(text):
    return f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)})'


state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])


def ai_calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def gh_writes():
    return (state / "writes.jsonl").read_text().splitlines()



def set_text(label, value):
    evaluate(
        "(()=>{const t=Array.from(document.querySelectorAll('textarea')).find(e=>e.closest('label')?.textContent.includes(%s)||e.getAttribute('aria-label')===%s);"
        "const set=Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set;set.call(t,%s);t.dispatchEvent(new Event('input',{bubbles:true}));})()"
        % (json.dumps(label), json.dumps(label), json.dumps(value))
    )


def pick(label, shift=False):
    evaluate(
        "(()=>{const b=document.querySelector('button[aria-label=%s]');b.dispatchEvent(new MouseEvent('click',{bubbles:true,shiftKey:%s}));})()"
        % (json.dumps(label), "true" if shift else "false")
    )


def severity(value):
    evaluate(
        "(()=>{const s=Array.from(document.querySelectorAll('select')).find(e=>e.closest('label')?.textContent.includes('Severity'));"
        "const set=Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set;set.call(s,%s);s.dispatchEvent(new Event('change',{bubbles:true}));})()"
        % json.dumps(value)
    )


repo = pathlib.Path(sys.argv[4])


def git(*args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_AUTHOR_NAME="AMF demo",
               GIT_AUTHOR_EMAIL="demo@example.com", GIT_COMMITTER_NAME="AMF demo", GIT_COMMITTER_EMAIL="demo@example.com")
    return subprocess.check_output(["git", *args], cwd=repo, env=env, text=True).strip()


def open_workspace():
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.includes("Round invoice totals"))')
    evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.includes("Round invoice totals")).click()')
    wait(has_button("PR Workspace (experimental)"))
    click("PR Workspace (experimental)")
    wait('Array.from(document.querySelectorAll(".pr-entry")).some(b=>b.textContent.includes("#12"))')
    evaluate('Array.from(document.querySelectorAll(".pr-entry")).find(b=>b.textContent.includes("#12")).click()')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Files" && !b.disabled)')


try:
    window.configure(x=0, y=0, width=1500, height=1100)
    x.sync()
    open_workspace()
    click("Files")
    wait('document.body.innerText.includes("pinned to head") && document.querySelectorAll(".diff-line-select").length>0')
    capture("pr-files-lines-opened.png", "The workspace Files tab is a reviewable diff: every line number is selectable and comments are pinned to the head the workspace loaded.", ["pinned to head", "Comment on selection", "Preview and post in PR Review"], 'document.querySelectorAll(".review-inline-thread").length===0')
    pick("Select line 8")
    pick("Select line 9", True)
    click("Comment on selection")
    wait('!!document.querySelector(".review-editor")')
    set_text("Line comment", "Round after tax, but add the currency rounding rule to the docs.")
    severity("blocker")
    capture("pr-files-lines-range-editor.png", "Shift-click selects lines 8-9 in the Files diff; the same editor and severity as PR Review save a range comment.", ["Selected line 8 – line 9", "Anchored to line 8 – line 9", "Save line comment"])
    click("Save line comment")
    wait('document.querySelectorAll(".review-inline-thread").length===1')
    pick("Select base line 8")
    click("Comment on selection")
    wait('!!document.querySelector(".review-editor")')
    set_text("Line comment", "The old unrounded return was relied on by the exporter.")
    severity("question")
    click("Save line comment")
    wait('document.querySelectorAll(".review-inline-thread").length===2')
    pick("Select base line 8")
    click("Suggest replacement")
    wait('!!document.querySelector("[role=alert]")')
    capture("pr-files-lines-suggestion-refused.png", "The dialog's rule applies here: a suggestion over a removed base line is refused with the reason, and nothing is saved.", ["removed (base-only) line", "Saved line comments (2)"], allow_alert=True)
    pick("Select line 10")
    click("Suggest replacement")
    wait('!!document.querySelector(".review-editor")')
    set_text("Suggested replacement", "}  // end invoiceTotal")
    click("Save suggestion")
    wait('document.querySelectorAll(".review-inline-thread").length===3')
    capture("pr-files-lines-inline-threads.png", "Saved threads appear inline under their lines in the Files diff: range, base side and suggestion.", ["Saved line comments (3)", "base line 8", "line 8 – line 9"], 'document.querySelectorAll(".review-inline-thread").length===3')
    # The draft belongs to the PR, not to the tab: leaving and returning keeps it.
    click("Threads")
    wait('document.body.innerText.includes("Sort")')
    click("Files")
    wait('document.querySelectorAll(".review-inline-thread").length===3')
    capture("pr-files-lines-after-tab-switch.png", "Switching to Threads and back reopens the same saved draft; the three threads are still in the diff.", ["Saved line comments (3)"], 'document.querySelectorAll(".review-inline-thread").length===3')
    # Preview and posting stay in the explicit-confirm PR Review dialog.
    click("Preview and post in PR Review…")
    wait('document.querySelectorAll(".pr-entry").length>=1 && !document.querySelector(".pr-workspace-page")')
    evaluate('Array.from(document.querySelectorAll(".pr-entry")).find(b=>b.textContent.includes("#12")).click()')
    wait('document.body.innerText.includes("Saved line comments (3)")')
    click("Preview submission")
    wait('!!document.querySelector("[aria-label=\\"Confirm PR review\\"]")')
    capture("pr-files-lines-dialog-preview.png", "The same draft previews in the PR Review dialog with its ranges, sides and suggestion block. Nothing is posted without the confirm button.", ["Confirm and post to GitHub", "invoice.ts:8–9 (current · RIGHT)", "invoice.ts:8 (base · LEFT)"])
    assert ai_calls() == [] and gh_writes() == []
    click("Keep editing")
    click("Back to pull requests")
    evaluate('document.querySelector("[role=dialog] button[aria-label=Close]").click()')
    wait('!document.querySelector("[role=dialog]")')
    # A head that moved after the workspace loaded the PR refuses editing.
    open_workspace()
    tree = git("rev-parse", "round-invoice-totals^{tree}")
    old = git("rev-parse", "round-invoice-totals")
    moved = git("commit-tree", tree, "-p", old, "-m", "Moved after the workspace loaded it")
    git("update-ref", "refs/pull/12/head", moved)
    (state / "head.txt").write_text(moved + "\n")
    click("Files")
    wait('document.body.innerText.includes("pinned to one head")')
    capture("pr-files-lines-moved-head-refusal.png", "The PR moved after the workspace loaded it. Files refuses to author comments against a different head and says how to refresh.", ["pinned to one head", "editing is refused"], 'Array.from(document.querySelectorAll(".diff-line-select")).every(b=>b.disabled)', allow_alert=True)
    assert ai_calls() == [] and gh_writes() == []
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
