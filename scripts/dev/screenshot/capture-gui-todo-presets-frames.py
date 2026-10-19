#!/usr/bin/env python3
"""Assert native TODO preset states through WebKit IPC and capture its X11 window.

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
    raise AssertionError(f"{expression}: {evaluate('document.body.innerText')}")


def click(text):
    button = f'Array.from(document.querySelectorAll("button")).find(b=>b.offsetParent!==null && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)}))'
    wait(f'!!({button}) && !({button}).disabled')
    evaluate(f'({button}).click()')


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
    assert not evaluate('!!document.querySelector(".toast-error")'), evaluate('document.body.innerText')
    evaluate('document.querySelectorAll(`.toast-info button[aria-label="Dismiss"]`).forEach(button=>button.click())')
    wait('!document.querySelector(".toast-info")')
    for text in expects:
        wait(f"document.body.innerText.includes({json.dumps(text)})")
    if expression:
        wait(expression)
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_alert:
        assert not evaluate('Array.from(document.querySelectorAll("[role=alert]")).some(e=>e.offsetParent!==null)'), body
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


dbpath = pathlib.Path(sys.argv[4])
repo = pathlib.Path(sys.argv[5])
scratch = pathlib.Path(sys.argv[6])
hooks = scratch / 'hooks.txt'
calls = scratch / 'calls.jsonl'


def field(label, value, root='document'):
    evaluate(f'''(()=>{{const f=Array.from({root}.querySelectorAll('label')).find(l=>l.querySelector('.field-label')?.textContent.trim()==={json.dumps(label)} || l.firstElementChild?.textContent.trim()==={json.dumps(label)}); if(!f)throw Error('Missing field '+{json.dumps(label)});const e=f.querySelector('input,select,textarea');const p=e.tagName==='SELECT'?HTMLSelectElement:e.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event(e.tagName==='SELECT'?'change':'input',{{bubbles:true}}));}})()''')


def input_value(selector, value):
    evaluate(f'''(()=>{{const e=document.querySelector({json.dumps(selector)});const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()''')


def select_text(selector, text):
    evaluate(f'''(()=>{{const e=document.querySelector({json.dumps(selector)});const o=Array.from(e.options).find(o=>o.text.trim()==={json.dumps(text)});if(!o)throw Error('Missing option '+{json.dumps(text)});e.value=o.value;e.dispatchEvent(new Event('change',{{bubbles:true}}));}})()''')


def rows(sql, values=()):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql, values).fetchall()


def wait_db(sql, expected, values=()):
    for _ in range(100):
        if rows(sql, values) == expected:
            return
        time.sleep(0.1)
    raise AssertionError((sql, rows(sql, values), expected))


def call_records(kind=None):
    records = [json.loads(line) for line in calls.read_text().splitlines()]
    return [row for row in records if kind is None or row['kind'] == kind]


def wait_calls(kind, count):
    for _ in range(100):
        if len(call_records(kind)) == count:
            return
        time.sleep(0.1)
    raise AssertionError(call_records())


def worktree_for(branch):
    output = subprocess.check_output(['git', 'worktree', 'list', '--porcelain'], cwd=repo, text=True)
    for block in output.split('\n\n'):
        if 'branch refs/heads/' + branch in block.splitlines():
            return pathlib.Path(block.splitlines()[0].removeprefix('worktree '))
    return None


def project_todos():
    click('Invoice API')
    wait('!!document.querySelector(`.todo-panel[aria-label="Project TODOs"]`)')


def open_todo(title, planning=False):
    project_todos()
    evaluate(f'Array.from(document.querySelectorAll(".todo")).find(e=>e.querySelector(".todo-title").textContent==={json.dumps(title)}).querySelector(".todo-actions button").click()')
    wait('!!document.querySelector(`[role=dialog][aria-label="Start TODO"]`)')
    field('Where', '__new__')
    if planning:
        click('Plan first')
    evaluate('document.querySelector(`[role=dialog][aria-label="Start TODO"] button.btn-primary`).click()')
    wait('!!document.querySelector(`[aria-label="Feature preset"] option[value="0"]`)')
    wait('!!document.querySelector(`select[aria-label="Choose fixture stack"]`)')
    select_text('select[aria-label="Choose fixture stack"]', 'rust & tools')


def choose(name):
    select_text('select[aria-label="Feature preset"]', name)


def no_reservation():
    assert rows("SELECT status,agent_session_id,linked_feature_id FROM todos WHERE id IN ('plan','direct') ORDER BY id") == [('not_started', None, None), ('not_started', None, None)]
    assert rows('SELECT COUNT(*) FROM features') == [(1,)]
    assert call_records() == []


def cancel_plan(branch):
    click('Cancel interview…'); click('Cancel interview')
    wait('!document.querySelector(`[role=dialog][aria-label="Plan interview"]`)')
    no_reservation()
    checkout = worktree_for(branch)
    assert checkout and (checkout/'hook-choice').read_text() == 'rust & tools'
    assert not (checkout/'AMF_PLAN.md').exists()
    return checkout


try:
    window.configure(x=0, y=0, width=1280, height=980); x.sync()
    wait('document.body.innerText.includes("Invoice API")')
    original_config = (scratch/'config/amf/config.json').read_bytes()
    original_project = (repo/'amf.json').read_bytes()
    tmux = ['tmux', '-S', str(scratch/'tmux.sock')]
    host_identity = subprocess.check_output(tmux+['display-message','-p','-t','amf-fixture-host:opencode','#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip()
    assert re.fullmatch(r'\$[0-9]+:%[0-9]+:[1-9][0-9]*:[1-9][0-9]*',host_identity)
    open_todo('Plan quota validation')
    names=evaluate('Array.from(document.querySelector(`[aria-label="Feature preset"]`).options).map(e=>e.textContent)')
    assert set(names) == {'Custom settings','Shared','Remote','Full'} and names.count('Shared') == 1, names
    choose('Shared')
    assert evaluate('Array.from(document.querySelectorAll("label")).find(e=>e.querySelector(".field-label")?.textContent.trim()==="Harness").querySelector("select").value') == 'opencode'
    assert evaluate('Array.from(document.querySelectorAll("label")).find(e=>e.querySelector(".field-label")?.textContent.trim()==="Mode").querySelector("select").value') == 'supervibe'
    assert evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).filter(e=>["Review","Chrome"].some(label=>e.closest("label").textContent.trim().startsWith(label))).every(e=>e.checked)')
    capture('001-project-preset.png', 'TODO creation resolves the project Shared template over its global namesake, filters disallowed Pi, and fills OpenCode, SuperVibe, review, Chrome and Quick Plan without launching anything.',
            ['Plan in a new feature','Plan quota validation','Quick Plan','Review','Chrome'],
            'document.querySelector(`select[aria-label="Feature preset"]`).selectedOptions[0].textContent==="Shared" && Array.from(document.querySelectorAll("input")).some(e=>e.value==="project/plan-quota-validation")')
    no_reservation()
    choose('Remote'); field('Branch','custom/preserve-quota'); choose('Custom settings')
    capture('002-remote-block.png', 'Remote Control intent survives manual branch edits and switching to Custom settings. Both click and form submission remain blocked until an explicit opt-out.',
            ['does not support Remote Control yet.','Remote Control','Quick Plan'],
            'Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Start Quick Plan").disabled', allow_alert=True)
    evaluate('document.querySelector(`[role=dialog] form`).requestSubmit()'); no_reservation()
    click('Cancel')
    capture('003-unsaved-guard.png', 'Cancelling the edited TODO form asks before discarding its launch settings. Keep editing retains the branch and Remote Control intent; the TODO is still untouched.',
            ['Discard unsaved TODO feature settings?','Keep editing','Discard changes'])
    click('Keep editing')
    evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).find(e=>e.closest("label").textContent.trim().startsWith("Remote Control")).click()')
    wait('!document.querySelector(`[role=alert]`)')
    capture('004-explicit-opt-out.png', 'The user explicitly clears Remote Control. The edited branch remains, and Quick Plan can now start; no feature, hook or TODO reservation has happened yet.',
            ['Plan in a new feature','Quick Plan','Choose fixture stack','Start Quick Plan'],
            '!Array.from(document.querySelectorAll("button")).find(e=>e.textContent.trim()==="Start Quick Plan").disabled && Array.from(document.querySelectorAll("input")).some(e=>e.value==="custom/preserve-quota")')
    no_reservation()
    click('Start Quick Plan')
    wait('!!document.querySelector(`[role=dialog][aria-label="Plan interview"] textarea`)')
    capture('005-quick-interview.png', 'Quick Plan follows the TODO preset route through the real Rust engine. Setup has run, but the TODO is not reserved, no persisted feature or TODO association exists and no agent has been launched.',
            ['Quick Plan','custom/preserve-quota','Plan quota validation','Cancel interview…'],
            'document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Plan quota validation")')
    no_reservation(); quick_worktree=cancel_plan('custom/preserve-quota')
    project_todos()
    capture('006-cancelled-quick.png', 'Cancelling Quick Plan leaves the TODO not started and available to launch. Its prepared worktree stays on disk; no persisted feature, association, saved plan or agent was created.',
            ['Project TODOs','Plan quota validation','Start'])
    open_todo('Plan quota validation', planning=True); choose('Full')
    full_branch=evaluate('Array.from(document.querySelectorAll("label")).find(e=>e.querySelector(".field-label")?.textContent.trim()==="Branch").querySelector("input").value')
    click('Start plan'); wait('!!document.querySelector(`[role=dialog][aria-label="Plan interview"] textarea`)')
    capture('007-full-interview.png', 'The Full template chooses the shared full interview independently of the entry button. TODO provenance is retained, and cancellation applies the same no-reservation rule as Quick Plan.',
            ['Plan interview',full_branch,'Plan quota validation','Cancel interview…'],
            'document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Plan quota validation")')
    full_worktree=cancel_plan(full_branch)
    open_todo('Implement invoice guard'); choose('Shared'); field('Branch','launch/edited-invoice')
    evaluate('Array.from(document.querySelectorAll(`[role=radio]`)).find(e=>e.textContent.trim()==="Start directly").click()')
    evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).find(e=>e.closest("label").textContent.trim().startsWith("Chrome")).click()')
    capture('008-edited-direct-launch.png', 'After applying a project preset, manual branch, planning and Chrome edits stay authoritative. Direct creation will use OpenCode, SuperVibe and review, with Chrome off.',
            ['Start in a new feature','Start directly','Create and start','Review','Chrome'],
            'Array.from(document.querySelectorAll("input")).some(e=>e.value==="launch/edited-invoice")')
    no_reservation(); click('Create and start')
    wait('document.body.innerText.includes("Offline planning fixture ready")')
    wait('document.querySelector(`textarea[aria-label="Draft prompt"]`)?.value.includes("Implement invoice guard")')
    created=rows("SELECT workdir,agent,mode,review,plan_mode,enable_chrome,id FROM features WHERE branch='launch/edited-invoice'")
    assert len(created)==1 and created[0][1:6]==('opencode','supervibe',1,0,0),created
    checkout=pathlib.Path(created[0][0]); wait_calls('interactive',1)
    new_session=rows("SELECT tmux_session FROM features WHERE id=?", (created[0][6],))[0][0]
    new_identity=subprocess.check_output(tmux+['display-message','-p','-t',new_session+':opencode','#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip()
    assert re.fullmatch(r'\$[0-9]+:%[0-9]+:[1-9][0-9]*:[1-9][0-9]*',new_identity),new_identity
    assert json.loads((checkout/'.fixture-received.json').read_text()) == []
    assert rows("SELECT status,agent_session_id IS NOT NULL,linked_feature_id FROM todos WHERE id='direct'") == [('in_progress',1,created[0][6])]
    assert rows("SELECT status,agent_session_id,linked_feature_id FROM todos WHERE id='plan'")==[('not_started',None,None)]
    capture('009-unsent-handoff.png', 'The direct TODO launch saves the edited settings and links exactly one new private agent session. Its kickoff remains an editable composer draft; the offline receiver has received no prompt.',
            ['launch/edited-invoice','Compose prompt','Offline planning fixture ready','Send prompt'],
            'document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Implement invoice guard")')
    assert subprocess.check_output(tmux+['display-message','-p','-t',new_session+':opencode','#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip()==new_identity
    assert (scratch/'config/amf/config.json').read_bytes()==original_config
    assert (repo/'amf.json').read_bytes()==original_project
    assert subprocess.check_output(tmux+['display-message','-p','-t','amf-fixture-host:opencode','#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip()==host_identity
    assert not call_records('synthesis') and not list(scratch.rglob('AMF_PLAN.md'))
    (out/'validation.json').write_text(json.dumps({'native_ipc':True,'effective_project_presets':names,'quick_and_full_cancelled_without_reservation':True,'retained_cancelled_worktrees':[str(quick_worktree),str(full_worktree)],'offline_interactive_calls':1,'offline_synthesis_calls':0,'todo_draft_unsent':True,'config_bytes_unchanged':True,'existing_host_identity_unchanged':True},indent=2)+'\n')
finally:
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps(note)+'\n' for note in notes))
    try:
        (out/'last-state.txt').write_text(evaluate('document.body.innerText'))
    except Exception as error:
        print(f'Could not record final state: {error!r}',file=sys.stderr)
    ws.close(); x.close()
