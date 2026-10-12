#!/usr/bin/env python3
"""Assert native OpenCode supervised edits through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
import signal
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
    # Keep transient arrival/answer notices from obscuring later decisions.
    evaluate('document.querySelectorAll(`.toasts button[aria-label="Dismiss"]`).forEach(b=>b.click())')
    wait('!document.querySelector(".toast")')
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


scratch = pathlib.Path(sys.argv[6])
workspace = pathlib.Path(__file__).resolve().parents[3]
notify_dir=pathlib.Path(os.environ['XDG_CONFIG_HOME'])/'amf/notifications'
hooks=[]


def input_value(selector, value):
    evaluate(f'''(()=>{{const e=document.querySelector({json.dumps(selector)});const p=e.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}})()''')


def start_hook(name):
    before=set(notify_dir.glob('*.json'))
    result=scratch/(name+'-result.json')
    hook=subprocess.Popen(['node',str(workspace/'scripts/dev/screenshot/fixtures/gui-opencode-hook.mjs'),str(scratch/'change-tracker.mjs'),str(scratch/'worktrees/host'),name,str(result)],
                          cwd=scratch/'worktrees/host',env=os.environ.copy(),stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,start_new_session=True)
    hooks.append(hook)
    for _ in range(100):
        assert hook.poll() is None, ('Hook exited before a request',hook.communicate())
        created=set(notify_dir.glob('*.json'))-before
        if created:
            notification=created.pop()
            request=json.loads(notification.read_text())
            assert request['type']=='change-reason' and request['amf_feature_session_id']=='host-opencode'
            assert pathlib.Path(request['response_file']).is_relative_to(scratch/'reviews')
            assert pathlib.Path(request['proposed_file']).is_file()
            return hook,notification,request,result
        time.sleep(0.1)
    raise AssertionError('The actual OpenCode plugin did not create its private fallback request')


def edit_ready(name):
    wait('!!document.querySelector(`[role=dialog][aria-label="Supervised edits"]`)')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(name+".py")}')
    wait('!!document.querySelector(`[aria-label="Pending edit"]`)')


def confirm_open():
    wait('!!document.querySelector(`[role=alertdialog][aria-label="Confirm answer"]`)')
    evaluate('document.querySelector(`[role=alertdialog][aria-label="Confirm answer"]`).scrollIntoView({block:"nearest"})')


def finish(hook, result):
    stdout,stderr=hook.communicate(timeout=15)
    assert hook.returncode==0, (stdout,stderr)
    assert result.is_file(), 'The offline tool executor must record its result'
    wait('document.body.innerText.includes("No edits are waiting for review")')
    return json.loads(result.read_text())


try:
    window.configure(x=0,y=0,width=1280,height=950)
    x.sync()
    wait('Array.from(document.querySelectorAll(".nav-label")).some(e=>e.textContent.trim()==="Invoice API")')
    click('Invoice API')
    wait('Array.from(document.querySelectorAll("button")).some(e=>e.textContent.trim()==="host")')
    click('host')
    wait('document.body.innerText.includes("Vibeless")')
    if evaluate('!!document.querySelector(`button[aria-label="Hide agent sidebar"]`)'):
        evaluate('document.querySelector(`button[aria-label="Hide agent sidebar"]`).click()')
        wait('!document.querySelector(`button[aria-label="Hide agent sidebar"]`)')
    # Let the pending-count observer establish an empty baseline first.
    time.sleep(2)
    approve,approve_file,approve_request,approve_result=start_hook('approve')
    edit_ready('approve')
    capture('001-opencode-reason.png',
            'The shipped OpenCode change-tracker logic creates a private change-reason request and opens the native review automatically. Its generated reason, captured diff and feedback limitation are visible before any answer.',
            ['Supervised edits · host', "Agent's reason:", 'Modify code in approve.py', 'OpenCode does not forward rejection feedback', 'Approve edit'])
    assert approve.poll() is None and not pathlib.Path(approve_request['proceed_signal']).exists()
    evaluate('document.dispatchEvent(new KeyboardEvent("keydown",{key:"Escape",bubbles:true}))')
    wait('!document.querySelector(`[role=dialog][aria-label="Supervised edits"]`)')
    assert approve.poll() is None
    click_js('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim().startsWith("Supervised edits"))')
    edit_ready('approve')
    click('Approve edit')
    confirm_open()
    capture('002-approve-and-record-reason.png',
            'Approval explicitly says the agent will write the change and record its stated reason. Merely opening this confirmation leaves the real hook waiting and the source unchanged.',
            ['Approve this edit?', 'The agent writes this change and records its stated reason.', 'Send approval'])
    assert 'return used < limit' in (scratch/'worktrees/host/approve.py').read_text()
    assert approve.poll() is None
    click('Send approval')
    approved=finish(approve,approve_result)
    assert approved['status']=='written' and 'return used <= limit' in approved['after']
    assert len(approved['history'])==1 and approved['history'][0]['reason']=='Modify code in approve.py'
    assert not approve_file.exists() and not pathlib.Path(approve_request['response_file']).parent.exists()
    capture('003-approved-hook-continues.png',
            'The actual hook consumes the approval, records its reason and permits the offline tool executor to write. Its notification and request-specific reply directory are removed, and the native queue is empty.',
            ['Approved the edit to approve.py', 'No edits are waiting for review'])

    reject,reject_file,reject_request,reject_result=start_hook('reject')
    edit_ready('reject')
    input_value('textarea[aria-label="Feedback for the agent"]','Keep the strict quota boundary; this text is not forwarded.')
    click('Reject edit')
    confirm_open()
    capture('004-reject-with-opencode-effect.png',
            'OpenCode rejection states that the tool will not write and that reviewer feedback is not forwarded. The editable feedback remains local while the confirmation is open.',
            ['Reject this edit?', 'OpenCode is told the change was rejected; your feedback is not forwarded.', 'Send rejection', 'OpenCode does not forward rejection feedback'])
    assert reject.poll() is None and not pathlib.Path(reject_request['proceed_signal']).exists()
    click('Send rejection')
    rejected=finish(reject,reject_result)
    assert rejected['status']=='rejected' and rejected['error']=='Change rejected by user'
    assert rejected['after']==rejected['before'] and len(rejected['history'])==1
    assert 'strict quota boundary' not in rejected['error']
    capture('005-rejected-tool-blocked.png',
            'The shipped hook rejects the offline tool execution with its generic rejection error. The private source and change history remain unchanged; no rejection feedback reaches the executor.',
            ['Rejected the edit to reject.py', 'No edits are waiting for review'])

    cancel,cancel_file,cancel_request,cancel_result=start_hook('cancel')
    edit_ready('cancel')
    click('Cancel edit')
    confirm_open()
    capture('006-cancel-is-opencode-skip.png',
            'OpenCode cancellation is explicitly a skip: it permits the tool to write without recording a reason. The confirmation describes this effect before sending anything.',
            ['Cancel this edit?', 'OpenCode treats cancel as a skip: the agent writes this change without recording a reason.', 'Send cancellation'])
    assert cancel.poll() is None
    click('Send cancellation')
    cancelled=finish(cancel,cancel_result)
    assert cancelled['status']=='written' and 'return used <= limit' in cancelled['after']
    assert len(cancelled['history'])==2 and cancelled['history'][-1]['reason'] is None
    capture('007-skipped-reason-hook-continues.png',
            'The real hook consumes cancellation as skip, permits the offline tool write and records a null reason. The request leaves the native queue without changing the earlier approved history.',
            ['Cancelled the edit to cancel.py', 'No edits are waiting for review'])

    stale,stale_file,stale_request,stale_result=start_hook('stale')
    edit_ready('stale')
    click('Approve edit')
    confirm_open()
    pathlib.Path(stale_request['proposed_file']).write_text('def changed_after_review():\n    return False\n')
    click('Send approval')
    wait('document.querySelector(`[role=alert]`)?.textContent.includes("changed after you reviewed")')
    assert stale.poll() is None and not pathlib.Path(stale_request['proceed_signal']).exists()
    capture('008-changed-captured-diff-refused.png',
            'Changing the hook’s captured proposal after confirmation refuses the stale answer. The live hook remains blocked, its source is unchanged and no proceed signal is written.',
            ['This edit changed after you reviewed it', 'stale.py'], allow_error=True)
    os.killpg(stale.pid,signal.SIGTERM)
    stale.communicate(timeout=5)
    assert not stale_result.exists()
    click('Refresh')
    wait('document.body.innerText.includes("The agent is no longer waiting for this edit")')
    capture('009-abandoned-hook-refused.png',
            'After the request-owning OpenCode process exits, its remaining files cannot authorize an answer. Native review marks it unanswerable and disables all decisions rather than recreating its reply paths.',
            ['The agent is no longer waiting for this edit', 'Not answerable', 'stale.py'],
            'Array.from(document.querySelectorAll(".supervised-actions button")).filter(b=>["Approve edit","Reject edit","Cancel edit"].includes(b.textContent.trim())).every(b=>b.disabled)', allow_error=True)
    assert 'return used < limit' in (scratch/'worktrees/host/stale.py').read_text()
    assert not pathlib.Path(stale_request['proceed_signal']).exists()
    (out/'validation.json').write_text(json.dumps({'native_ipc':True,'shipped_opencode_plugin':True,'path_substitutions_only':3,'approved_records_reason':True,'rejected_source_unchanged':True,'feedback_not_forwarded':True,'cancel_writes_without_reason':True,'stale_and_abandoned_refused':True,'paid_harness_calls':0},indent=2)+'\n')
finally:
    for hook in hooks:
        if hook.poll() is None:
            os.killpg(hook.pid,signal.SIGTERM)
            try:
                hook.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(hook.pid,signal.SIGKILL)
                hook.communicate()
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps(note)+'\n' for note in notes))
    (out/'last-state.txt').write_text(evaluate('document.body.innerText'))
    ws.close()
    x.close()
