#!/usr/bin/env python3
"""Assert native TODO/planning states through WebKit IPC and capture its X11 window.

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
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>b.offsetParent!==null && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)})).click()'
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


def capture(name, note, expects, expression=None, allow_error=False):
    wait('document.fonts.status==="loaded"')
    for text in expects:
        wait(f"document.body.innerText.includes({json.dumps(text)})")
    if expression:
        wait(expression)
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    if not allow_error:
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


def menu(title, action):
    evaluate(f'document.querySelector({json.dumps("button[aria-label="+json.dumps("Actions for "+title)+"]")}).click()')
    click(action)


def feature_form(branch, planning='Start directly', choice='node'):
    click('New feature')
    wait('!!document.querySelector("[role=dialog][aria-label=\\"New feature\\"]")')
    field('Branch / feature name', branch)
    field('Harness', 'codex')
    field('Mode', 'vibe')
    # Direct creation may default to the ordinary checkout. Capture uses a
    # private worktree so its hook and retained files have explicit ownership.
    if not evaluate('document.querySelector("input.switch").checked'):
        evaluate('document.querySelector("input.switch").click()')
    wait('!!document.querySelector("select[aria-label=\\"Choose fixture stack\\"]")')
    select_text('select[aria-label="Choose fixture stack"]', choice)
    evaluate(f'Array.from(document.querySelectorAll("[role=radio]")).find(e=>e.textContent.trim()==={json.dumps(planning)}).click()')


def cancel_interview(branch):
    click('Cancel interview…')
    click('Cancel interview')
    wait('!document.querySelector("[role=dialog][aria-label=\\"Plan interview\\"]")')
    assert not rows('SELECT id FROM features WHERE branch=?', (branch,))
    assert rows("SELECT id FROM todos WHERE id='plan' AND (status!='not_started' OR agent_session_id IS NOT NULL)") == []
    assert not list(scratch.rglob('AMF_PLAN.md'))


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


def todo_form(title, branch, *, planning=False, choice='rust & tools'):
    project_todos()
    evaluate(f'Array.from(document.querySelectorAll(".todo")).find(e=>e.querySelector(".todo-title").textContent==={json.dumps(title)}).querySelector(".todo-actions button").click()')
    wait('!!document.querySelector(`[role=dialog][aria-label="Start TODO"]`)')
    field('Where', '__new__')
    if planning:
        click('Plan first')
    evaluate('document.querySelector(`[role=dialog][aria-label="Start TODO"] button.btn-primary`).click()')
    label = 'Plan TODO in a new feature' if planning else 'Start TODO in a new feature'
    wait(f'!!document.querySelector(`[role=dialog][aria-label="{label}"]`)')
    field('Branch', branch)
    field('Harness', 'codex')
    field('Mode', 'vibe')
    wait('!!document.querySelector(`select[aria-label="Choose fixture stack"]`)')
    select_text('select[aria-label="Choose fixture stack"]', choice)


def no_launch(branch):
    assert not rows('SELECT id FROM features WHERE branch=?', (branch,))
    assert rows("SELECT status,agent_session_id FROM todos WHERE id='direct'") == [('not_started', None)]
    assert not worktree_for(branch)
    assert hooks.read_text().splitlines() == ['node']
    assert len(call_records('interactive')) == 1


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    wait('document.body.innerText.includes("Invoice API")')
    click('Invoice API')
    wait('!!document.querySelector(".todo-panel[aria-label=\\"Project TODOs\\"]")')
    # Seed the association after this process has observed the externally
    # inserted session. Startup reconciliation may precede the first refresh.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE todos SET agent_session_id='host-codex' WHERE id='move'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    menu('Plan quota validation', 'Move up')
    wait_db('SELECT id FROM todos WHERE list_id=\'project-list\' ORDER BY sort_order', [('move',), ('plan',), ('direct',)])
    menu('Keep linked quota work', 'Move or copy…')
    wait('!!document.querySelector("[role=dialog][aria-label=\\"Move or copy TODO\\"]")')
    select_text('select[aria-label="Destination for Keep linked quota work"]', 'Global')
    capture('001-scoped-move.png',
            'The native project TODO list has been reordered. Moving its in-progress item to Global explicitly retains its status and agent link.',
            ['Move or copy', 'Moving keeps its status and agent link. A copy starts fresh.', 'Global', 'Plan quota validation', 'Implement invoice guard'])
    click('Move')
    wait_db('SELECT l.scope,t.status,t.agent_session_id FROM todos t JOIN todo_lists l ON l.id=t.list_id WHERE t.id=\'move\'', [('global', 'in_progress', 'host-codex')])
    click('Global TODOs')
    wait('document.querySelector(".todo-panel")?.innerText.includes("Keep linked quota work")')
    menu('Keep linked quota work', 'Move or copy…')
    select_text('select[aria-label="Destination for Keep linked quota work"]', 'Feature: Invoice API / host')
    click('Copy')
    wait_db("SELECT l.scope,t.status,t.agent_session_id FROM todos t JOIN todo_lists l ON l.id=t.list_id WHERE t.title='Keep linked quota work' ORDER BY l.scope", [('global', 'in_progress', 'host-codex'), ('worktree', 'not_started', None)])
    click('host')
    click('TODOs')
    wait('document.querySelector(`.todo-panel[aria-label="Worktree TODOs"]`)?.innerText.includes("Keep linked quota work")')
    capture('002-copy-reset.png',
            'Copying into the host worktree creates a separate not-started TODO with no agent link. The original remains in Global, in progress and linked; SQLite assertions verify both rows.',
            ['Worktree TODOs', 'Keep linked quota work', 'Start'],
            'document.querySelector(".todo-panel").querySelectorAll(".todo-not_started").length===1')
    click('Invoice API')
    feature_form('form-cancel', choice='rust & tools')
    capture('003-hook-form-cancel.png',
            'Ordinary feature creation shows the project worktree hook choices. Cancelling this form runs no hook and creates no checkout or feature.',
            ['New feature', 'Choose fixture stack', 'Create feature', 'Cancel'],
            'document.querySelector(`select[aria-label="Choose fixture stack"]`).value==="rust & tools"')
    click('Cancel')
    assert hooks.read_text() == '' and rows('SELECT count(*) FROM features') == [(1,)]
    assert not list(scratch.rglob('*form-cancel*'))
    feature_form('ordinary-direct')
    click('Create feature')
    wait_db("SELECT name FROM features WHERE branch='ordinary-direct'", [('ordinary-direct',)])
    wait('document.body.innerText.includes("Offline planning fixture ready")')
    assert hooks.read_text().splitlines() == ['node']
    ordinary = pathlib.Path(rows("SELECT workdir FROM features WHERE branch='ordinary-direct'")[0][0])
    assert (ordinary/'hook-choice').read_text() == 'node'
    wait('!!document.querySelector(`button[aria-label="Hide agent sidebar"]`)')
    evaluate('document.querySelector(`button[aria-label="Hide agent sidebar"]`).click()')
    wait('document.body.innerText.includes("Offline planning fixture ready")')
    capture('004-direct-created.png',
            'The ordinary feature runs its selected setup hook and starts only the private offline terminal fixture.',
            ['ordinary-direct', 'Offline planning fixture ready', 'No prompt has been sent.'])
    todo_form('Implement invoice guard', 'todo-form-cancel')
    capture('005-todo-hook-form.png',
            'TODO creation presents the same project hook choices and explains that its prompt will open as an editable draft. Cancelling here runs no setup.',
            ['Start in a new feature', 'Implement invoice guard', 'Choose fixture stack', 'editable draft', 'Create and start'],
            'document.querySelector(`select[aria-label="Choose fixture stack"]`).value==="rust & tools"')
    click('Cancel')
    no_launch('todo-form-cancel')
    todo_form('Implement invoice guard', 'todo-direct')
    click('Create and start')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve new TODO feature"]`)')
    no_launch('todo-direct')
    capture('006-resource-cancel.png',
            'The running-agent limit asks for approval before creating a checkout, running a hook or reserving the TODO. Cancelling this warning leaves every one of those unchanged.',
            ['Start another agent?', 'Start anyway', 'Cancel'])
    evaluate('document.querySelector(`[role=dialog][aria-label="Approve new TODO feature"] button.btn-ghost`).click()')
    no_launch('todo-direct')
    click('Cancel')
    todo_form('Implement invoice guard', 'todo-direct')
    click('Create and start')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve new TODO feature"]`)')
    click('Start anyway')
    wait_calls('interactive', 2)
    wait('document.querySelector(`textarea[aria-label="Draft prompt"]`)?.value.includes("Implement invoice guard")')
    todo_worktree = pathlib.Path(rows("SELECT workdir FROM features WHERE branch='todo-direct'")[0][0])
    assert (todo_worktree/'hook-choice').read_text() == 'rust & tools'
    assert json.loads((todo_worktree/'.fixture-received.json').read_text()) == []
    assert rows("SELECT status,agent_session_id IS NOT NULL FROM todos WHERE id='direct'") == [('in_progress', 1)]
    capture('007-unsent-handoff.png',
            'After explicit approval, the selected hook runs and the TODO is linked to its new private session. Its prompt is only an editable composer draft: the offline receiver has received no message.',
            ['todo-direct', 'Compose prompt', 'Draft here, then send to the agent.', 'Send prompt'],
            'document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Implement invoice guard")')

    project_todos()
    feature_form('full-cancel', planning='Full Plan', choice='rust & tools')
    click('Create and plan')
    wait('!!document.querySelector(`[role=dialog][aria-label="Plan interview"] textarea[aria-label="Plan answer"]`)')
    full_worktree = worktree_for('full-cancel')
    assert full_worktree and (full_worktree/'hook-choice').read_text() == 'rust & tools'
    assert not rows("SELECT id FROM features WHERE branch='full-cancel'")
    input_value('textarea[aria-label="Plan answer"]', 'Preserve quota checks before accepting invoice work.')
    capture('008-full-plan-brief.png',
            'Full planning runs the chosen worktree setup before opening its brief. The checkout already exists, but no feature or agent has been created and no plan has been accepted.',
            ['Plan interview', 'full-cancel', 'Describe the change you want to plan.', 'Draft plan now', 'Cancel interview…'],
            'document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Preserve quota checks")')
    click('Draft plan now')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve planning agent call"]`)')
    click('View prompt')
    capture('009-precall-cancel.png',
            'The synthesis call requires an explicit reviewable approval. Cancelling the call invokes no offline synthesis and returns to the interview with its entered brief intact.',
            ['This step calls Codex.', 'Cancel call', 'Continue', 'Hide prompt'])
    click('Cancel call')
    wait('!!document.querySelector(`textarea[aria-label="Plan answer"]`)')
    assert len(call_records('synthesis')) == 0
    assert evaluate('document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Preserve quota checks")')
    capture('009b-return-to-brief.png',
            'Cancelling the synthesis call restores the editable Brief with its entered text intact. No synthesis has run, and an explicit retry remains available.',
            ['Describe the change you want to plan.', 'Draft plan now', 'Cancel interview…'],
            'document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Preserve quota checks")')
    click('Draft plan now')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve planning agent call"]`)')
    click('Continue')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Accept plan" && !b.disabled)')
    wait_calls('synthesis', 1)
    assert not list(scratch.rglob('AMF_PLAN.md'))
    capture('010-offline-review.png',
            'The native interview displays the plan returned by the private offline synthesis fixture. Review has not written a plan file or created a feature or agent.',
            ['Validate invoice quotas', 'Goal', 'Decisions', 'Architecture', 'Accept plan'])
    click('Edit markdown')
    wait('!!document.querySelector(`textarea[aria-label="Plan answer"]`)')
    edited = evaluate('document.querySelector(`textarea[aria-label="Plan answer"]`).value') + '\nUnsubmitted local reviewer note.\n'
    input_value('textarea[aria-label="Plan answer"]', edited)
    click('Minimize')
    wait('!!document.querySelector(".plan-pill")')
    evaluate('document.querySelector(".plan-pill").click()')
    wait('document.querySelector(`textarea[aria-label="Plan answer"]`)?.offsetParent!==null')
    assert evaluate('document.querySelector(`textarea[aria-label="Plan answer"]`).value') == edited
    click('Discard edit')
    wait('document.body.innerText.includes("Accept plan")')
    cancel_interview('full-cancel')
    assert full_worktree.is_dir() and (full_worktree/'hook-choice').read_text() == 'rust & tools'
    assert len(call_records('interactive')) == 2
    capture('011-full-cancelled.png',
            'Cancelling the reviewed full interview returns to the project without saving a plan, creating a feature or starting another agent. Its already-created setup checkout remains available.',
            ['Invoice API', 'Project TODOs', 'Plan quota validation'],
            '!document.querySelector(`[role=dialog][aria-label="Plan interview"]`)')

    evaluate('document.querySelectorAll(`.toasts button[aria-label="Dismiss"]`).forEach(b=>b.click())')
    wait('!document.querySelector(".toast")')
    feature_form('quick-cancel', planning='Quick Plan', choice='node')
    click('Create and plan')
    wait('!!document.querySelector(`[role=dialog][aria-label="Plan interview"] textarea[aria-label="Plan answer"]`)')
    quick_worktree = worktree_for('quick-cancel')
    assert quick_worktree and (quick_worktree/'hook-choice').read_text() == 'node'
    capture('012-quick-plan-cancel.png',
            'Quick Plan also runs the chosen hook and opens its brief before any feature or agent is saved. Cancelling leaves its prepared checkout and starts no harness.',
            ['Quick Plan', 'quick-cancel', 'Describe the change you want to plan.', 'Cancel interview…'])
    cancel_interview('quick-cancel')
    assert quick_worktree.is_dir() and len(call_records('interactive')) == 2
    evaluate('document.querySelectorAll(`.toasts button[aria-label="Dismiss"]`).forEach(b=>b.click())')
    wait('!document.querySelector(".toast")')
    todo_form('Plan quota validation', 'todo-plan-cancel', planning=True, choice='node')
    capture('013-todo-plan-hook.png',
            'Planning a TODO into a new feature offers the project setup choice and explains that setup runs before the interview, while the agent waits for acceptance.',
            ['Plan in a new feature', 'Plan quota validation', 'Choose fixture stack', 'before the interview', 'Start plan'],
            'document.querySelector(`select[aria-label="Choose fixture stack"]`).value==="node"')
    click('Start plan')
    wait('!!document.querySelector(`[role=dialog][aria-label="Plan interview"] textarea[aria-label="Plan answer"]`)')
    planned_worktree = worktree_for('todo-plan-cancel')
    assert planned_worktree and (planned_worktree/'hook-choice').read_text() == 'node'
    assert not rows("SELECT id FROM features WHERE branch='todo-plan-cancel'")
    assert rows("SELECT status,agent_session_id FROM todos WHERE id='plan'") == [('not_started', None)]
    assert evaluate('document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("Plan quota validation")')
    click('Continue')
    wait('document.body.innerText.includes("Which quota rule should be preserved?")')
    input_value('textarea[aria-label="Plan answer"]', 'Keep the existing per-account quota and reject excess work.')
    capture('014-todo-plan-answer.png',
            'The TODO seeds the native interview brief, then the project question accepts a scoped quota answer. The TODO is still not started and unlinked throughout planning.',
            ['Which quota rule should be preserved?', 'Question 1 of 1', 'Continue', 'todo-plan-cancel'],
            'document.querySelector(`textarea[aria-label="Plan answer"]`).value.includes("per-account quota")')
    click('Continue')
    wait('document.body.innerText.includes("Synthesize now")')
    evaluate('Array.from(document.querySelectorAll("button.choice-card")).find(b=>b.querySelector("strong").textContent==="Synthesize now").click()')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve planning agent call"]`)')
    click('Continue')
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Accept plan" && !b.disabled)')
    wait_calls('synthesis', 2)
    click('Accept plan')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve planned agent"]`)')
    assert not list(scratch.rglob('AMF_PLAN.md'))
    assert not rows("SELECT id FROM features WHERE branch='todo-plan-cancel'")
    assert rows("SELECT status,agent_session_id FROM todos WHERE id='plan'") == [('not_started', None)]
    assert len(call_records('interactive')) == 2
    capture('015-plan-resource-warning.png',
            'Accepting the reviewed TODO plan asks for resource approval before saving a plan, creating its feature, claiming the TODO or starting another agent. Keeping review applies none of those effects.',
            ['Start the planned agent?', 'Accept plan and start agent', 'Keep reviewing'])
    click('Keep reviewing')
    cancel_interview('todo-plan-cancel')
    assert planned_worktree.is_dir() and (planned_worktree/'hook-choice').read_text() == 'node'
    assert len(call_records('interactive')) == 2
    assert rows("SELECT branch FROM features ORDER BY branch") == [('host',), ('ordinary-direct',), ('todo-direct',)]
    assert rows("SELECT status,agent_session_id FROM todos WHERE id='plan'") == [('not_started', None)]
    assert hooks.read_text().splitlines() == ['node', 'rust & tools', 'rust & tools', 'node', 'node']
    assert json.loads((todo_worktree/'.fixture-received.json').read_text()) == []
    capture('016-todo-plan-cancelled.png',
            'The cancelled planned TODO is still available to start. No full, quick or TODO plan was accepted; none created a feature or agent, and their prepared worktrees are retained.',
            ['Project TODOs', 'Plan quota validation', 'Start'],
            '!document.querySelector(`[role=dialog][aria-label="Plan interview"]`)')
    (out / 'validation.json').write_text(json.dumps({'native_ipc': True, 'offline_interactive_calls': len(call_records('interactive')), 'offline_synthesis_calls': len(call_records('synthesis')), 'hooks': hooks.read_text().splitlines(), 'no_saved_plans': not list(scratch.rglob('AMF_PLAN.md')), 'retained_cancelled_worktrees': [path.name for path in [full_worktree,quick_worktree,planned_worktree]], 'todo_draft_unsent': json.loads((todo_worktree/'.fixture-received.json').read_text()) == []}, indent=2)+'\n')

finally:
    (out / 'capture-notes.jsonl').write_text(''.join(json.dumps(note) + '\n' for note in notes))
    (out / 'last-state.txt').write_text(evaluate('document.body.innerText'))
    ws.close()
    x.close()
