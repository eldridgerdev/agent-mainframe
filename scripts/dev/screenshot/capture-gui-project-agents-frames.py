#!/usr/bin/env python3
"""Assert native GUI states through WebKit IPC and capture its X11 window.

Only the window belonging to the supplied isolated GUI PID is inspected.
"""

import hashlib
import json
import os
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
    raise AssertionError(f"Timed out: {expression}; body={evaluate('document.body.innerText')}")


def click(text):
    # Settings may load Rust state asynchronously; wait for enabled controls.
    wait(f'Array.from(document.querySelectorAll("button")).some(b=>!b.disabled && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)}))')
    evaluate(
        f'Array.from(document.querySelectorAll("button")).find(b=>!b.disabled && (b.textContent.trim()==={json.dumps(text)} || b.querySelector(".nav-label")?.textContent.trim()==={json.dumps(text)})).click()'
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
        assert not evaluate('!!document.querySelector(`[role=alert]`)'), body
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



global_path = pathlib.Path(sys.argv[4])
global_original = global_path.read_bytes()

def invoke(command, args=None):
    evaluate(f'window.__projectResult=null;window.__TAURI_INTERNALS__.invoke({json.dumps(command)},{json.dumps(args or {})}).then(v=>window.__projectResult={{ok:v}},e=>window.__projectResult={{err:e}})')
    wait('window.__projectResult!==null')
    result = evaluate('window.__projectResult')
    assert 'err' not in result, result
    return result['ok']

def toggle(label):
    wait(f'Array.from(document.querySelectorAll("label")).some(l=>l.textContent.trim()==={json.dumps(label)} && !l.querySelector("input").disabled)')
    evaluate(f'Array.from(document.querySelectorAll("label")).find(l=>l.textContent.trim()==={json.dumps(label)}).querySelector("input").click()')

def checked():
    return evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).filter(e=>e.checked).map(e=>e.parentElement.textContent.trim())')

def open_settings():
    wait('!!document.querySelector(`button[aria-label="Project agent availability"]`)')
    evaluate('document.querySelector(`button[aria-label="Project agent availability"]`).click()')
    wait('document.querySelectorAll("input[type=checkbox]").length===4 && !document.body.innerText.includes("Loading or saving settings…")')

def close_settings():
    click('Cancel')
    wait('!document.querySelector(`[role=dialog]`)')

try:
    window.configure(x=0, y=0, width=1380, height=1000)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    snapshot = invoke('get_snapshot')
    project = next(p for p in snapshot['projects'] if p['id'] == 'shot-project')
    project_path = pathlib.Path(project['repo']) / 'amf.json'
    tmux = ['tmux', '-S', os.environ['AMF_TMUX_SOCKET']]
    def pane_identity():
        identity = subprocess.check_output(tmux + ['display-message', '-p', '-t', 'amf-gui-agent-availability-proof:claude', '#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'], text=True).strip()
        assert re.fullmatch(r'\$[0-9]+:%[0-9]+:[1-9][0-9]*:[1-9][0-9]*', identity), identity
        return identity
    existing_identity = pane_identity()
    original = {'allowed_agents':['codex'],'future_setting':42,'future_nested':{'keep':True},'custom_sessions':[]}
    project_path.write_text(json.dumps(original))
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Round invoice totals")')
    click('Round invoice totals')
    wait('!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    draft = 'Keep this unsent prompt while changing project agents.'
    evaluate(f'(()=>{{const el=document.querySelector(`textarea[aria-label=\"Draft prompt\"]`);Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,"value").set.call(el,{json.dumps(draft)});el.dispatchEvent(new Event("input",{{bubbles:true}}));}})()')
    click('Invoice API')
    open_settings()
    assert checked() == ['Codex']
    capture('001-project-agent-loaded.png', 'The project override is distinct from global Claude Code/Codex choices and edits the tracked project config.', ['Project agent availability','Use project override','Use global setting (clear project override)','Global choices: Claude Code, Codex'])

    toggle('Pi'); click('Cancel')
    wait('!!document.querySelector(`[role=alertdialog]`)')
    capture('002-project-agent-discard.png','Unsaved project choices require explicit discard; the existing agent composer draft stays cached.', ['Discard unsaved settings and close?','Keep editing','Discard changes'])
    click('Keep editing')
    external = dict(original); external['future_setting'] = 43
    project_path.write_text(json.dumps(external))
    click('Save settings')
    wait('document.body.innerText.includes("Project or agent settings changed. Reload settings before saving.")')
    assert checked() == ['Codex','Pi']
    assert json.loads(project_path.read_text()) == external
    capture('003-project-agent-stale.png','An external project edit refuses stale saving while retaining the local Codex/Pi draft.', ['Project or agent settings changed. Reload settings before saving.','Reload settings'],allow_alert=True)
    click('Reload settings'); wait('!!document.querySelector(`[role=alertdialog]`)'); click('Discard changes')
    wait('!document.querySelector(`[role=alertdialog]`) && !document.body.innerText.includes("Loading or saving settings…")')
    assert checked() == ['Codex']
    toggle('Pi'); click('Save settings')
    wait('!document.querySelector(`[role=dialog]`)')
    saved = json.loads(project_path.read_text())
    assert saved == external | {'allowed_agents':['codex','pi']}, saved
    assert global_path.read_bytes() == global_original
    open_settings()
    assert checked() == ['Codex','Pi']
    capture('004-project-agent-saved.png','Reopening verifies the saved project Codex/Pi override with global config and unrelated project JSON unchanged.', ['Project agent availability','Global choices: Claude Code, Codex','Use project override'])
    close_settings()
    click('New feature')
    wait('!!document.querySelector(`[role=dialog][aria-label="New feature"] select`)')
    feature_choices = evaluate('Array.from(document.querySelector(`[role=dialog][aria-label="New feature"] select`).options).map(e=>e.value)')
    assert feature_choices == ['codex','pi'], feature_choices
    capture('005-project-agent-new-feature.png','New feature loads project choices before accepting input: Codex is selected and available values are asserted as Codex/Pi.', ['New feature','Harness','Mode','Create feature'])
    click('Cancel'); wait('!document.querySelector(`[role=dialog]`)'); click('Round invoice totals')
    wait('!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    assert evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).value') == draft
    click('New session'); wait('!!document.querySelector(`[role=dialog][aria-label="New session"]`)')
    types = evaluate('Array.from(document.querySelectorAll("input[name=new-session-kind]")).map(e=>e.value)')
    assert 'codex' in types and 'pi' in types and 'claude' not in types and 'opencode' not in types, types
    capture('006-project-agent-new-session.png','A later New session form offers project Codex/Pi choices while the existing Claude session and unsent composer remain intact.', ['New session','Codex','Pi','Create session'])
    click('Cancel'); wait('!document.querySelector(`[role=dialog]`)'); click('Invoice API'); open_settings()
    toggle('Use global setting (clear project override)'); click('Save settings')
    wait('!document.querySelector(`[role=dialog]`)')
    cleared = json.loads(project_path.read_text())
    assert cleared == {key:value for key,value in external.items() if key != 'allowed_agents'}, cleared
    assert global_path.read_bytes() == global_original
    open_settings()
    assert checked() == ['Claude Code','Codex']
    assert evaluate('Array.from(document.querySelectorAll("input[type=checkbox]")).every(e=>e.disabled)')
    capture('007-project-agent-inherited.png','Explicit clearing removes only project allowed_agents; reopening shows inherited global choices with disabled checkboxes.', ['Use global setting (clear project override)','Global choices: Claude Code, Codex','Project agent availability'])
    close_settings(); click('Round invoice totals')
    wait('!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    assert evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).value') == draft
    options = invoke('new_session_options',{'target':{'project_id':'shot-project','feature_id':'shot-feature'}})
    assert [c['kind'] for c in options['builtin'] if c['kind'] in ['claude','codex','opencode','pi']] == ['claude','codex'], options
    capture('008-project-agent-existing-draft.png','The existing Claude terminal remains attached with its unsent composer draft after project save and clear; no prompt was sent.', ['Round invoice totals','Claude','Compose prompt','Send prompt'])
    assert pane_identity() == existing_identity, 'An existing session or receiver was replaced'
    terminal = subprocess.check_output(tmux + ['capture-pane', '-p', '-t', 'amf-gui-agent-availability-proof:claude'], text=True)
    assert 'AMF capture fixture · Claude tab' in terminal, terminal
    for filename in ['claude-received.json', 'codex-received.json']:
        received = pathlib.Path(project['repo']) / filename
        assert received.is_file() and json.loads(received.read_text()) == [], received
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps({'file':n['file'],'note':n['note']})+'\n' for n in notes))
    print('PASS: scoped project save/clear, freshness, unsaved guard, effective new-session choices and existing draft/session preservation',flush=True)
finally:
    ws.close()
    x.close()
