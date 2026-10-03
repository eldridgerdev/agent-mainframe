#!/usr/bin/env python3
"""Assert native line/range comments and suggestion editing through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
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
    raise AssertionError(expression)


def click(text):
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
    )


def select(label, value):
    evaluate(
        f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');el.value={json.dumps(value)};el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()"""
    )
    wait(
        '!document.body.innerText.includes("Loading changes…") && !!document.querySelector(".diff-reader")'
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


def ready():
    wait('!document.body.innerText.includes("Updating review…")')


def choose_file(path):
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    ready()


def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def save(label):
    click(label)
    wait('!document.querySelector(".review-editor")')
    ready()


repo = pathlib.Path(sys.argv[4])
claude = repo / ".claude"
claude.mkdir()
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n")
progress_path = claude / "final-review-progress.json"
original_source = (repo / "invoice.ts").read_text()
replacement = "  const total = invoice.subtotal + tax;\n  return Math.round((total + Number.EPSILON) * 100) / 100;"


def select_line(number, extend=False):
    label = f"Select line {number}"
    evaluate(f"""document.querySelector('button[aria-label={json.dumps(label)}]').dispatchEvent(new MouseEvent('click',{{bubbles:true,shiftKey:{str(extend).lower()}}}))""")
    wait(f"document.querySelector('button[aria-label={json.dumps(label)}]')?.getAttribute('aria-pressed')==='true'")


def expand_threads():
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')


def thread():
    saved = json.loads(progress_path.read_text())
    comments = saved["line_comments"]["invoice.ts"]
    assert len(comments) == 1, comments
    comment = comments[0]
    assert comment["start"] == {"old_line": None, "new_line": 8}, comment
    assert comment["location"] == {"old_line": None, "new_line": 9}, comment
    assert not comment["draft"] and not comment["anchor_lost"], comment
    assert (repo / "invoice.ts").read_text() == original_source, "Suggestion authoring changed source code"
    return comment


try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    select_line(8)
    select_line(9, extend=True)
    click("Comment on selection")
    fill("Line comment", "Handle negative totals and test rounding at the cent boundary.")
    select("Severity", "blocker")
    capture("001-range-comment-editor.png", "Clicking line 8 and Shift-clicking line 9 highlights a range and opens a severity-tagged comment draft.", ["Selected line 8 – line 9", "Line comment", "Anchored to line 8 – line 9"], 'document.querySelectorAll(".review-line-selected").length===2 && document.querySelector(".review-editor textarea").value.includes("negative totals")')
    save("Save comment")
    expand_threads()
    capture("002-saved-range-comment.png", "Saving creates a kept line 8–9 thread and marks the file as needing revision through the shared TUI engine.", ["1 rejected", "line 8 – line 9 [blocker] Handle negative totals", "Edit line comment", "Edit suggestion", "Resolve thread"])
    assert thread()["severity"] == "blocker"

    click("Edit suggestion")
    wait('!!document.querySelector(".review-editor textarea")')
    assert evaluate('document.querySelector(".review-editor textarea").value') == "  const total = invoice.subtotal + tax;\n  return Math.round(total * 100) / 100;"
    fill("Suggested replacement", replacement)
    capture("003-suggestion-editor.png", "The replacement editor is seeded from the full saved range and accepts indented code while retaining the thread's prose.", ["Suggested replacement", "Anchored to line 8 – line 9", "Handle negative totals and test rounding"], 'document.querySelector(".review-editor textarea").value.includes("Number.EPSILON")')
    save("Save suggestion")
    select("Review layout", "split")
    expand_threads()
    capture("004-saved-suggestion-split.png", "The saved replacement and blocker comment remain attached to line 8–9 beside the side-by-side diff; the source file stays untouched.", ["1 rejected", "line 8 – line 9 [blocker]", "Number.EPSILON", "Edit suggestion"], '!!document.querySelector(".diff-split")')
    assert thread()["suggestion"] == replacement
    assert thread()["text"] == "Handle negative totals and test rounding at the cent boundary."

    click("Edit suggestion")
    fill("Suggested replacement", replacement + "\n// Unsaved replacement draft")
    click("Pause review")
    wait('!!document.querySelector("[role=alertdialog]")')
    capture("005-unsaved-suggestion-protection.png", "Pausing with an edited replacement asks for explicit discard and keeps the unsaved code in the editor.", ["Discard unsaved edits and continue?", "Keep editing", "Suggested replacement"], 'document.querySelector(".review-editor textarea").value.includes("Unsaved replacement draft")')
    click("Keep editing")
    click("Cancel edit")
    wait('!!document.querySelector("[role=alertdialog]")')
    click("Discard and continue")
    wait('!document.querySelector(".review-editor") && !document.querySelector("[role=alertdialog]")')
    click("Resolve thread")
    ready()
    wait('document.body.innerText.includes("Reopen thread")')
    capture("006-resolved-range-thread.png", "Resolving the kept range marks it settled and clears its automatic rejection; Reopen thread remains available.", ["0 approved · 0 rejected · 4 undecided", "(resolved)", "Reopen thread", "Number.EPSILON"])
    assert thread()["resolved"]

    click("Reopen thread")
    ready()
    click("Edit line comment")
    fill("Line comment", "Also cover negative totals in a regression test.")
    select("Severity", "question")
    save("Save comment")
    assert thread()["suggestion"] == replacement
    assert not thread()["resolved"]
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    expand_threads()
    capture("007-range-and-suggestion-resumed.png", "Pause and reopen restores the reopened line 8–9 thread, edited question prose and unchanged replacement code from shared review progress.", ["1 rejected", "line 8 – line 9 [question] Also cover negative totals in a regression test.", "Number.EPSILON", "Resolve thread"])
    assert thread()["text"] == "Also cover negative totals in a regression test."
    assert thread()["suggestion"] == replacement
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
