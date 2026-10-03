#!/usr/bin/env python3
"""Assert native Final Review states through WebKit IPC and capture its X11 window.

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
(claude / "review-notes.md").write_text("# Review Notes\n\n## invoice.ts\n\nRound invoice totals to cents before formatting them as US dollars.\n")
(claude / "final-review-progress.json").write_text(json.dumps({
    "line_comments": {"invoice.ts": [{"location": {"old_line": None, "new_line": 9}, "text": "Add a negative-total test before approving.", "severity": "nit"}]}
}))

try:
    window.configure(x=0, y=0, width=1100, height=720)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    capture("001-final-review-entry.png", "A stopped Git feature offers Final Review without starting an agent.", ["Stopped", "Final Review", "Round invoice totals"])

    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')
    capture("002-review-notes-and-threads.png", "Final Review reuses the current diff and shows developer notes and a line thread saved in the TUI progress format.", ["Final Review · Round invoice totals", "Approve file", "Reject file", "Math.round(total * 100) / 100", "Add a negative-total test before approving.", "Round invoice totals to cents before formatting"], 'document.querySelectorAll(".diff-lines:not(.diff-split)").length>0')

    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=false)')
    click("Edit file comment")
    wait('!!document.querySelector(".review-editor textarea")')
    fill("File comment", "Should the currency formatter support other locales?")
    select("Severity", "question")
    save("Save comment")
    click("Reject file")
    fill("Rejection feedback", "Handle negative totals before rounding to cents.")
    select("Severity", "blocker")
    save("Save rejection")
    choose_file("invoice.ts")
    select("Review layout", "split")
    capture("003-verdict-and-file-comment.png", "A blocker rejection and a separate question comment are saved beside the side-by-side diff.", ["1 rejected", "[blocker] Handle negative totals before rounding to cents.", "[question] Should the currency formatter support other locales?", "Resolve comment"], 'document.querySelectorAll(".diff-split").length>0')
    progress = json.loads((claude / "final-review-progress.json").read_text())
    assert progress["decisions"]["invoice.ts"]["Reject"]["severity"] == "blocker"
    assert progress["file_comments"]["invoice.ts"]["severity"] == "question"
    assert progress["line_comments"]["invoice.ts"][0]["text"] == "Add a negative-total test before approving."

    click("Overall feedback")
    fill("Overall feedback draft", "Good direction. Please cover negative totals and locale handling.")
    click("Pause review")
    wait('!!document.querySelector("[role=alertdialog]")')
    capture("004-unsaved-draft-protection.png", "Pausing with unsaved overall feedback asks for an explicit discard; Keep editing preserves the draft.", ["Discard unsaved edits and continue?", "Discard and continue", "Keep editing"], 'document.querySelector(".review-editor textarea").value.includes("Good direction")')
    click("Keep editing")
    save("Save overall feedback")
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.test.ts")
    click("Approve file")
    ready()
    wait('document.body.innerText.includes("1 approved")')
    choose_file("invoice.ts")
    capture("005-paused-review-resumed.png", "Reopening restores rejection, file comment, overall feedback and saved line threads; another file can be approved independently.", ["1 approved · 1 rejected · 2 undecided", "Good direction. Please cover negative totals and locale handling.", "Should the currency formatter support other locales?", "Handle negative totals before rounding to cents."])

    choose_file("invoice.test.ts")
    with (repo / "invoice.test.ts").open("a") as test:
        test.write("\nconsole.assert(invoiceTotal({ subtotal: -19.99, taxRate: 0.0825 }) === -21.64);\n")
    click("Approve file")
    wait('!!document.querySelector("[role=alert]")')
    capture("006-changed-patch-rejected.png", "A file changed on disk after it was reviewed cannot receive a verdict until the diff is refreshed.", ["File changed since you opened it; refresh changes before reviewing it"], allow_alert=True)
    click("Refresh changes")
    ready()
    wait('!document.querySelector("[role=alert]") && document.body.innerText.includes("0 approved")')
    capture("007-refresh-invalidates-approval.png", "Refreshing shows the new negative-total test and clears its previous approval while retaining the other review feedback.", ["0 approved · 1 rejected · 3 undecided", "subtotal: -19.99", "Good direction. Please cover negative totals and locale handling."])
    progress = json.loads((claude / "final-review-progress.json").read_text())
    assert "invoice.test.ts" not in progress["decisions"]
    assert progress["general_feedback"].startswith("Good direction")

    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
