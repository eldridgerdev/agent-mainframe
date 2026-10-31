#!/usr/bin/env python3
"""Assert PR Review line, range, side and suggestion comments through native WebKit IPC.

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
            str(pathlib.Path(os.environ.get("AMF_PR_REVIEW_LINES_PROOF_SCOPE", str(out))) / name),
        ],
        check=True,
    )
    digest = hashlib.sha256((pathlib.Path(os.environ.get("AMF_PR_REVIEW_LINES_PROOF_SCOPE", str(out))) / name).read_bytes()).digest()
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


try:
    window.configure(x=0, y=0, width=1500, height=1100)
    x.sync()
    wait('document.querySelector("h1")?.textContent==="Invoice API"')
    click("PR Review")
    wait('document.querySelectorAll(".pr-entry").length===2')
    evaluate('Array.from(document.querySelectorAll(".pr-entry")).find(b=>b.textContent.includes("#12")).click()')
    wait('document.body.innerText.includes("pinned to head") && document.querySelectorAll(".diff-line-select").length>0')
    capture("pr-review-lines-opened.png", "PR Review opens #12 with every diff line number selectable; comments are pinned to the reviewed head.", ["#12 Round invoice totals to cents", "pinned to head", "Comment on selection"])
    # A range on the current side: two added lines.
    pick("Select line 8")
    pick("Select line 9", True)
    evaluate('document.querySelector(".diff-code").scrollIntoView({block:"start"})')
    capture("pr-review-lines-selection.png", "Click then Shift-click highlights the selected diff rows (lines 8-9, added side).", ["Selected line 8 – line 9"], 'document.querySelectorAll(".review-line-selected").length===2')
    evaluate('document.querySelector(".diff-content").scrollTop=0')
    click("Comment on selection")
    wait('!!document.querySelector(".review-editor")')
    set_text("Line comment", "Round after tax, but add the currency rounding rule to the docs.")
    severity("blocker")
    capture("pr-review-lines-range-editor.png", "Shift-click selects lines 8-9; the editor shows the range anchor and a severity.", ["Selected line 8 – line 9", "Anchored to line 8 – line 9", "Save line comment"])
    click("Save line comment")
    wait('document.body.innerText.includes("Saved line comments (1)")')
    # A comment on the removed line anchors to the BASE side.
    pick("Select base line 8")
    click("Comment on selection")
    wait('!!document.querySelector(".review-editor")')
    set_text("Line comment", "The old unrounded return was relied on by the exporter.")
    severity("question")
    click("Save line comment")
    wait('document.body.innerText.includes("Saved line comments (2)")')
    # A suggestion cannot cover a removed line: refused with a reason.
    pick("Select base line 8")
    click("Suggest replacement")
    wait('!!document.querySelector("[role=alert]")')
    capture("pr-review-lines-suggestion-refused.png", "A suggestion over a removed base line is refused with the reason, before anything is saved.", ["removed (base-only) line", "Saved line comments (2)"], allow_alert=True)
    # A suggestion on a current line.
    pick("Select line 10")
    click("Suggest replacement")
    wait('!!document.querySelector(".review-editor")')
    set_text("Suggested replacement", "}  // end invoiceTotal")
    click("Save suggestion")
    wait('document.body.innerText.includes("Saved line comments (3)")')
    pick("Select line 6")
    click("Comment on selection")
    wait('!!document.querySelector(".review-editor")')
    set_text("Line comment", "Nit: the tax variable could be inlined.")
    severity("nit")
    click("Save line comment")
    wait('document.body.innerText.includes("Saved line comments (4)")')
    evaluate('Array.from(document.querySelectorAll("button")).filter(b=>b.textContent.trim()==="Resolve thread")[0].click()')
    wait('document.body.innerText.includes("(resolved)")')
    evaluate('document.querySelector(".diff-content").scrollTop=document.querySelector(".diff-content").scrollHeight')
    capture("pr-review-lines-threads.png", "Saved threads show side, range, severity and suggestion; a resolved thread is kept locally and not posted.", ["base", "current", "(resolved)", "Resolved threads are kept locally and not posted"])
    click("Preview submission")
    wait('!!document.querySelector("[aria-label=\\"Confirm PR review\\"]")')
    capture("pr-review-lines-preview.png", "The preview lists exactly what would post (ranges, sides, suggestion block); nothing is sent until the explicit confirm.", ["Confirm and post to GitHub", "invoice.ts:8–9 (current · RIGHT)", "invoice.ts:8 (base · LEFT)"])
    assert ai_calls() == [] and gh_writes() == []
    click("Keep editing")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
