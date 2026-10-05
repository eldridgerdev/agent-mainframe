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
progress_path = claude / "final-review-progress.json"
live_path = claude / "final-review-feedback.md"
archive_path = claude / "final-review-feedback-archive.md"
progress_path.write_text(json.dumps({
    "general_feedback": "Keep the rounding change; verify negative totals before shipping.",
    "decisions": {"invoice.ts": {"Reject": {"feedback": "Cover negative totals.", "severity": "blocker"}}},
    "file_comments": {"invoice.ts": {"text": "Should formatting support other locales?", "severity": "question", "resolved": False, "carried": False}},
    "line_comments": {"invoice.ts": [{"location": {"old_line": None, "new_line": 9}, "text": "Rounding behavior is now explained.", "severity": "nit", "resolved": True, "suggestion": "return Math.round(total * 100) / 100;"}]},
}))
live_path.write_text("""# Final Review Feedback

## Review — 2026-10-04T18:00:00Z

**Files reviewed:** 4 | **Approved:** 3 | **Needs work:** 1

**Check:** `npm test` — FAILED

```
Negative-total assertion failed: expected -21.64.
```

#### invoice.ts:9 — [blocker] (unresolved from a previous round)

Please test rounding for negative invoice totals.

```suggestion
return Math.round(total * 100) / 100;
```

**Agent:** Added a regression test; please review the negative-total case again.

## Review — 2026-10-02T12:00:00Z

**Check:** `npm test` — passed

Currency formatting should remain separate from rounding.
""")
archive_path.write_text("""# Final Review Feedback Archive

## Review — 2026-09-29T12:00:00Z

**Files reviewed:** 2 | **Approved:** 1 | **Needs work:** 1

#### invoice.ts:7 — [suggestion]

Calculate tax once before adding it to the subtotal.

**Agent:** Extracted the tax calculation into a local constant.

## Review — 2026-10-01T12:00:00Z

**Check:** `npm test` — passed

Round the total after calculating tax.
""")
original_live = live_path.read_bytes()
original_archive = archive_path.read_bytes()
original_source = (repo / "invoice.ts").read_bytes()
draft = "Keep this unsaved question.\nShould the formatter accept a locale argument?"


def history_ready():
    ready()
    wait('!!document.querySelector(".review-history-body")')


def choose_round(title):
    evaluate(f'Array.from(document.querySelectorAll(".review-history .diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(title)}).click()')
    wait(f'document.querySelector(".review-history .diff-file[aria-pressed=true] span")?.textContent==={json.dumps(title)}')
    history_ready()


try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.ts")
    # Selecting a file uses the existing saved-review action. Measure history
    # writes only after this unrelated setup navigation has completed.
    original_progress = progress_path.read_bytes()
    click("Edit file comment")
    fill("File comment", draft)
    capture("001-history-entry-with-local-draft.png", "Final Review now offers Review history while an unsaved multiline comment stays in its editor.", ["Review history", "Save comment", "Cover negative totals."], 'document.querySelector(".review-editor textarea").value===' + json.dumps(draft))

    click("Review history")
    history_ready()
    capture("002-current-review-history.png", "Current projects the open review, including verdicts, saved feedback and resolved threads; local editor text is retained separately.", ["Current Review", "Keep the rounding change", "Should formatting support other locales?", "nit · resolved", "Return to review", "Load older rounds"], 'document.querySelectorAll(".review-history .diff-file").length===3 && !document.querySelector(".review-editor") && !document.body.innerText.includes("2026-09-29")')

    choose_round("Review — 2026-10-04T18:00:00Z")
    capture("003-completed-round-feedback.png", "A completed round preserves its failed check, carried blocker, suggested code and agent reply in read-only form.", ["Negative-total assertion failed", "1 carried unresolved", "Please test rounding for negative invoice totals.", "return Math.round(total * 100) / 100;", "Added a regression test"], '!document.querySelector(".review-editor") && !!document.querySelector(".review-history-body pre code")')

    click("Load older rounds")
    history_ready()
    wait('document.querySelectorAll(".review-history .diff-file").length===5')
    assert evaluate('Array.from(document.querySelectorAll(".review-history .diff-file span")).map(e=>e.textContent)') == ["Current", "Review — 2026-10-04T18:00:00Z", "Review — 2026-10-02T12:00:00Z", "Review — 2026-10-01T12:00:00Z", "Review — 2026-09-29T12:00:00Z"]
    choose_round("Review — 2026-09-29T12:00:00Z")
    capture("004-archived-review-round.png", "Load older rounds appends the archive newest first; the earliest round retains its original feedback and agent reply.", ["Review — 2026-10-01T12:00:00Z", "Review — 2026-09-29T12:00:00Z", "Calculate tax once", "Extracted the tax calculation"], '!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Load older rounds")')

    click("Return to review")
    ready()
    wait('!!document.querySelector(".review-editor textarea")')
    capture("005-unsaved-comment-restored.png", "Returning from history restores the exact unsaved multiline comment without saving progress or changing source code.", ["Review history", "Save comment", "Cancel edit"], 'document.querySelector(".review-editor textarea").value===' + json.dumps(draft))

    archive_path.write_bytes(bytes([255]))
    click("Review history")
    history_ready()
    click("Load older rounds")
    history_ready()
    wait('!!document.querySelector("[role=alert]")')
    capture("006-archive-read-failure.png", "An unreadable archive reports the error while Current and completed live rounds remain accessible and the editor can be reopened.", ["Could not read archived review history", "Current", "Review — 2026-10-04T18:00:00Z", "Return to review"], 'document.querySelectorAll(".review-history .diff-file").length===3', allow_alert=True)
    archive_path.write_bytes(original_archive)
    click("Return to review")
    ready()
    click("Review history")
    history_ready()
    click("Load older rounds")
    history_ready()
    wait('document.querySelectorAll(".review-history .diff-file").length===5')
    choose_round("Review — 2026-10-01T12:00:00Z")
    capture("007-history-retry-after-repair.png", "Reopening history retries the repaired archive and returns to a readable older round without losing the local draft.", ["Round the total after calculating tax.", "Check:", "passed", "Return to review"], '!document.querySelector("[role=alert]")')
    click("Return to review")
    ready()
    assert evaluate('document.querySelector(".review-editor textarea").value') == draft
    assert progress_path.read_bytes() == original_progress, "History unexpectedly wrote review progress"
    assert live_path.read_bytes() == original_live
    assert archive_path.read_bytes() == original_archive
    assert (repo / "invoice.ts").read_bytes() == original_source
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
