#!/usr/bin/env python3
"""Assert native supervised-edit states through WebKit IPC and capture its X11 window.

The pending edits come from AMF's real Claude diff-review hook script, fed
offline hook input: no agent or paid harness runs. With no AMF socket in the
isolated state directory the hook takes its file fallback, which is the path
the GUI answers. Only the window of the supplied isolated GUI PID is used.
"""

import hashlib
import json
import os
import signal
import sqlite3
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


def grab(path):
    g = window.get_geometry()
    subprocess.run(
        [
            "/usr/bin/python3",
            "-c",
            """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
            str(window.id),
            str(g.width),
            str(g.height),
            str(path),
        ],
        check=True,
    )


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
    # Focus restoration is asserted before this call. Blur only the terminal
    # while taking the image so its blinking cursor cannot prevent two stable
    # frames or outlive the short result toast. This sends no input to tmux.
    evaluate('if(document.activeElement?.classList.contains("xterm-helper-textarea")) document.activeElement.blur()')
    # WebKit can paint well after the DOM settles under a software renderer.
    # Keep grabbing until two grabs agree and differ from every earlier frame.
    previous = None
    for _ in range(40):
        time.sleep(0.4)
        grab(out / name)
        digest = hashlib.sha256((out / name).read_bytes()).digest()
        if digest == previous and digest not in captured_frames:
            break
        previous = digest
    else:
        raise AssertionError(f"The native window never painted a new stable frame for {name}")
    captured_frames.add(digest)
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, ("Expected text disappeared before the frame painted", text, body)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)


def fill(label, value):
    evaluate(f"""(()=>{{const el=document.querySelector('textarea[aria-label='+JSON.stringify({json.dumps(label)})+']');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


workspace = pathlib.Path(__file__).resolve().parents[3]
repo = pathlib.Path(sys.argv[4])
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n")
hook_script = workspace / "plugins/diff-review/scripts/custom-diff-review.sh"
amf_bin = os.environ.get("AMF_BIN") or str(workspace / "target/debug/amf")
assert os.access(amf_bin, os.X_OK), f"amf binary missing: {amf_bin}"
dbpath = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"
with sqlite3.connect(dbpath) as db:
    db.execute("UPDATE features SET mode='vibeless' WHERE id='shot-feature'")
    version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
    db.execute("INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)", ((int(version[0]) if version else 0) + 1,))
notify_dir = repo / ".claude/notifications"
hooks = []


def start_hook(tool, tool_input):
    """Run the shipped hook exactly as Claude Code would for one tool call."""
    payload = {
        "tool_name": tool,
        "session_id": f"shot-claude-{len(hooks)}",
        "cwd": str(repo),
        "tool_input": tool_input,
    }
    before = set(notify_dir.glob("*.json")) if notify_dir.exists() else set()
    hook = subprocess.Popen(
        ["bash", str(hook_script), str(repo)],
        cwd=repo,
        env=os.environ | {"AMF_ACTIVE": "1", "AMF_BIN": amf_bin, "AMF_SESSION": "amf-gui-shot-unused"},
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    hook.stdin.write(json.dumps(payload))
    hook.stdin.close()
    hooks.append(hook)
    deadline = time.monotonic() + 15
    while True:
        assert hook.poll() is None, ("hook exited early", hook.returncode, hook.stderr.read())
        created = set(notify_dir.glob("*.json")) - before if notify_dir.exists() else set()
        if created:
            notification = created.pop()
            time.sleep(0.2)
            return hook, notification, json.loads(notification.read_text())
        assert time.monotonic() < deadline, "The hook never fell back to a notification file"
        time.sleep(0.05)


def finish(hook):
    hook.wait(timeout=15)
    return hook.returncode, hook.stderr.read()


def open_feature():
    evaluate('document.querySelector("button.tree-name[title^=\\"Round invoice totals\\"]").click()')
    wait('document.body.innerText.includes("Final Review")')


def edit_ready(path):
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    wait('!!document.querySelector("[aria-label=\\"Pending edit\\"]")')


def confirm_open():
    wait('!!document.querySelector("[role=alertdialog][aria-label=\\"Confirm answer\\"]")')
    evaluate('document.querySelector("[role=alertdialog][aria-label=\\"Confirm answer\\"]").scrollIntoView({block:"nearest"})')


tmux = ["tmux", "-S", os.environ["AMF_TMUX_SOCKET"]]
subprocess.run(tmux + ["new-session", "-d", "-s", "amf-gui-shot-unused", "-n", "claude", "-x", "120", "-y", "30",
                      "printf 'Offline Claude stand-in: waiting for a supervised edit\\n'; sleep 600"], check=True)
with sqlite3.connect(dbpath) as db:
    db.execute("UPDATE features SET status='idle' WHERE id='shot-feature'")
    db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
               ("shot-agent", "shot-feature", "claude", "Claude", "claude", "2026-10-07T00:00:00Z", 0))
    db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")

original_source = (repo / "invoice.ts").read_bytes()
try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('!!document.querySelector("button.tree-name[title^=\\"Round invoice totals\\"]")')
    open_feature()
    wait('document.body.innerText.includes("Vibeless")')
    wait('!!document.querySelector(".xterm-screen")')
    evaluate('document.querySelector(".xterm-helper-textarea").focus()')
    # Let the first count poll settle so the arrival below is announced.
    time.sleep(2.5)

    edit_input = {
        "file_path": str(repo / "invoice.ts"),
        "old_string": "  const tax = invoice.subtotal * invoice.taxRate;",
        "new_string": "  // Credits (negative subtotals) carry no tax.\n  const tax = Math.max(0, invoice.subtotal * invoice.taxRate);",
    }
    approve_hook, approve_file, approve_request = start_hook("Edit", edit_input)
    assert approve_request["type"] == "diff-review"
    wait('document.querySelector(".nav-count-attention")?.textContent==="1"')
    wait('document.body.innerText.includes("Edit waiting for review")')
    wait('!!document.querySelector("[role=dialog][aria-label=\\"Supervised edits\\"]")')
    edit_ready("invoice.ts")
    assert evaluate('document.activeElement.closest(".supervised-popup")!==null'), "The popup did not take focus"
    capture("000-popup-over-agent.png", "A waiting Vibeless edit opens automatically over the running offline agent tab, with its captured diff. The reviewer has not clicked Review or answered the hook.", ["Supervised edits · Round invoice totals", "invoice.ts", "Approve edit", "Reject edit"], '!!document.querySelector(".xterm-screen")')
    assert approve_hook.poll() is None, "Automatic opening answered the hook"
    evaluate('document.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true}))')
    wait('!document.querySelector("[role=dialog]")')
    assert evaluate('document.activeElement.classList.contains("xterm-helper-textarea")'), "Dismissal did not restore terminal focus"
    assert approve_hook.poll() is None, "Dismissing answered the hook"
    capture("001-waiting-edit-announced.png", "Dismissing the popup restores the agent terminal without answering: the hook stays blocked, and the feature badges and Supervised edits action still reopen it.", ["Supervised edits", "Compose prompt"], 'document.querySelectorAll(".nav-count-attention").length===2')

    # Reopen through the feature action, which remains after the toast expires.
    evaluate('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim().startsWith("Supervised edits")).click()')
    edit_ready("invoice.ts")
    select("Layout", "split")
    wait('!!document.querySelector("table.diff-split")')
    capture("002-pending-edit-diff.png", "The panel shows the exact change the hook captured, from the hook's own original and proposed copies, in a side-by-side diff.", ["Supervised edits · Round invoice totals", "invoice.ts", "Edit", "Waiting", "Credits (negative subtotals) carry no tax.", "Approve edit", "Reject edit", "Cancel edit"])
    assert approve_hook.poll() is None, "Opening the panel answered the hook"

    wait('!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Approve edit").disabled')
    click("Approve edit")
    confirm_open()
    capture("003-approval-confirmation.png", "Answering an agent is a separate, explicit step that states the effect before anything is sent.", ["Approve this edit?", "The agent writes this change.", "Send approval", "Back"])
    assert approve_hook.poll() is None, "Opening the confirmation answered the hook"
    click("Send approval")
    code, stderr = finish(approve_hook)
    assert code == 0, ("approval did not let the write proceed", code, stderr)
    wait('document.body.innerText.includes("No edits are waiting for review")')
    capture("004-approved-agent-continues.png", "The hook read the approval and let the agent continue (exit 0); the request left the queue, the manually opened panel stays open in its empty state, and AMF did not write the source itself.", ["Approved the edit to invoice.ts", "No edits are waiting for review"], 'document.querySelectorAll(".nav-count-attention").length===0')
    assert not approve_file.exists()
    assert (repo / "invoice.ts").read_bytes() == original_source, "AMF wrote source while answering"

    feedback = "Keep credits in invoice.ts; no new module — café €"
    write_hook, write_file, _ = start_hook("Write", {
        "file_path": str(repo / "credit.ts"),
        "content": "export function creditTotal(amount: number): number {\n  return -Math.abs(amount);\n}\n",
    })
    edit_ready("credit.ts")
    fill("Feedback for the agent", feedback)
    wait('!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Reject edit").disabled')
    click("Reject edit")
    confirm_open()
    capture("005-new-file-rejection-feedback.png", "A proposed new file is shown as added in unified layout; rejecting it previews the exact feedback the agent will receive.", ["credit.ts", "new file", "Reject this edit?", "receives your feedback", feedback, "Send rejection"], '!!document.querySelector("table[aria-label=\\"Unified hunk\\"]")')
    click("Send rejection")
    code, stderr = finish(write_hook)
    assert code == 2, ("rejection did not block the write", code, stderr)
    assert f"User rejected this change with feedback: {feedback}" in stderr, stderr
    wait('document.body.innerText.includes("No edits are waiting for review")')
    wait('document.body.innerText.includes("Rejected the edit to credit.ts")')
    capture("006-rejected-feedback-delivered.png", "The hook blocked the write and handed the feedback to the agent (exit 2 with the reviewer's text); no file was created, and the manual review remains open for the next request.", ["Rejected the edit to credit.ts", "No edits are waiting for review"])
    assert not (repo / "credit.ts").exists()

    guide_hook, guide_file, guide_request = start_hook("Edit", {
        "file_path": str(repo / "GUIDE.md"),
        "old_string": "Simple invoice calculations.",
        "new_string": "Invoice calculations, rounded to cents.",
    })
    edit_ready("GUIDE.md")
    wait('!Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Approve edit").disabled')
    click("Approve edit")
    confirm_open()
    # Another interface answers first. Pause the hook so the answer is still
    # unread when the GUI tries to send its own.
    os.killpg(guide_hook.pid, signal.SIGSTOP)
    external = {"type": "review-response", "decision": "reject", "reason": "Answered in the TUI", "skip": False, "reject": True}
    pathlib.Path(guide_request["response_file"]).write_text(json.dumps(external))
    pathlib.Path(guide_request["proceed_signal"]).write_text("")
    click("Send approval")
    wait('document.querySelector("[role=alert]")?.textContent.includes("already answered")')
    assert json.loads(pathlib.Path(guide_request["response_file"]).read_text()) == external, "The GUI overwrote another interface's answer"
    os.killpg(guide_hook.pid, signal.SIGCONT)
    code, stderr = finish(guide_hook)
    assert code == 2 and "Answered in the TUI" in stderr, (code, stderr)
    wait('document.body.innerText.includes("The edit to GUIDE.md is no longer waiting for review")')
    wait('document.body.innerText.includes("No edits are waiting for review")')
    capture("007-answered-elsewhere-refused.png", "An edit answered from another interface is refused rather than answered twice: the GUI keeps the first answer intact and says the edit has left the queue.", ["already answered", "The edit to GUIDE.md is no longer waiting for review", "No edits are waiting for review"], None, allow_alert=True)
    assert not guide_file.exists()
    assert (repo / "invoice.ts").read_bytes() == original_source
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    subprocess.run(tmux + ["kill-server"], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    with sqlite3.connect(dbpath) as db:
        db.execute("DELETE FROM feature_sessions WHERE id='shot-agent'")
        db.execute("UPDATE features SET status='stopped' WHERE id='shot-feature'")
    for hook in hooks:
        if hook.poll() is None:
            try:
                os.killpg(hook.pid, signal.SIGCONT)
                os.killpg(hook.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            hook.wait()
    ws.close()
    x.close()
