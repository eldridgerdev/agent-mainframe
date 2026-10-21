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
    return evaluate('document.querySelector(\'[role=dialog][aria-label="User prompt editor"] textarea\')?.value')


def composer_body():
    return evaluate('document.querySelector(".composer textarea")?.value')


def library():
    click('Prompt library', '.composer button')
    wait('document.querySelectorAll(".library-item").length>=4')


def close_editor():
    click('Cancel', '[role=dialog] button')
    wait('!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')


try:
    window.configure(x=0, y=0, width=1440, height=1000)
    x.sync()
    config_path = pathlib.Path(sys.argv[4])
    dbpath = config_path.parent / 'amf.db'
    with sqlite3.connect(dbpath) as db:
        repo = pathlib.Path(db.execute("SELECT repo FROM projects WHERE id='shot-project'").fetchone()[0])
    worktree = repo.parent / 'prompt-worktree'
    git_env = os.environ | {'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
    subprocess.run(['git','-C',str(repo),'worktree','add','-b','prompt-authoring-proof',str(worktree)], env=git_env, check=True, capture_output=True)
    definitions = [{'key':'area','kind':'select','options':['billing','api'],'required':True}, {'key':'unused','kind':'multi_line','default':'Retained configured notes'}]
    template = {'name':'Invoice repair','description':'Keep all configured metadata.','tags':['repair'],'body':'Repair {{area}}.','placeholders':definitions}
    project = {'prompt_templates':[template, {'name':'Future metadata','body':'Read-only future format.','future_format':{'version':2}}]}
    project_path = repo / 'amf.json'
    project_path.write_text(json.dumps(project))
    worktree_path = worktree / 'amf.json'
    worktree_path.write_text(json.dumps({'prompt_templates':[{'name':'Worktree audit','body':'Audit the isolated checkout.'}]}))
    config = json.loads(config_path.read_text())
    config.setdefault('extension', {})['prompt_templates'] = [{'name':'Global review','body':'Review shared guidance.'}]
    config_path.write_text(json.dumps(config))
    source_bytes = {path: path.read_bytes() for path in [project_path, worktree_path, config_path]}
    raw_definitions = json.dumps([dict(definitions[0],future_format={'width':80}), definitions[1]], indent=1)
    with sqlite3.connect(dbpath) as db:
        stamp = '2026-10-15T17:00:00Z'
        db.execute('INSERT INTO prompt_templates(id,name,description,body,tags,placeholders,created_at,updated_at,sort_order) VALUES(?,?,?,?,?,?,?,?,?)', ('proof-user','User repair','Preserved User description.','Repair {{area}}.',json.dumps(['bug']),raw_definitions,stamp,stamp,0))
        db.execute('UPDATE features SET workdir=?,is_worktree=1 WHERE id=?',(str(worktree),'shot-feature'))
        bump(db)
    wait('document.body.innerText.includes("Round invoice totals")')
    evaluate('document.querySelector("button.feature-open").click()')
    wait('!!document.querySelector(".composer textarea")')
    draft = 'Keep this unsent composer draft.\nUnicode: café 世界. {{area}}'
    evaluate("""(()=>{const e=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,%s);e.dispatchEvent(new Event('input',{bubbles:true}));})()""" % json.dumps(draft))
    wait('document.querySelector(".composer textarea").value===' + json.dumps(draft))
    library()
    capture('001-user-and-declarative-sources.png','The native library offers User authoring alongside read-only global, project and worktree sources; the existing composer remains unsent.',['User repair','Invoice repair','Worktree audit','Global review','New User template'])
    choose('Invoice repair')
    assert not evaluate('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Edit User template")')
    click('Duplicate to User')
    wait('document.body.innerText.includes("Duplicated to your User library.")')
    # The new copy is selected on its own; no click is needed to reach it.
    wait('document.querySelector(".library-item-active strong")?.textContent==="Invoice repair (copy)"')
    click('Edit User template')
    wait('!!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')
    capture('002-declarative-copy-keeps-metadata.png','Duplicate to User creates a fresh editable copy with the original description and both configured placeholder definitions, including the unused definition. Declarative source files remain byte-identical.',['Edit User prompt','Keep all configured metadata.','2 configured placeholder definitions'])
    close_editor()
    choose('User repair')
    click('Edit User template')
    wait('!!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')
    field('Template name','User repair revised')
    field('Template body','Local revised {{area}}. Keep my edits.')
    field('Tags','bug, invoice')
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE prompt_templates SET body='External User edit' WHERE id='proof-user'")
    click('Save User template')
    wait('document.body.innerText.includes("changed or was deleted")')
    assert editor_body() == 'Local revised {{area}}. Keep my edits.'
    capture('003-stale-save-keeps-local-edit.png','A concurrent SQLite edit refuses the delayed save. The editor keeps the local name, tags and body; raw description and placeholder metadata remain untouched.',['changed or was deleted','Keep your draft','Preserved User description.'],allow_alert=True)
    click('Reload template')
    click('Discard changes')
    wait('document.querySelector(\'[role=dialog][aria-label="User prompt editor"] textarea\').value==="External User edit"')
    field('Template name','User repair revised')
    field('Template body','Saved revised {{area}}.')
    field('Tags','#bug invoice BUG')
    click('Save User template')
    wait('!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')
    with sqlite3.connect(dbpath) as db:
        row=db.execute("SELECT name,body,tags,description,placeholders FROM prompt_templates WHERE id='proof-user'").fetchone()
        assert row == ('User repair revised','Saved revised {{area}}.',json.dumps(['bug','invoice'],separators=(',',':')),'Preserved User description.',raw_definitions),row
    choose('Invoice repair (copy)')
    click('Delete User template…')
    wait('!!document.querySelector(\'[role=dialog][aria-label="Delete User template"]\')')
    capture('004-explicit-user-delete-confirmation.png','Deletion names the User copy and requires a separate confirmation. The configured project source and existing composer are kept.',['Delete this User template?','Invoice repair (copy)','Existing composer drafts and declarative templates are kept.'])
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE prompt_templates SET body='Copy changed elsewhere' WHERE name='Invoice repair (copy)'")
    click('Delete User template')
    wait('document.body.innerText.includes("changed or was deleted")')
    capture('005-stale-delete-refused.png','The exact persisted row is rechecked under the SQLite writer lock. Changing the copy after confirmation opens refuses deletion, leaving both User rows and all declarative sources intact.',['Delete this User template?','changed or was deleted'],allow_alert=True)
    click('Cancel')
    click('Refresh')
    wait('document.body.innerText.includes("was changed or removed outside this window")')
    choose('Invoice repair (copy)')
    click('Delete User template…')
    wait('!!document.querySelector(\'[role=dialog][aria-label="Delete User template"]\')')
    click('Delete User template')
    wait('document.body.innerText.includes("Deleted from your User library.")')
    # Exercise every other source through real duplicate/delete IPC too.
    for source_name, expected_body in [('User repair revised','Saved revised {{area}}.'), ('Global review','Review shared guidance.'), ('Worktree audit','Audit the isolated checkout.')]:
        choose(source_name)
        click('Duplicate to User')
        wait('document.body.innerText.includes("Duplicated to your User library.")')
        copy_name = source_name + ' (copy)'
        with sqlite3.connect(dbpath) as db:
            copy = db.execute('SELECT body,placeholders FROM prompt_templates WHERE name=?', (copy_name,)).fetchone()
            assert copy and copy[0] == expected_body, (source_name,copy)
            if source_name == 'User repair revised':
                assert copy[1] == raw_definitions
        choose(copy_name)
        click('Delete User template…')
        wait('!!document.querySelector(\'[role=dialog][aria-label="Delete User template"]\')')
        click('Delete User template')
        wait('document.body.innerText.includes("Deleted from your User library.")')
        print('PASS: duplicated then explicitly deleted private User copy from ' + source_name, flush=True)
    click('New User template')
    field('Template name','New audit')
    field('Tags','audit, invoice')
    field('Template body','Audit {{area}}.\nKeep Unicode: café 世界.')
    capture('006-create-user-template.png','A new User template has editable name, tags and multiline body. No declarative file is edited, and nothing is sent to either offline session.',['New User prompt','Template name','Template body','Save User template'])
    click('Cancel')
    capture('007-unsaved-authoring-protected.png','Cancel protects unsaved template edits with an explicit discard choice. Keep editing restores the fields; the original composer draft stays mounted.',['Discard unsaved prompt edits?','Keep editing','The original composer draft is kept.'])
    click('Keep editing')
    click('Save User template')
    wait('document.body.innerText.includes("Saved in your User library.")')
    choose('Future metadata')
    click('Duplicate to User')
    wait('document.body.innerText.includes("unsupported metadata")')
    capture('008-unsupported-source-copy-refused.png','Browsing remains tolerant of future declarative fields, but copying refuses metadata that could be lost. The source is unchanged and no User copy is created.',['Future metadata','unsupported metadata'],allow_alert=True)
    click('Cancel')
    assert composer_body() == draft
    click('Save as template','.composer button')
    wait('!!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')
    assert editor_body() == draft
    field('Template name','Saved composer')
    capture('009-save-unsent-composer-as-template.png','Save as template copies the unsent composer text into a new User editor. It neither clears nor sends the mounted composer draft.',['New User prompt','Save User template','Template body'])
    click('Save User template')
    wait('!document.querySelector(\'[role=dialog][aria-label="User prompt editor"]\')')
    assert composer_body() == draft
    evaluate('Array.from(document.querySelectorAll(".toast .btn-icon")).forEach(b=>b.click())')
    capture('010-saved-template-keeps-unsent-draft.png','After saving, the original multiline composer is unchanged and both private offline receivers remain empty. User metadata and declarative source bytes are preserved.',['Round invoice totals','Save as template'],'document.querySelector(".composer textarea").value===' + json.dumps(draft))
    with sqlite3.connect(dbpath) as db:
        rows = db.execute('SELECT name,body FROM prompt_templates ORDER BY sort_order').fetchall()
        assert rows == [('User repair revised','Saved revised {{area}}.'),('New audit','Audit {{area}}.\nKeep Unicode: café 世界.'),('Saved composer',draft)],rows
    for path, baseline in source_bytes.items():
        assert path.read_bytes() == baseline, f'Source changed: {path.name}'
    for filename in ['claude-received.json','opencode-received.json']:
        assert json.loads((repo/filename).read_text()) == [], 'Authoring submitted a prompt'
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps({'file':note['file'],'note':note['note']})+'\n' for note in notes))
    print('PASS: User create/edit/delete, metadata-preserving copy, stale refusal, unsaved protection, source preservation and unsent composer', flush=True)
except Exception:
    try:
        ws.settimeout(2)
        (out / 'last-state.txt').write_text(evaluate('document.body.innerText') or '')
    except Exception:
        pass
    raise
finally:
    ws.close()
    x.close()
