#!/usr/bin/env python3
"""Assert native Learning states through WebKit IPC and capture its X11 window.

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


def click_js(button, shift=False):
    wait(f'!!({button}) && !({button}).disabled')
    if shift:
        evaluate(f'({button}).dispatchEvent(new MouseEvent("click",{{bubbles:true,shiftKey:true}}))')
    else:
        evaluate(f'({button}).click()')


def click(text):
    click_js(f'Array.from(document.querySelectorAll("button")).find(b=>b.offsetParent!==null && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)}))')


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
calls = scratch / 'calls.jsonl'


def field(label, value, root='document'):
    wait('!!document.querySelector(".learning-controls select") && !document.querySelector(".learning-controls select").disabled')
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


def call_records(kind):
    return [row for row in map(json.loads, calls.read_text().splitlines()) if row['kind'] == kind]


def scroll(selector):
    evaluate(f'document.querySelector({json.dumps(selector)}).scrollIntoView({{block:"center"}})')
    time.sleep(0.15)


def close_reader():
    click_js('document.querySelector(`[role=dialog][aria-label="Learning"] button[aria-label="Close"]`)')
    wait('!document.querySelector(`[role=dialog][aria-label="Learning"]`)')


def open_reader():
    click('Learning')
    wait('!!document.querySelector(`[role=dialog][aria-label="Learning"]`)')
    field('Answering harness', 'codex')
    wait('document.querySelector(".learning-question button.btn-primary").textContent.trim()==="Ask codex"')


try:
    window.configure(x=0, y=0, width=1180, height=900)
    x.sync()
    wait('Array.from(document.querySelectorAll(".nav-label")).some(e=>e.textContent.trim()==="Invoice API")')
    click('Invoice API')
    wait('Array.from(document.querySelectorAll("button")).some(e=>e.textContent.trim()==="host")')
    click('host')
    wait('Array.from(document.querySelectorAll("button")).some(e=>e.textContent.trim()==="Learning")')
    if evaluate('!!document.querySelector(`button[aria-label="Hide agent sidebar"]`)'):
        evaluate('document.querySelector(`button[aria-label="Hide agent sidebar"]`).click()')
        wait('!document.querySelector(`button[aria-label="Hide agent sidebar"]`)')
    open_reader()
    click_js('Array.from(document.querySelectorAll(".learning-files button")).find(e=>e.textContent.trim()==="quota.py")')
    wait('document.querySelector(`[aria-label="Code reader"]`).innerText.includes("within_quota")')
    field('Reading level', 'familiar')
    click_js('document.querySelector(`button[aria-label="Select line 2"]`)')
    wait('document.querySelector(".learning-line-selected")!==null')
    click_js('document.querySelector(`button[aria-label="Select line 3"]`)', shift=True)
    wait('document.querySelectorAll(".learning-line-selected").length===2')
    capture('001-code-reader.png',
            'Learning reads the private worktree through real IPC and highlights a selected line range. The answering harness and reading level are explicit; no question has been sent.',
            ['Learning · host', 'within_quota', 'Ask codex', 'Next question:'],
            'document.querySelectorAll(".learning-line-selected").length===2')
    assert not rows('SELECT id FROM learning_qa') and not call_records('answer')
    click('Show branch changes')
    wait('!!document.querySelector(`button[aria-label="Select hunk 1"]`)')
    click_js('document.querySelector(`button[aria-label="Select hunk 1"]`)')
    wait('document.body.innerText.includes("Next question:")')
    capture('002-diff-hunk.png',
            'The reader switches to branch changes and selects the actual diff hunk as the next question anchor. Browsing and selection invoke no answering or editing harness.',
            ['Show repo tree', 'Hunk 1', 'Reject accounts at or above their quota.', 'Ask codex'])
    click('Show repo tree')
    wait('!document.querySelector(".learning-hunk")')
    click('Ask about file')
    click_js('document.querySelector(`[aria-label="Starter questions"] button`)')
    assert evaluate('document.querySelector(".learning-question textarea").value.trim().length>0')
    assert not rows('SELECT id FROM learning_qa') and not call_records('answer')
    click_js('document.querySelector(`[role=dialog][aria-label="Learning"] button[aria-label="Close"]`)')
    wait('document.body.innerText.includes("Discard this unsent question")')
    capture('003-starter-close-protection.png',
            'A starter fills an editable question without asking it. Closing the reader asks whether to discard that unsent text; Keep writing preserves the draft and no harness has run.',
            ['Discard this unsent question and close Learning?', 'Keep writing', 'Discard question and close'], allow_error=True)
    draft = evaluate('document.querySelector(".learning-question textarea").value')
    click('Keep writing')
    assert evaluate('document.querySelector(".learning-question textarea").value') == draft
    field('Question', 'Why reject work at quota?')
    click('Ask codex')
    wait_db("SELECT status FROM learning_qa", [('answered',)])
    wait('document.querySelector(`[aria-label="Learning answer"]`)?.innerText.includes("Quota explanation")')
    original = rows('SELECT id,anchor_kind,file_path,selection_text,answer FROM learning_qa')[0]
    assert original[4] and len(call_records('answer')) == 1
    scroll('[aria-label="Learning answer"]')
    capture('004-persisted-answer.png',
            'The real question engine stores an answered row and renders the offline Codex response. The captured command is read-only; answering has started no interactive editing session.',
            ['Quota explanation', 'Why reject work at quota?', 'read the repo', 'Open editing agent'])
    close_reader()
    open_reader()
    wait('document.querySelector(`[aria-label="Learning answer"]`)?.innerText.includes("Quota explanation")')
    scroll('[aria-label="Learning answer"]')
    assert rows('SELECT id,anchor_kind,file_path,selection_text,answer FROM learning_qa') == [original]
    assert len(call_records('answer')) == 1 and not call_records('interactive')
    capture('005-reopened-history.png',
            'Closing and reopening Learning reloads the same saved answer and question from SQLite without running the answering harness again.',
            ['Quota explanation', 'Why reject work at quota?', 'Follow up', 'Keep as TODO'])
    click('Follow up')
    field('Question', 'Which boundary test?')
    click('Ask codex')
    wait_db('SELECT status FROM learning_qa ORDER BY created_at,id', [('answered',), ('answered',)])
    wait('document.querySelector(`[aria-label="Learning answer"]`)?.innerText.includes("Which boundary test?")')
    child = rows('SELECT id,parent_qa_id,anchor_kind,file_path,selection_text FROM learning_qa WHERE parent_qa_id IS NOT NULL')[0]
    assert child[1] == original[0] and child[2:5] == original[1:4]
    assert len(call_records('answer')) == 2
    assert 'Why reject work at quota?' in call_records('answer')[1]['prompt']
    scroll('[aria-label="Learning answer"]')
    capture('006-followup-thread.png',
            'A follow-up is persisted under its original question with the same captured code anchor. Its real offline request contains the earlier turn, and both answers remain in history.',
            ['Which boundary test?', 'Quota explanation', 'Keep as TODO'])
    click('Re-file as change request')
    wait_db('SELECT intent FROM learning_qa WHERE id=?', [('action',)], (child[0],))
    click('Keep as TODO')
    wait('!!document.querySelector(`form[aria-label="Keep as TODO"]`)')
    field('TODO title', 'Add quota boundary regression')
    field('TODO notes', 'Preserve the current guard; test used equal to limit.\nKept from the Learning follow-up.')
    scroll('form[aria-label="Keep as TODO"]')
    capture('007-editable-todo.png',
            'Keep as TODO offers editable title and notes. Nothing has been saved yet; cancelling this draft leaves the answer unlinked and creates no TODO.',
            ['A note about your code, not a change to it.', 'TODO title', 'TODO notes', 'Save TODO'])
    click('Cancel')
    assert not rows('SELECT id FROM todos')
    assert rows('SELECT todo_id FROM learning_qa WHERE id=?', (child[0],)) == [(None,)]
    click('Keep as TODO')
    field('TODO title', 'Add quota boundary regression')
    field('TODO notes', 'Preserve the current guard; test used equal to limit.\nKept from the Learning follow-up.')
    click('Save TODO')
    wait_db('SELECT title,status,agent_session_id FROM todos', [('Add quota boundary regression','not_started',None)])
    wait('document.body.innerText.includes("On the TODO list")')
    assert rows('SELECT todo_id FROM learning_qa WHERE id=?',(child[0],))[0][0]
    assert rows('SELECT body FROM todos') == [('Preserve the current guard; test used equal to limit.\nKept from the Learning follow-up.',)]
    assert rows('SELECT scope,feature_id FROM todo_lists JOIN todos ON todos.list_id=todo_lists.id') == [('worktree','host')]
    assert not call_records('interactive')
    scroll('[aria-label="Learning answer"]')
    capture('008-kept-todo.png',
            'Saving keeps the edited note as a not-started worktree TODO and links it back to this answer. It starts no editing session and sends no prompt.',
            ['On the TODO list', 'Open editing agent', 'Which boundary test?'])
    count = rows('SELECT COUNT(*) FROM feature_sessions')[0][0]
    click('Open editing agent')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve Learning agent"]`)')
    capture('009-editing-resource-approval.png',
            'Opening an editing agent is a separate explicit action. The concurrency warning appears before a new session or answer link is created; cancelling leaves both unchanged.',
            ['Start editing agent?', 'Start anyway', 'Cancel'])
    click('Cancel')
    assert rows('SELECT COUNT(*) FROM feature_sessions') == [(count,)]
    assert rows('SELECT spawned_session_id FROM learning_qa WHERE id=?',(child[0],)) == [(None,)]
    assert not call_records('interactive')
    click('Open editing agent')
    wait('!!document.querySelector(`[role=dialog][aria-label="Approve Learning agent"]`)')
    click('Start anyway')
    wait('!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    for _ in range(100):
        if (scratch/'worktrees/host/.fixture-received.json').is_file():
            break
        time.sleep(0.1)
    receiver=scratch/'worktrees/host/.fixture-received.json'
    assert json.loads(receiver.read_text()) == [] and len(call_records('interactive')) == 1
    linked=rows('SELECT spawned_session_id FROM learning_qa WHERE id=?',(child[0],))[0][0]
    assert linked and rows('SELECT COUNT(*) FROM feature_sessions') == [(count+1,)]
    compose='textarea[aria-label="Draft prompt"]'
    prompt=evaluate(f'document.querySelector({json.dumps(compose)}).value')
    assert 'Which boundary test?' in prompt
    input_value(compose, prompt+'\nUser review: keep this unsent until approved.')
    if evaluate('!!document.querySelector(`button[aria-label="Hide agent sidebar"]`)'):
        evaluate('document.querySelector(`button[aria-label="Hide agent sidebar"]`).click()')
        wait('!document.querySelector(`button[aria-label="Hide agent sidebar"]`)')
    wait('document.body.innerText.includes("Offline learning fixture ready")')
    scroll(compose)
    evaluate('(()=>{const e=document.querySelector(`textarea[aria-label="Draft prompt"]`);e.scrollTop=e.scrollHeight})()')
    capture('010-unsent-editing-handoff.png',
            'After explicit resource approval, Learning opens one private editing session and loads an editable composer draft. The user can revise it, and the offline receiver confirms no message has been sent.',
            ['Compose prompt', 'Send prompt'],
            'document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("User review: keep this unsent")')
    open_reader()
    wait('document.body.innerText.includes("Return to editing agent")')
    scroll('[aria-label="Learning answer"]')
    capture('011-linked-session-reuse.png',
            'Reopening Learning offers the answer’s existing editing session. Returning reuses its persisted link, creates no additional session and preserves the unsent composer edits.',
            ['Return to editing agent', 'On the TODO list', 'Which boundary test?'])
    click('Return to editing agent')
    wait('!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    assert rows('SELECT COUNT(*) FROM feature_sessions') == [(count+1,)]
    assert rows('SELECT spawned_session_id FROM learning_qa WHERE id=?',(child[0],)) == [(linked,)]
    assert json.loads(receiver.read_text()) == [] and len(call_records('interactive')) == 1
    assert evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("User review: keep this unsent")')
    (out/'validation.json').write_text(json.dumps({'native_ipc':True,'offline_answer_calls':len(call_records('answer')),'offline_interactive_calls':len(call_records('interactive')),'persisted_questions':len(rows('SELECT id FROM learning_qa')),'kept_todo':True,'unsent_handoff':True,'linked_session_reused':True},indent=2)+'\n')
finally:
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps(note)+'\n' for note in notes))
    (out/'last-state.txt').write_text(evaluate('document.body.innerText'))
    ws.close()
    x.close()
