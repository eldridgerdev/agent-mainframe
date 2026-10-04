#!/usr/bin/env python3
"""Assert native local suggestion application through WebKit IPC and capture its X11 window.

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
source_path = repo / "invoice.ts"
original_source = source_path.read_text()
replacement = "  const total = invoice.subtotal + tax;\n  return Math.round((total + Number.EPSILON) * 100) / 100;"
source_mode = source_path.stat().st_mode
claude_mode = claude.stat().st_mode


def select_line(number, extend=False):
    label = f"Select line {number}"
    evaluate(f"document.querySelector('button[aria-label={json.dumps(label)}]').dispatchEvent(new MouseEvent('click',{{bubbles:true,shiftKey:{str(extend).lower()}}}))")
    wait(f"document.querySelector('button[aria-label={json.dumps(label)}]')?.getAttribute('aria-pressed')==='true'")


def expand_threads():
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')


def progress():
    return json.loads(progress_path.read_text())


def application_count():
    return len(progress()["applied_suggestions"])


def begin_application():
    click("Apply suggestion locally")
    wait('document.querySelector("[role=alertdialog]")?.getAttribute("aria-label")==="Apply suggestion locally"')


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
    fill("Line comment", "Use an explicit rounding adjustment at the cent boundary.")
    select("Severity", "blocker")
    save("Save comment")
    expand_threads()
    click("Edit suggestion")
    fill("Suggested replacement", replacement)
    save("Save suggestion")
    click("Approve file")
    ready()
    choose_file("invoice.ts")
    select("Review layout", "split")
    expand_threads()
    capture("001-saved-suggestion-ready.png", "A saved line 8–9 replacement exposes Apply suggestion locally while the file remains approved and the checkout is untouched.", ["1 approved", "Number.EPSILON", "Apply suggestion locally", "line 8 – line 9 [blocker]"], 'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Apply suggestion locally" && !b.disabled)')
    assert source_path.read_text() == original_source
    assert progress()["line_comments"]["invoice.ts"][0]["suggestion"] == replacement

    begin_application()
    capture("002-explicit-application-confirmation.png", "Applying requires confirmation that the saved whole-range replacement writes to the checkout, settles its thread and requires the changed code to be reviewed again.", ["This writes to your checkout", "Apply replacement", "Cancel application", "line 8 – line 9"], '!!document.querySelector("[role=alertdialog]")')
    click("Cancel application")
    wait('!document.querySelector("[role=alertdialog]")')
    assert source_path.read_text() == original_source, "Cancelling application changed source"
    assert application_count() == 0
    begin_application()
    click("Apply replacement")
    ready()
    wait('!document.querySelector("[role=alertdialog]") && document.body.innerText.includes("Applied locally (1)")')
    expand_threads()
    capture("003-applied-source-and-invalidated-approval.png", "The replacement now appears in the real source diff, its prose is resolved, its suggestion is consumed, application history is saved, and the old approval is cleared.", ["0 approved", "Applied locally (1)", "Number.EPSILON", "(resolved)", "does not undo source changes"], '!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Apply suggestion locally")')
    applied_source = original_source.replace("  return Math.round(total * 100) / 100;", "  return Math.round((total + Number.EPSILON) * 100) / 100;")
    assert source_path.read_text() == applied_source
    saved = progress()
    assert saved["line_comments"]["invoice.ts"][0]["resolved"]
    assert saved["line_comments"]["invoice.ts"][0]["suggestion"] is None
    assert "invoice.ts" not in saved["decisions"]
    assert application_count() == 1

    select_line(14)
    click("Suggest replacement")
    fill("Suggested replacement", '    style: "decimal",')
    save("Save suggestion")
    expand_threads()
    source_path.write_text(applied_source + "\n// Changed outside AMF\n")
    externally_changed = source_path.read_text()
    begin_application()
    click("Apply replacement")
    ready()
    wait('document.body.innerText.includes("File changed since you opened it")')
    capture("004-stale-source-refused.png", "An edit outside AMF makes local application fail with a refresh instruction; the saved replacement remains available and the external edit is preserved.", ["File changed since you opened it", "refresh changes", 'style: "decimal"', "Apply replacement"], allow_alert=True)
    assert source_path.read_text() == externally_changed
    assert application_count() == 1
    assert any(c.get("suggestion") == '    style: "decimal",' for c in progress()["line_comments"]["invoice.ts"])
    click("Cancel application")
    click("Refresh changes")
    ready()
    choose_file("invoice.ts")
    expand_threads()

    source_path.chmod(0o444)
    begin_application()
    click("Apply replacement")
    ready()
    wait('document.body.innerText.includes("could not write file")')
    capture("005-source-write-failure-retains-suggestion.png", "A read-only source file refuses the write without consuming its replacement or adding an application record.", ["could not write file", 'style: "decimal"', "Applied locally (1)", "Cancel application"], allow_alert=True)
    assert source_path.read_text() == externally_changed
    assert application_count() == 1
    source_path.chmod(source_mode)
    click("Cancel application")

    claude.chmod(0o555)
    begin_application()
    click("Apply replacement")
    ready()
    wait('document.body.innerText.includes("Progress was not saved:") && document.body.innerText.includes("Applied locally (2)")')
    expand_threads()
    capture("006-progress-save-failure-offers-retry.png", "When source application succeeds but saving review progress fails, the GUI retains both applications in memory, shows Retry save and explains that discarding progress will not undo source changes.", ["Progress was not saved:", "Retry save", "Applied locally (2)", "does not undo source changes", 'style: "decimal"'], allow_alert=True)
    final_source = externally_changed.replace('    style: "currency",', '    style: "decimal",')
    assert source_path.read_text() == final_source
    assert application_count() == 1, "The failed progress save unexpectedly committed"
    claude.chmod(claude_mode)
    click("Retry save")
    ready()
    wait('!document.body.innerText.includes("Progress was not saved:")')
    assert application_count() == 2
    assert all(c.get("suggestion") is None for c in progress()["line_comments"]["invoice.ts"])
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    expand_threads()
    capture("007-applied-history-resumed-after-retry.png", "Retry save commits the application records; pause and reopen restores both records, consumed suggestions and the changed source through real Rust IPC.", ["Applied locally (2)", "Number.EPSILON", 'style: "decimal"', "(resolved)"], '!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Apply suggestion locally")')
    assert source_path.read_text() == final_source
    assert application_count() == 2
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    source_path.chmod(source_mode)
    claude.chmod(claude_mode)
    ws.close()
    x.close()
