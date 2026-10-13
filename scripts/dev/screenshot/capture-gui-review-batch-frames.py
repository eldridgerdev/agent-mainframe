#!/usr/bin/env python3
"""Assert native Final Review batch preparation through WebKit IPC and capture its X11 window.

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
    raise AssertionError(expression)


def click(text):
    wait(f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled)')
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
    for expected in expects:
        wait(f"document.body.innerText.includes({json.dumps(expected)})")
    if expression:
        wait(expression)
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
    wait(f'Array.from(document.querySelectorAll(".diff-file")).some(b=>b.querySelector("span").textContent==={json.dumps(path)} && !b.disabled)')
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    ready()


def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def save(label):
    click(label)
    wait('!document.querySelector(".review-editor")')
    ready()


config_path = pathlib.Path(sys.argv[4])
with sqlite3.connect(config_path.with_name("amf.db")) as db:
    repo = pathlib.Path(db.execute("SELECT workdir FROM features WHERE id='shot-feature'").fetchone()[0])
claude = repo / ".claude"
claude.mkdir(exist_ok=True)
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n*-received.json\n")
progress_path = claude / "final-review-progress.json"
source_path = repo / "invoice.ts"
blocked_path = repo / "invoice.test.ts"
original_source = source_path.read_text()
original_blocked = blocked_path.read_text()
blocked_mode = blocked_path.stat().st_mode
upper = "  const total = invoice.subtotal + tax;\n  const cents = total + Number.EPSILON;\n  return Math.round(cents * 100) / 100;"
lower = '  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(total);'
blocked_replacement = 'import { invoiceTotal as calculateTotal } from "./invoice";'
stale_refusal = "Review changes changed; refresh changes before continuing"


def progress():
    return json.loads(progress_path.read_text())


def select_line(number, extend=False):
    label = f"Select line {number}"
    evaluate(f"document.querySelector('button[aria-label={json.dumps(label)}]').dispatchEvent(new MouseEvent('click',{{bubbles:true,shiftKey:{str(extend).lower()}}}))")
    wait(f"document.querySelector('button[aria-label={json.dumps(label)}]')?.getAttribute('aria-pressed')==='true'")


def suggest(start, end, replacement):
    select_line(start)
    if end != start:
        select_line(end, extend=True)
    click("Suggest replacement")
    fill("Suggested replacement", replacement)
    save("Save suggestion")


def filter_files(value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==='Filter review files');const el=label.querySelector('input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def begin_batch():
    click("Apply pending suggestions")
    wait('document.querySelector("[role=alertdialog]")?.getAttribute("aria-label")==="Apply pending suggestions locally"')


def verdict_is(path, verdict):
    # The file row reads "<verdict>" or "<verdict> · changed since last review".
    return f'Array.from(document.querySelectorAll(".diff-file")).some(b=>b.querySelector("span").textContent==={json.dumps(path)} && b.querySelector("small").textContent.split(" ")[0]==={json.dumps(verdict)})'


def no_completion():
    assert not (claude / "final-review-feedback.md").exists(), "Preparation completed the review"
    for receiver in ["claude-received.json", "codex-received.json"]:
        assert json.loads((repo / receiver).read_text()) == [], "Preparation sent feedback"


try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    suggest(8, 9, upper)
    suggest(13, 16, lower)
    click("Approve file")
    ready()
    choose_file("invoice.test.ts")
    suggest(1, 1, blocked_replacement)
    click("Approve file")
    ready()
    choose_file("receipt.bin")
    click("Skip file")
    ready()
    filter_files("invoice.ts")
    click("Pre-finish summary")
    ready()
    capture("001-summary-includes-hidden-suggestions.png", "The summary includes all three saved suggestions even when the editor filters to invoice.ts; browsing changes no source.", ["3 open suggestion(s)", "2 approved", "invoice.ts", "invoice.test.ts", "receipt.bin", "Apply pending suggestions"])
    evaluate("document.querySelector('.review-summary').lastElementChild.scrollIntoView({block:'end'})")
    capture("002-summary-hidden-file-rows.png", "The summary retains the filtered-out binary row and both saved invoice replacements; it changes no source or saved progress.", ["receipt.bin", upper, lower])
    evaluate("document.querySelector('.review-summary').scrollIntoView({block:'start'})")
    assert source_path.read_text() == original_source
    assert blocked_path.read_text() == original_blocked
    saved_before = progress_path.read_bytes()
    begin_batch()
    capture("003-batch-confirmation.png", "Batch application explicitly confirms checkout writes, successful-thread resolution and approval invalidation without finishing or sending feedback.", ["Apply 3 saved replacement(s)", "Blocked suggestions stay open", "Apply batch locally", "Cancel batch application"])
    click("Cancel batch application")
    wait('!document.querySelector("[role=alertdialog]")')
    assert source_path.read_text() == original_source
    assert progress_path.read_bytes() == saved_before
    no_completion()

    source_path.write_text(original_source + "\n// External edit after opening summary\n")
    changed_source = source_path.read_text()
    begin_batch()
    click("Apply batch locally")
    ready()
    stale_alert = f'Array.from(document.querySelectorAll("[role=alert]")).some(a=>a.textContent==={json.dumps(stale_refusal)})'
    wait(stale_alert)
    capture("004-changed-patch-refuses-entire-batch.png", "An external patch edit refuses the whole batch before any source is written; saved replacements and external text survive.", [stale_refusal, "3 open suggestion(s)", "0 applied locally", "Apply batch locally"], stale_alert, allow_alert=True)
    assert source_path.read_text() == changed_source
    assert blocked_path.read_text() == original_blocked
    assert progress_path.read_bytes() == saved_before
    no_completion()
    click("Cancel batch application")
    click("Return to review")
    click("Refresh changes")
    ready()
    filter_files("")
    # Refreshing already forgot invoice.ts's approval, because the external edit
    # changed its patch. Approve the refreshed patch so the batch below, not the
    # refresh, is what invalidates it.
    wait(verdict_is("invoice.ts", "undecided"))
    assert "invoice.ts" not in progress()["decisions"]
    choose_file("invoice.ts")
    click("Approve file")
    ready()
    wait(verdict_is("invoice.ts", "approved"))
    assert progress()["decisions"]["invoice.ts"] == "Approve"
    assert progress()["decisions"]["invoice.test.ts"] == "Approve"
    click("Pre-finish summary")
    ready()
    wait('document.body.innerText.includes("2 approved")')
    blocked_path.chmod(0o444)
    begin_batch()
    click("Apply batch locally")
    ready()
    wait('document.body.innerText.includes("2 applied locally")')
    capture("005-partial-batch-preserves-blocked-replacement.png", "Two same-file replacements apply bottom-up despite different line counts; a read-only second file stays unchanged and its suggestion remains open with the failure reason.", ["1 open suggestion(s)", "2 applied locally", "Some suggestions could not be applied", "could not write file", "1 approved"], allow_alert=True)
    expected_source = changed_source.replace("  const total = invoice.subtotal + tax;\n  return Math.round(total * 100) / 100;", upper).replace('  return new Intl.NumberFormat("en-US", {\n    style: "currency",\n    currency: "USD",\n  }).format(total);', lower)
    assert source_path.read_text() == expected_source
    assert blocked_path.read_text() == original_blocked
    saved = progress()
    assert len(saved["applied_suggestions"]) == 2
    assert "invoice.ts" not in saved["decisions"]
    assert saved["decisions"]["invoice.test.ts"] == "Approve"
    assert all(c["suggestion"] is None and c["resolved"] for c in saved["line_comments"].get("invoice.ts", []))
    assert saved["line_comments"]["invoice.test.ts"][0]["suggestion"] == blocked_replacement
    no_completion()
    click("Return to review")
    choose_file("invoice.ts")
    capture("006-source-ready-for-rereview.png", "Batch preparation leaves the review open with changed source and invalidated approval, preserving the external edit and showing two application records.", ["Number.EPSILON", "Applied locally (2)", "1 approved"], verdict_is("invoice.ts", "undecided"))
    click("Pause review")
    wait('!document.querySelector("[role=dialog]")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    click("Pre-finish summary")
    ready()
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')
    capture("007-reopen-retains-batch-history-and-failure.png", "Pause/reopen restores application history and the unapplied replacement/failure; reopening does not repeat source writes or finish the review.", ["1 open suggestion(s)", "2 applied locally", "Some suggestions could not be applied", blocked_replacement, "1 approved"], allow_alert=True)
    assert source_path.read_text() == expected_source
    assert progress()["decisions"]["invoice.test.ts"] == "Approve"
    blocked_path.chmod(blocked_mode)
    begin_batch()
    click("Apply batch locally")
    ready()
    wait('document.body.innerText.includes("0 open suggestion(s)")')
    capture("008-retry-applies-only-retained-replacement.png", "After restoring write permission, retry applies only the retained replacement, clears its failure, invalidates that file's approval and leaves all three application records in the open review.", ["0 open suggestion(s)", "3 applied locally", "Applied locally (3)", "0 approved"], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Apply pending suggestions").disabled')
    assert source_path.read_text() == expected_source
    assert blocked_path.read_text() == original_blocked.replace('import { invoiceTotal } from "./invoice";', blocked_replacement)
    saved = progress()
    assert len(saved["applied_suggestions"]) == 3
    assert "invoice.test.ts" not in saved["decisions"]
    no_completion()
    (out / "validation.json").write_text(json.dumps({"native_ipc": True, "applied_suggestions": 3, "same_file_bottom_up": True, "partial_failure_retry": True, "feedback_recorded": False, "prompts_sent": 0}, indent=2) + "\n")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    blocked_path.chmod(blocked_mode)
    ws.close()
    x.close()
