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
check_dir = repo / ".amf"
check_dir.mkdir()
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n.amf/\n")
command = "bash .amf/review-check.sh"
(check_dir / "config.json").write_text(json.dumps({"final_review_check_command": command}))
mode_path = check_dir / "check-mode"
mode_path.write_text("fail")
runs_path = check_dir / "check-runs.log"
(check_dir / "review-check.sh").write_text('''#!/usr/bin/env bash
set -eu
echo started >> .amf/check-runs.log
case "$(cat .amf/check-mode)" in
  fail)
    echo 'Invoice checks: 3 passed, 1 failed'
    echo 'FAIL negative totals: expected -21.64, received -21.63' >&2
    exit 1 ;;
  pass)
    echo 'Invoice checks: 4 passed, 0 failed'
    echo 'Unicode amounts validated: €21.64 · café'
    exit 0 ;;
  slow)
    sleep 30 &
    echo $! > .amf/check-descendant
    wait ;;
esac
''')
progress_path = claude / "final-review-progress.json"
progress_path.write_text(json.dumps({
    "general_feedback": "Verify negative invoice totals before shipping.",
    "decisions": {"invoice.ts": {"Reject": {"feedback": "Add coverage for negative invoice totals.", "severity": "blocker"}}},
}))
original_source = (repo / "invoice.ts").read_bytes()
original_progress = progress_path.read_bytes()


def summary_ready():
    ready()
    wait('!!document.querySelector(".review-summary")')


def check_text(text):
    wait(f'document.querySelector("[aria-label=\\"Project review check\\"]")?.innerText.includes({json.dumps(text)})')
    ready()


def run_check():
    click("Run project check")
    wait('!!document.querySelector("[aria-label=\\"Run project review check\\"]")')
    click("Run check now")
    ready()


def runs():
    return len(runs_path.read_text().splitlines()) if runs_path.exists() else 0


def descendant_ready():
    deadline = time.monotonic() + 10
    while not (check_dir / "check-descendant").exists():
        assert time.monotonic() < deadline, "The isolated check did not start its child"
        time.sleep(0.02)


def check_exited():
    # The native cancellation must kill the shell's descendant too.
    descendant = (check_dir / "check-descendant").read_text().strip()
    for _ in range(100):
        result = subprocess.run(["ps", "-o", "stat=", "-p", descendant], capture_output=True, text=True)
        status = result.stdout.strip()
        if not status or status.startswith("Z"):
            return
        time.sleep(0.02)
    raise AssertionError("Check cancellation left a descendant running")


try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Final Review")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    click("Pre-finish summary")
    summary_ready()
    capture("001-project-check-preview.png", "The pre-finish summary exposes the effective project check beside the full review feedback, even for a stopped feature.", ["Project check", command, "Run project check", "Verify negative invoice totals before shipping."], '!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Run project check").disabled')
    assert runs() == 0

    click("Run project check")
    wait('!!document.querySelector("[role=alertdialog]")')
    capture("002-explicit-command-confirmation.png", "A separate confirmation previews the exact shell command and explains its effects before any process starts.", ["may write build or test artifacts", command, "Run check now", "Cancel check launch"])
    assert runs() == 0
    click("Cancel check launch")
    wait('!document.querySelector("[role=alertdialog]")')
    assert runs() == 0, "Cancelling confirmation launched a check"

    run_check()
    check_text("Check failed:")
    capture("003-failed-check-diagnostics.png", "A failed check shows both output streams and remains in the open review for an explicit retry.", ["Check failed:", "Invoice checks: 3 passed, 1 failed", "FAIL negative totals: expected -21.64, received -21.63", "Return to review"], '!!document.querySelector("[aria-label=\\"Project check output\\"]")')
    assert runs() == 1

    mode_path.write_text("pass")
    run_check()
    check_text("Check passed:")
    capture("004-passing-check-result.png", "Rerunning the same confirmed command replaces the failed result with passing output, including Unicode, without finishing or sending feedback.", ["Check passed:", "Invoice checks: 4 passed, 0 failed", "Unicode amounts validated: €21.64 · café", "Results stay in this open review."])
    assert runs() == 2

    # Local unsaved edits survive the summary and prevent another check launch.
    click("Return to review")
    ready()
    choose_file("invoice.ts")
    original_progress = progress_path.read_bytes()  # existing selection saves progress
    click("Edit file comment")
    draft = "Keep this unsaved multiline draft.\nVerify the negative-total assertion first."
    fill("File comment", draft)
    click("Pre-finish summary")
    summary_ready()
    assert evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Run project check").disabled')
    assert evaluate('document.body.innerText.includes("Your unsaved drafts are retained")')
    click("Return to review")
    ready()
    assert evaluate('document.querySelector(".review-editor textarea").value') == draft
    assert progress_path.read_bytes() == original_progress
    click("Cancel edit")
    click("Discard and continue")
    wait('!document.querySelector(".review-editor")')
    click("Pre-finish summary")
    summary_ready()

    mode_path.write_text("slow")
    run_check()
    check_text("Check running:")
    wait(f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Cancel running check")')
    descendant_ready()
    capture("005-running-check-cancellation.png", "A running check disables duplicate launches and source application while offering explicit cancellation.", ["Check running:", "Cancel running check", "Return to review"], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Run project check").disabled')
    click("Cancel running check")
    check_text("Check cancelled:")
    check_exited()
    capture("006-cancelled-check-result.png", "Cancellation terminates the isolated command and its child process; the cancelled result stays visible and another explicit run is available.", ["Check cancelled:", "Cancelled by reviewer", "Run project check"], '!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Run project check").disabled')
    assert runs() == 3

    (check_dir / "check-descendant").unlink()
    run_check()
    check_text("Check running:")
    descendant_ready()
    (repo / "invoice.ts").write_bytes(original_source + b"\n// Changed externally during the check.\n")
    check_text("Check stale:")
    check_exited()
    capture("007-stale-check-result-discarded.png", "An external source change cancels the in-flight check and discards its obsolete result, asking the reviewer to refresh or reload before running again.", ["Check stale:", "Check cancelled; result discarded", "Refresh or reload before running again.", "Return to review"])
    assert runs() == 4
    (repo / "invoice.ts").write_bytes(original_source)
    assert progress_path.read_bytes() == original_progress, "Running checks unexpectedly saved review progress"
    assert not (claude / "final-review-feedback.md").exists(), "Checks unexpectedly completed or dispatched feedback"
    assert (repo / "invoice.ts").read_bytes() == original_source
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
