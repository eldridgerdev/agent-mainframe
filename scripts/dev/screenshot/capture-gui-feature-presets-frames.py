#!/usr/bin/env python3
"""Assert native global preset edits through WebKit IPC and capture its X11 window.

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

import websocket
from Xlib import display

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])

for _ in range(100):
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


for _ in range(100):
    visit(x.screen().root)
    if windows:
        break
    time.sleep(0.1)
assert windows, "The isolated GUI has no X11 window"
window = max(windows, key=lambda w: w.get_geometry().width * w.get_geometry().height)
notes = []
captured_frames = set()


def readable_text():
    return 'document.body.innerText+"\\n"+Array.from(document.querySelectorAll("input,select")).map(e=>e.tagName==="SELECT"?e.selectedOptions[0]?.textContent:e.value).join("\\n")'


def capture(name, note, expects, expression=None, allow_error=False):
    # Keep transient arrival/answer notices from obscuring later decisions.
    evaluate('document.querySelectorAll(`.toasts button[aria-label="Dismiss"]`).forEach(b=>b.click())')
    wait('!document.querySelector(".toast")')
    wait('document.fonts.status==="loaded"')
    for text in expects:
        wait(f"({readable_text()}).includes({json.dumps(text)})")
    if expression:
        wait(expression)
    body = evaluate(readable_text())
    for text in expects:
        assert text in body, (text, body)
    if not allow_error:
        assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    # DOM state can advance before the native compositor paints, especially
    # during startup under a concurrent build. Cross two browser paint frames.
    evaluate('window.__amfPresetPainted=false;requestAnimationFrame(()=>requestAnimationFrame(()=>{window.__amfPresetPainted=true}))')
    wait('window.__amfPresetPainted===true')
    time.sleep(0.5)
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
config_path = scratch/'config/amf/config.json'
dbpath = pathlib.Path(sys.argv[4])

def input_field(label, value):
    selector = f'Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith({json.dumps(label)}))?.querySelector("input,select")'
    wait(f'!!({selector}) && !({selector}).disabled')
    evaluate(f"(()=>{{const e={selector};const p=e.tagName==='SELECT'?HTMLSelectElement:HTMLInputElement;Object.getOwnPropertyDescriptor(p.prototype,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event(e.tagName==='SELECT'?'change':'input',{{bubbles:true}}));}})()")

def toggle(label):
    selector=f'Array.from(document.querySelectorAll("label")).find(e=>e.textContent.trim().startsWith({json.dumps(label)}))?.querySelector("input")'
    click_js(selector)

def config():
    return json.loads(config_path.read_text())

def unchanged():
    import sqlite3
    with sqlite3.connect(dbpath) as db:
        assert json.loads(json.dumps(db.execute('SELECT * FROM features ORDER BY id').fetchall())) == json.loads((scratch/'features-before.json').read_text()), 'Saving presets must not mutate or launch existing features'
        assert db.execute('SELECT COUNT(*) FROM features').fetchone()[0]==1

try:
    window.configure(x=0,y=0,width=1280,height=950)
    x.sync()
    wait('Array.from(document.querySelectorAll(".nav-label")).some(e=>e.textContent.trim()==="Invoice API")')
    click('Feature presets')
    wait('!!document.querySelector(`[role=dialog][aria-label="Feature presets"]`) && !!document.querySelector(`input`)')
    capture('001-stored-presets.png', 'Stored global templates load without writing or changing existing features.', ['Feature presets', 'Working', 'Branch prefix', 'Remote Control'])
    unchanged()
    input_field('Preset name', 'Shipping')
    input_field('Branch prefix', 'ship')
    input_field('Harness', 'opencode')
    input_field('Mode', 'supervibe')
    input_field('Planning', 'quick')
    for label in ['Review','Chrome','Remote Control']: toggle(label)
    click('Cancel')
    capture('002-unsaved-guard.png', 'Cancel asks before discarding unsaved changes to every known preset field.', ['Discard unsaved presets and close?', 'Keep editing', 'Shipping'])
    assert config()['extension']['feature_presets'][0]['name']=='Working'
    click('Keep editing')
    capture('003-edited-template.png', 'The manager exposes branch prefix, harness, mode, Quick Plan, review, Chrome and Remote Control before explicit Save.', ['Shipping', 'Quick Plan', 'Remote Control'])
    click('Save presets')
    wait('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save presets")?.disabled')
    saved=config()
    expected={'name':'Shipping','branch_prefix':'ship','agent':'opencode','mode':'supervibe','review':True,'plan_mode':True,'quick_plan':True,'enable_chrome':True,'remote_control':True}
    assert all(saved['extension']['feature_presets'][0][key]==value for key,value in expected.items())
    assert saved['future_outer']=={'keep':42} and saved['extension']['future_nested']=={'keep':True}
    assert saved['extension']['feature_presets'][0]['future_row']=={'keep':'renamed'}
    assert saved['extension']['feature_presets'][1]['future_row']=='untouched'
    unchanged()
    input_field('Stored preset','3'); click('Delete preset'); click('Add preset')
    input_field('Preset name','Launchable'); input_field('Harness','codex'); input_field('Mode','vibe'); click('Save presets')
    wait('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Save presets")?.disabled')
    assert [p['name'] for p in config()['extension']['feature_presets']]==['Shipping','Shared','Hidden Pi','Launchable']
    click('Cancel'); click('Feature presets')
    wait('Array.from(document.querySelectorAll("input")).some(e=>e.value==="Shipping")')
    capture('004-reopened-save.png', 'Saved fields survive close/reopen; add/delete committed separately, with unknown outer and retained row data preserved.', ['Shipping', 'Quick Plan', 'Remote Control'])
    input_field('Preset name','Uncommitted rename')
    external=config();external['outside_edit']='preserved';config_path.write_text(json.dumps(external))
    click('Save presets')
    capture('005-stale-refusal.png', 'External config edits refuse stale Save and retain the local draft until an explicit confirmed Reload.', ['Global config changed. Reload presets before saving.'], allow_error=True)
    assert config()['outside_edit']=='preserved' and config()['extension']['feature_presets'][0]['name']=='Shipping'
    click('Reload presets'); click('Discard changes')
    wait('Array.from(document.querySelectorAll("input")).some(e=>e.value==="Shipping")')
    click('Cancel')
    click('Invoice API')
    click('New feature')
    wait('!!document.querySelector(`[aria-label="Feature preset"] option[value="2"]`)')
    options=evaluate('Array.from(document.querySelector(`[aria-label="Feature preset"]`).options).map(o=>o.textContent)')
    assert options==['Custom settings','Shared','Shipping','Launchable'], options
    evaluate('(()=>{const e=document.querySelector(`[aria-label="Feature preset"]`);Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,"value").set.call(e,"0");e.dispatchEvent(new Event("change",{bubbles:true}));})()')
    capture('006-effective-project-preset.png', 'Creation uses project-over-global name precedence, filters disallowed Pi, and immediately lists newly saved global templates.', ['Shared', 'Codex', 'SuperVibe', 'Review'], 'Array.from(document.querySelectorAll("input")).some(e=>e.value==="project/")')
    evaluate('(()=>{const e=document.querySelector(`[aria-label="Feature preset"]`);Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype,"value").set.call(e,"1");e.dispatchEvent(new Event("change",{bubbles:true}));})()')
    input_field('Branch / feature name','edited/shipping')
    capture('007-remote-control-block.png', 'A Remote Control template retains its intent after editing the branch; desktop creation is blocked until explicit opt-out.', ['Desktop creation does not support Remote Control yet.', 'Quick Plan'], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Create and plan")?.disabled', allow_error=True)
    toggle('Remote Control')
    toggle('Chrome')
    capture('008-editable-prefill.png', 'Explicit Remote Control opt-out enables creation; branch and Chrome edits stay authoritative after applying the template. No feature is launched in this proof.', ['Quick Plan','Review','Chrome'], 'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Create and plan")?.disabled===false')
    click('Cancel')
    unchanged()
    assert config()['outside_edit']=='preserved'
finally:
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps(note)+'\n' for note in notes))
    try: (out/'last-state.txt').write_text(evaluate('document.body.innerText'))
    except Exception: pass
    ws.close()
