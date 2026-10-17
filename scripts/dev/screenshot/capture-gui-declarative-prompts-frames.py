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
import sqlite3
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


def click(text, selector="button"):
    # Listing dormancy loads Rust state asynchronously. Its Settings button
    # is present but disabled until that load completes on slower runners.
    # "Prompt library" labels both a workspace nav item and a composer button,
    # and they open the panel differently, so those clicks pass a selector.
    wait(f'Array.from(document.querySelectorAll({json.dumps(selector)})).some(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled && b.getClientRects().length>0)')
    evaluate(
        f'Array.from(document.querySelectorAll({json.dumps(selector)})).find(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled && b.getClientRects().length>0).click()'
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



def field(label, value):
    evaluate(f"""(()=>{{const l=Array.from(document.querySelectorAll('[role=dialog] label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});if(!l)throw new Error('Missing field '+{json.dumps(label)});const e=l.querySelector('input,textarea,select');if(e.tagName==='SELECT'){{e.value={json.dumps(value)};e.dispatchEvent(new Event('change',{{bubbles:true}}));}}else{{const proto=e.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(e,{json.dumps(value)});e.dispatchEvent(new Event('input',{{bubbles:true}}));}}}})()""")


def assert_destination(key):
    picked=evaluate("""(()=>{const l=Array.from(document.querySelectorAll('[role=dialog] label')).find(l=>l.querySelector('span')?.textContent==='Agent session');const e=l?.querySelector('select');return e?[e.value,e.selectedOptions[0]?.textContent.trim()??'']:null;})()""")
    assert picked and picked[0]==key and 'no longer available' not in picked[1],picked


def choose(name):
    wait(f'Array.from(document.querySelectorAll(".library-item")).some(b=>b.querySelector("strong")?.textContent==={json.dumps(name)})')
    evaluate(f'Array.from(document.querySelectorAll(".library-item")).find(b=>b.querySelector("strong")?.textContent==={json.dumps(name)}).click()')


def bump(db):
    db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('store_version',CAST(COALESCE((SELECT value FROM store_meta WHERE key='store_version'),'0') AS INTEGER)+1)")


def editor_body():
    return evaluate('document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"] textarea\')?.value')


def composer_body():
    return evaluate('document.querySelector(".composer textarea")?.value')


def library():
    click('Prompt library', '.composer button')
    wait('document.querySelectorAll(".library-item").length>=4')


def close_editor():
    click('Cancel', '[role=dialog] button')
    wait('!document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"]\')')

def choose_source(name,source):
    wait('Array.from(document.querySelectorAll(".library-toolbar button")).some(b=>b.textContent.trim()==="Refresh"&&!b.disabled)')
    expression='Array.from(document.querySelectorAll(".library-item")).find(b=>b.querySelector("strong")?.textContent==='+json.dumps(name)+'&&b.querySelector(".badge")?.textContent==='+json.dumps(source)+')'
    wait('!!'+expression);evaluate(expression+'.click()')

try:
    window.configure(x=0,y=0,width=1440,height=1000);x.sync()
    config_path=pathlib.Path(sys.argv[4]);dbpath=config_path.parent/'amf.db'
    with sqlite3.connect(dbpath) as db:
        repo=pathlib.Path(db.execute("SELECT repo FROM projects WHERE id='shot-project'").fetchone()[0])
    worktree=repo.parent/'prompt-worktree'
    subprocess.run(['git','-C',str(repo),'worktree','add','-b','declarative-proof',str(worktree)],env=os.environ|{'GIT_CONFIG_GLOBAL':'/dev/null','GIT_CONFIG_NOSYSTEM':'1'},check=True,capture_output=True)
    definitions=[{'key':'area','kind':'select','options':['billing','api'],'required':True,'opaque_width':80},{'key':'unused','kind':'multi_line','default':'Retained notes'}]
    template={'name':'Invoice repair','body':'Repair {{area}}.','description':'Keep description and unused fields.','tags':['repair'],'placeholders':definitions,'opaque_template':{'version':2}}
    sibling={'name':'Sibling guidance','body':'Keep this sibling.','opaque_sibling':True}
    project={'opaque_project':{'keep':True},'prompt_templates':[template,sibling]}
    (repo/'.amf').mkdir(exist_ok=True);legacy=repo/'.amf/config.json';legacy.write_text(json.dumps(project,indent=1));legacy.chmod(0o640);legacy_bytes=legacy.read_bytes()
    project_path=repo/'amf.json'
    worktree_path=worktree/'amf.json';worktree_path.write_text(json.dumps({'opaque_worktree':9,'prompt_templates':[{'name':'Worktree audit','body':'Audit isolated worktree.','opaque_audit':True}]}))
    config=json.loads(config_path.read_text());config['opaque_global']={'keep':True};config.setdefault('extension',{})['prompt_templates']=[{'name':'Global review','body':'Review shared guidance.','opaque_global_template':7}];config['extension']['opaque_extension']='kept';config_path.write_text(json.dumps(config))
    with sqlite3.connect(dbpath) as db:
        stamp='2026-10-16T17:00:00Z'
        db.execute('INSERT INTO prompt_templates(id,name,description,body,tags,placeholders,created_at,updated_at,sort_order) VALUES(?,?,?,?,?,?,?,?,?)',('export-user','Invoice repair revised','User export metadata.','User repair {{area}}.',json.dumps(['user']),json.dumps(definitions),stamp,stamp,0))
        db.execute('UPDATE features SET workdir=?,is_worktree=1 WHERE id=?',(str(worktree),'shot-feature'));bump(db)
    wait('document.body.innerText.includes("Round invoice totals")');evaluate('document.querySelector("button.feature-open").click()');wait('!!document.querySelector(".composer textarea")')
    draft='Keep this unsent draft.\nUnicode: café 世界. {{area}}'
    evaluate("""(()=>{const e=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,%s);e.dispatchEvent(new Event('input',{bubbles:true}));})()"""%json.dumps(draft))
    library();choose('Invoice repair');click('Edit declarative template');wait('!!document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"]\')')
    capture('001-legacy-project-editor.png','Edit the original Project template, showing canonical write path and retained legacy source, description and both configured fields.',['Edit declarative prompt','The legacy file is kept.','2 configured placeholder definitions'])
    field('Template name','Invoice repair revised');field('Template body','Revised {{area}}.');field('Tags','repair invoice')
    click('Cancel');capture('002-unsaved-edits-protected.png','Closing a changed declarative editor asks before discarding. Keep editing preserves all local fields.',['Discard unsaved prompt edits?','Keep editing'])
    click('Keep editing')
    project['external_edit']='kept';legacy.write_text(json.dumps(project,indent=1));legacy_bytes=legacy.read_bytes()
    click('Save template');wait('!!document.querySelector("[role=alert]")')
    capture('003-stale-project-save-keeps-draft.png','A legacy source edited outside the dialog refuses saving and keeps the locally revised name/body/tags.',['Edit declarative prompt','Keep your draft and reload before saving.'],'document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"] textarea\').value==="Revised {{area}}."',allow_alert=True)
    assert not project_path.exists() and legacy.read_bytes()==legacy_bytes
    click('Reload template');click('Discard changes');wait('document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"] textarea\')?.value==="Repair {{area}}."')
    field('Template name','Invoice repair revised');field('Template body','Revised {{area}}.');field('Tags','repair invoice');click('Save template');wait('!document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"]\')')
    value=json.loads(project_path.read_text());assert legacy.read_bytes()==legacy_bytes;assert project_path.stat().st_mode&0o777==0o640;assert value['opaque_project']==project['opaque_project'];assert value['external_edit']=='kept';assert value['prompt_templates'][1]==sibling
    edited=dict(template,name='Invoice repair revised',body='Revised {{area}}.',tags=['repair','invoice']);assert value['prompt_templates'][0]==edited
    choose_source('Invoice repair revised','Project')
    capture('004-project-rename-preserves-metadata.png','A single canonical save renames the Project row, preserving opaque fields, metadata, sibling, legacy bytes and permissions.',['Invoice repair revised','Project','Sibling guidance'])
    choose('Global review');click('Edit declarative template');field('Template body','Revised global review.');capture('005-global-edit.png','Global template editing shows its actual destination and keeps unrelated global settings.',['Edit declarative prompt','Write to:'])
    click('Save template');wait('!document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"]\')');global_value=json.loads(config_path.read_text());assert global_value['opaque_global']==config['opaque_global'];assert global_value['extension']['opaque_extension']=='kept';assert global_value['extension']['prompt_templates'][0]['opaque_global_template']==7
    choose('Worktree audit');click('Edit declarative template');field('Template body','Revised isolated audit.');capture('006-worktree-edit.png','Worktree editing targets only the active isolated checkout.',['Edit declarative prompt','Write to:'])
    click('Save template');wait('!document.querySelector(\'[role=dialog][aria-label="Declarative prompt editor"]\')');assert json.loads(worktree_path.read_text())['opaque_worktree']==9
    # Select the User row with the same name explicitly rather than whichever
    # same-name row comes first in a refetched list.
    wait('Array.from(document.querySelectorAll(".library-item")).some(b=>b.querySelector("strong")?.textContent==="Invoice repair revised"&&b.querySelector(".badge")?.textContent==="User")')
    evaluate('Array.from(document.querySelectorAll(".library-item")).find(b=>b.querySelector("strong")?.textContent==="Invoice repair revised"&&b.querySelector(".badge")?.textContent==="User").click()')
    click('Export template…');field('Export destination',json.dumps({'kind':'project','project_id':'shot-project'},separators=(',',':')));click('Refresh export preview');wait('document.body.innerText.includes("Replace the same-name destination template")')
    capture('007-explicit-replacement-preview.png','Export copies the full User template into an explicit Project path and requires same-name replacement confirmation.',['Export prompt template','Replace the same-name destination template','User repair {{area}}.'],'document.querySelector(\'[role=dialog] input[type=checkbox]\')?.checked===false')
    destination=json.loads(project_path.read_text());destination['external_destination']='kept';project_path.write_text(json.dumps(destination))
    evaluate('document.querySelector(\'[role=dialog] input[type=checkbox]\').click()');click('Export template');wait('!!document.querySelector("[role=alert]")')
    capture('008-stale-export-refused.png','An external destination edit invalidates the preview. Replacement confirmation resets; the source and unsent composer remain intact.',['Export prompt template','Refresh the export preview before confirming.'],allow_alert=True)
    assert json.loads(project_path.read_text())==destination
    click('Refresh export preview');wait('!document.querySelector("[role=alert]")');evaluate('document.querySelector(\'[role=dialog] input[type=checkbox]\').click()');click('Export template');wait('!document.querySelector(\'[role=dialog][aria-label="Export prompt template"]\')')
    value=json.loads(project_path.read_text());assert value['external_destination']=='kept';assert value['prompt_templates'][0]['body']=='User repair {{area}}.';assert value['prompt_templates'][0]['opaque_template']==template['opaque_template'];assert value['prompt_templates'][0]['placeholders']==definitions;assert value['prompt_templates'][1]==sibling
    evaluate('Array.from(document.querySelectorAll(".library-item")).find(b=>b.querySelector("strong")?.textContent==="Invoice repair revised"&&b.querySelector(".badge")?.textContent==="Project").click()')
    click('Export template…');field('Export destination',json.dumps({'kind':'project','project_id':'shot-project'},separators=(',',':')));click('Refresh export preview');wait('document.body.innerText.includes("No file will be written.")')
    before=project_path.read_bytes();capture('009-self-export-is-noop.png','Exporting a template to its existing destination explicitly reports an unchanged file.',['This destination already contains the same template. No file will be written.','Confirm unchanged destination'])
    click('Confirm unchanged destination');wait('!document.querySelector(\'[role=dialog][aria-label="Export prompt template"]\')');assert project_path.read_bytes()==before
    click('Cancel');wait('!document.querySelector(\'[role=dialog][aria-label="Prompt library"]\')');assert composer_body()==draft
    evaluate('Array.from(document.querySelectorAll(".toast .btn-icon")).forEach(b=>b.click())')
    capture('010-composer-and-sessions-preserved.png','After Global, Project and Worktree edits and metadata-preserving export, the original multiline composer is unchanged and both offline sessions remain alive and empty.',['Round invoice totals','Save as template'],'document.querySelector(".composer textarea").value==='+json.dumps(draft))
    for filename in ['claude-received.json','opencode-received.json']:assert json.loads((repo/filename).read_text())==[]
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps({'file':note['file'],'note':note['note']})+'\n' for note in notes));print('PASS: declarative edits, raw preservation, legacy permissions, stale guards, explicit export and unchanged composer',flush=True)
except Exception:
    try:ws.settimeout(2);(out/'last-state.txt').write_text(evaluate('document.body.innerText') or '')
    except Exception:pass
    raise
finally:
    ws.close();x.close()
