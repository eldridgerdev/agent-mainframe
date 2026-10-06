#!/usr/bin/env python3
"""Assert native prompt-override manager states through WebKit IPC and capture its X11 window.

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
    (out / "failed-state.txt").write_text(evaluate("document.body.innerText"))
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
    wait('!document.body.innerText.includes("Updating review…") && !document.body.innerText.includes("Loading prompts…")')


def choose_file(path):
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    ready()


def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def choose(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()""")


def click_label(label):
    evaluate(f'Array.from(document.querySelectorAll("button")).find(b=>b.getAttribute("aria-label")==={json.dumps(label)}).click()')


def open_prompt(title):
    evaluate(f'Array.from(document.querySelectorAll(".library-item")).find(b=>b.querySelector("strong").textContent==={json.dumps(title)}).click()')
    wait(f'document.querySelector(".library-item[aria-pressed=true] strong")?.textContent==={json.dumps(title)}')


def textarea_value():
    return evaluate('document.querySelector(".overrides-textarea")?.value ?? null')


repo = pathlib.Path(sys.argv[4])
calls_path = pathlib.Path(os.environ["AMF_GUI_AI_CALLS"])
db_path = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "amf/amf.db"
amf_json = repo / "amf.json"


def calls():
    return [json.loads(line) for line in calls_path.read_text().splitlines()]


def overrides():
    with sqlite3.connect(db_path) as db:
        return sorted(db.execute("SELECT prompt_id, scope, scope_key, harness, template FROM prompt_overrides").fetchall(), key=repr)


def external_write(prompt_id, scope, key, harness, template):
    """Stand-in for the TUI or another AMF window writing the shared database."""
    with sqlite3.connect(db_path) as db:
        db.execute(
            "INSERT INTO prompt_overrides(prompt_id,scope,scope_key,harness,template,created_at,updated_at) VALUES(?,?,?,?,?,datetime('now'),datetime('now'))",
            (prompt_id, scope, key, harness, template),
        )


# The project's committed config carries one shared override; the user's
# machine-wide (global) layer carries a Codex-only one. Keep amf.json out of
# the reviewed diff.
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\namf.json\n")
project_config = {
    "allowed_agents": ["claude", "codex"],
    "prompt_overrides": {"session.summary": {"template": "PROJECT: one line about {{recent_lines}} ({{max_chars}} chars max)"}},
}
amf_json.write_text(json.dumps(project_config, indent=2) + "\n")
external_write("session.summary", "global", None, "codex", "GLOBAL Codex summary of {{recent_lines}}")
draft = "Summarize this {{harness_name}} session for the team.\nLead with failures.\nRecent output:\n{{recent_lines}}"

try:
    window.configure(x=0, y=0, width=1450, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click("Round invoice totals")
    wait('document.body.innerText.includes("Final Review")')
    click("Prompt overrides")
    wait('document.querySelectorAll(".library-item").length===25')
    ready()
    open_prompt("Session summary")
    capture("001-overrides-list-and-layers.png", "The manager lists every registry prompt with the layer in effect; Session summary shows its placeholders, effective project template and the stored global Codex override.", ["Prompt overrides", "Session summary", "Project (amf.json) · all harnesses", "Global · Codex", "{{recent_lines}}", "PROJECT: one line about", "Context: Invoice API / Round invoice totals"], 'document.querySelectorAll(".library-item").length===25')

    click("New override…")
    wait('!!document.querySelector(".overrides-textarea")')
    choose("Harness", "")
    fill("Template", draft)
    capture("002-editing-feature-override.png", "New override opens the effective template in a local editor. Scope and harness pickers offer only scopes this context allows.", ["Save to scope", "Harness", "Placeholders are not validated", "Save override"], f'document.querySelector(".overrides-textarea").value==={json.dumps(draft)} && document.querySelectorAll(".overrides-editor select")[0].value==="feature" && document.querySelectorAll(".overrides-editor select")[1].value===""')

    evaluate("document.body.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))")
    wait('!!document.querySelector("[aria-label=\'Discard unsaved template\']")')
    capture("003-unsaved-edit-protection.png", "Escape with unsaved template text asks before discarding; the draft is untouched.", ["Discard your unsaved template changes and close the manager?", "Keep editing", "Discard changes"], f'document.querySelector(".overrides-textarea").value==={json.dumps(draft)}')
    click("Keep editing")

    external_write("session.summary", "feature", str(repo), None, "TUI feature summary {{recent_lines}}")
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Reload current version")')
    capture("004-external-change-requires-reload.png", "Another AMF process saved an override for this prompt. Saving is blocked until the user reloads; the draft text stays.", ["changed outside this window", "Reload current version", "your text is kept"], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save override").disabled', allow_alert=True)
    assert textarea_value() == draft

    click("Reload current version")
    wait('!Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Reload current version")')
    wait('document.body.innerText.includes("This replaces the existing Feature · all harnesses override")')
    click("Save override")
    wait('document.body.innerText.includes("Saved the Feature · all harnesses override")')
    ready()
    capture("005-saved-feature-override.png", "After reloading, the save replaces the externally written feature override and becomes the effective template; project and global layers remain stored.", ["Saved the Feature · all harnesses override for session.summary", "Feature · all harnesses", "in effect", "Lead with failures.", "Project (amf.json) · all harnesses", "Global · Codex"])
    assert ("session.summary", "feature", str(repo), None, draft) in overrides(), overrides()

    click_label("Clear Global · Codex")
    wait('!!document.querySelector("[aria-label=\'Confirm clear override\']")')
    capture("006-clear-requires-confirmation.png", "Clearing names exactly one stored override and waits for explicit confirmation.", ["Clear the Global · Codex override for Session summary?", "Keep it", "Clear override"])
    assert any(row[1] == "global" for row in overrides())
    click("Clear override")
    wait('document.body.innerText.includes("Cleared the Global · Codex override")')
    ready()
    capture("007-override-cleared.png", "Only the confirmed global Codex row is removed; the feature and project overrides stay in place.", ["Cleared the Global · Codex override for session.summary", "Feature · all harnesses", "Project (amf.json) · all harnesses"], '!Array.from(document.querySelectorAll("button")).some(b=>b.getAttribute("aria-label")==="Clear Global · Codex")')
    assert not any(row[1] == "global" for row in overrides()), overrides()

    amf_json.write_text("{ \"prompt_overrides\": ")
    wait('document.body.innerText.includes("Project overrides are ignored.")')
    capture("008-broken-amf-json-reported.png", "A malformed amf.json is reported, its project overrides are shown as not applying, and project saves are refused rather than overwriting the file.", ["Project overrides are ignored.", "is not valid JSON", "fix it first"], 'document.querySelector(".library-item[aria-pressed=true] .badge").textContent.includes("Feature")', allow_alert=True)
    assert amf_json.read_text() == "{ \"prompt_overrides\": "
    amf_json.write_text(json.dumps(project_config, indent=2) + "\n")
    wait('!document.body.innerText.includes("Project overrides are ignored.")')
    click("Done")
    wait('!document.querySelector("[aria-label=\'Prompt overrides\']")')

    # The pending pre-call notice links to the same manager, focused on its prompt.
    click("Final Review")
    wait('!!document.querySelector(".diff-file")')
    choose_file("invoice.test.ts")
    click("Generate walkthrough")
    wait('document.body.innerText.includes("Continue AI call")')
    click("Edit prompt")
    wait('document.querySelector(".library-item[aria-pressed=true] strong")?.textContent==="Final Review: file walkthrough"')
    ready()
    capture("009-precall-edit-prompt.png", "Edit prompt on a pending Final Review AI call opens the manager on that prompt for the review's feature and harness; nothing has run.", ["Opened from a pending AI call", "Final Review: file walkthrough", "{{file_path}}", "{{patch}}", "Built-in"])
    assert calls() == []
    click("New override…")
    wait('!!document.querySelector(".overrides-textarea")')
    original = textarea_value()
    fill("Template", original.replace("You are helping a reviewer understand", "DEMO OVERRIDE: lead with risks.\nYou are helping a reviewer understand", 1))
    click("Save override")
    wait('document.body.innerText.includes("Saved the Feature · all harnesses override for review.walkthrough")')
    click("Done")
    wait('!document.querySelector("[aria-label=\'Prompt overrides\']")')
    assert evaluate('document.body.innerText.includes("Continue AI call")')
    click("Cancel AI call")
    wait('!document.body.innerText.includes("Continue AI call")')
    click("Generate walkthrough")
    wait('document.body.innerText.includes("Continue AI call")')
    click("View prompt")
    wait('document.body.innerText.includes("DEMO OVERRIDE: lead with risks.")')
    capture("010-precall-uses-saved-override.png", "The next walkthrough request resolves the saved feature override; the prompt preview shows it and still no AI call has been made.", ["Headless AI call: Final Review: file walkthrough", "DEMO OVERRIDE: lead with risks.", "File: invoice.test.ts", "Continue AI call", "Edit prompt"])
    click("Cancel AI call")
    wait('!document.body.innerText.includes("Continue AI call")')
    assert calls() == [], "The override proof must not launch a harness"
    (out / "capture-notes.jsonl").write_text("".join(json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]}) + "\n" for n in notes))
finally:
    ws.close()
    x.close()
