#!/usr/bin/env python3
"""Assert native Final Review completion and feedback handoff through WebKit IPC.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
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
db_path = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"
claude = repo / ".claude"
claude.mkdir()
check_dir = repo / ".amf"
check_dir.mkdir()
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n.amf/\n")
command = "bash .amf/review-check.sh"
(check_dir / "config.json").write_text(json.dumps({"final_review_check_command": command}))
mode_path = check_dir / "check-mode"
mode_path.write_text("gated")
runs_path = check_dir / "check-runs.log"
(check_dir / "review-check.sh").write_text("""#!/usr/bin/env bash
set -eu
echo started >> .amf/check-runs.log
case "$(cat .amf/check-mode)" in
  fail)
    echo 'Invoice checks: 3 passed, 1 failed'
    echo 'FAIL negative totals: expected -21.64, received -21.63' >&2
    exit 1 ;;
  gated)
    while [ ! -e .amf/check-release ]; do sleep 0.05; done
    echo 'Invoice checks: 4 passed, 0 failed' ;;
esac
""")
progress_path = claude / "final-review-progress.json"
progress_path.write_text(json.dumps({
    "general_feedback": "Verify negative invoice totals before shipping.",
    "decisions": {"invoice.ts": {"Reject": {"feedback": "Add coverage for negative invoice totals.", "severity": "blocker"}}},
}))
feedback_path = claude / "final-review-feedback.md"
original_source = (repo / "invoice.ts").read_bytes()


def bump_store(statement, params):
    # The feature stays stopped: its agent session row has no tmux window, so
    # the handoff can only become an unsent composer draft. No agent runs.
    with sqlite3.connect(db_path) as db:
        db.execute(statement, params)
        version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
        db.execute("INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)", (int(version[0]) + 1,))


bump_store(
    "INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
    ("shot-agent", "shot-feature", "claude", "Claude 1", "claude", "2026-10-05T18:00:00Z", 0),
)


def button(text):
    return f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)})'


def summary_ready():
    ready()
    wait('!!document.querySelector(".review-summary")')


def runs():
    return len(runs_path.read_text().splitlines()) if runs_path.exists() else 0


def open_confirmation():
    click("Complete review…")
    wait('!!document.querySelector("[aria-label=\\"Complete Final Review\\"][role=alertdialog]")')


try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review") && document.body.innerText.includes("Claude 1")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    click("Pre-finish summary")
    summary_ready()
    original_progress = progress_path.read_bytes()
    capture("001-complete-review-entry.png", "The pre-finish summary now offers completing the review, with the round's verdict and comment counts beside the configured project check.", ["Complete review", "0 approved · 1 need work", "Complete review…", command], f'!{button("Complete review…")}.disabled')

    # An unsaved local editor keeps completion disabled and survives the summary.
    click("Return to review")
    ready()
    choose_file("invoice.ts")
    original_progress = progress_path.read_bytes()
    click("Edit file comment")
    draft = "Keep this unsaved multiline draft.\nCheck the rounding helper too."
    fill("File comment", draft)
    click("Pre-finish summary")
    summary_ready()
    capture("002-unsaved-draft-blocks-completion.png", "An unsaved comment draft disables completion; the summary says it is retained rather than discarding or saving it.", ["Your unsaved drafts are retained", "Complete review…"], f'{button("Complete review…")}.disabled')
    click("Return to review")
    ready()
    assert evaluate('document.querySelector(".review-editor textarea").value') == draft
    assert progress_path.read_bytes() == original_progress
    click("Cancel edit")
    click("Discard and continue")
    wait('!document.querySelector(".review-editor")')
    click("Pre-finish summary")
    summary_ready()

    open_confirmation()
    capture("003-explicit-completion-confirmation.png", "Completion is explicitly confirmed: it reruns the configured check, records the round, and offers handing the feedback prompt to the stopped Claude session as an unsent draft, or completing without handoff.", ["Record this round in .claude/final-review-feedback.md", "Runs the configured check first", "earlier results shown here are not reused", "unsent draft in Claude 1 (stopped, so it cannot be sent yet)", "Complete and hand off to Claude 1", "Complete without handoff", "Keep reviewing"])
    assert runs() == 0 and not feedback_path.exists()

    # A patch changed after the confirmation opened is refused before any write.
    (repo / "invoice.ts").write_bytes(original_source + b"\n// Changed outside the review.\n")
    click("Complete and hand off to Claude 1")
    wait('document.body.innerText.includes("Review changes changed")')
    ready()
    capture("004-changed-patch-refused.png", "Changing the reviewed source after confirming is refused before the check starts or anything is written; the confirmation stays open.", ["Review changes changed; refresh changes before continuing", "Complete and hand off to Claude 1"], allow_alert=True)
    assert runs() == 0 and not feedback_path.exists()
    assert progress_path.read_bytes() == original_progress
    (repo / "invoice.ts").write_bytes(original_source)

    click("Complete and hand off to Claude 1")
    wait('document.body.innerText.includes("Completing: the configured check is running")')
    ready()
    capture("005-completion-check-running.png", "Confirming reruns the configured check; nothing is recorded until it finishes, and the reviewer can cancel the completion.", ["Completing: the configured check is running", "Cancel completion", "Check running:"], f'{button("Return to review")}.disabled')
    assert runs() == 1 and not feedback_path.exists()
    click("Cancel completion")
    wait('document.body.innerText.includes("Check cancelled:")')
    ready()
    capture("006-completion-cancelled.png", "Cancelling stops the check and leaves the review open: nothing was written, and completion can be confirmed again.", ["Review not completed; nothing was written.", "Complete review…"])
    assert not feedback_path.exists()
    assert progress_path.read_bytes() == original_progress

    mode_path.write_text("fail")
    open_confirmation()
    click("Complete and hand off to Claude 1")
    wait('!document.querySelector(".review-summary") && !!document.querySelector("textarea[aria-label=\\"Draft prompt\\"]")')
    wait('document.querySelector("textarea[aria-label=\\"Draft prompt\\"]").value.includes("final-review-feedback.md")')
    capture("007-feedback-handed-to-unsent-draft.png", "The failed check is recorded with the rejection, the review closes, and the address-the-feedback prompt waits as an unsent draft in Claude 1's composer. No agent received anything.", ["Final Review", "check `bash .amf/review-check.sh` FAILED", "unsent draft", "Claude 1"])
    assert runs() == 2
    round_text = feedback_path.read_text()
    assert round_text.count("## Review") == 1, round_text
    for text in ["FAILED", "FAIL negative totals", "Add coverage for negative invoice totals.", "Verify negative invoice totals before shipping."]:
        assert text in round_text, (text, round_text)
    assert not progress_path.exists(), "Completion left saved progress behind"
    assert (repo / "invoice.ts").read_bytes() == original_source
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    bump_store("DELETE FROM feature_sessions WHERE id=?", ("shot-agent",))
    ws.close()
    x.close()
