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
    wait(f'Array.from(document.querySelectorAll({json.dumps(selector)})).some(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled)')
    evaluate(
        f'Array.from(document.querySelectorAll({json.dumps(selector)})).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
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


try:
    window.configure(x=0, y=0, width=1440, height=1000)
    x.sync()
    config_path=pathlib.Path(sys.argv[4])
    dbpath=config_path.parent/'amf.db'
    with sqlite3.connect(dbpath) as db:
        repo=pathlib.Path(db.execute("SELECT repo FROM projects WHERE id='shot-project'").fetchone()[0])
    worktree=repo.parent/'prompt-worktree'
    # The runner passes its GUI environment, not its private git one, so the
    # user's global and system git config (hooks included) is shut out here.
    git_env=os.environ|{'GIT_CONFIG_GLOBAL':'/dev/null','GIT_CONFIG_NOSYSTEM':'1'}
    added=subprocess.run(['git','-C',str(repo),'worktree','add','-b','prompt-library-proof',str(worktree)],env=git_env,capture_output=True,text=True)
    assert added.returncode==0,f'git worktree add failed: {added.stderr}'
    template={"name":"Invoice repair","description":"Repair invoice behavior without sending a prompt.","tags":["bug"],"body":"Fix {{area}} in {{env: dev|prod}}.\n{{notes}}\nUse {{rust|go}}.","placeholders":[{"key":"area","label":"Area","kind":"text","default":"billing","required":True},{"key":"notes","label":"Notes","kind":"multi_line","default":"Keep unit tests."}]}
    project_config={"prompt_templates":[template]}
    (repo/'amf.json').write_text(json.dumps(project_config))
    (worktree/'amf.json').write_text(json.dumps({"prompt_templates":[{"name":"Worktree audit","body":"Audit only this worktree.","tags":["audit"]}]}))
    config=json.loads(config_path.read_text())
    config.setdefault('extension',{})['prompt_templates']=[{"name":"Global review","body":"Review the shared defaults.","tags":["review"]}]
    config_path.write_text(json.dumps(config))
    with sqlite3.connect(dbpath) as db:
        stamp='2026-10-11T17:00:00Z'
        db.execute("INSERT INTO prompt_templates(id,name,body,tags,placeholders,created_at,updated_at,sort_order) VALUES(?,?,?,?,?,?,?,?)",('proof-user','User repair','Read user guidance.',json.dumps(['bug']),'[]',stamp,stamp,0))
        db.execute("UPDATE features SET workdir=?,is_worktree=1 WHERE id='shot-feature'",(str(worktree),))
        bump(db)
    wait('document.body.innerText.includes("Round invoice totals")')
    evaluate('document.querySelector("button.feature-open").click()')
    wait('!!document.querySelector(\'[role=tablist][aria-label="Feature sessions"]\')')
    evaluate('Array.from(document.querySelectorAll("[role=tab]")).find(b=>b.textContent.trim()==="Codex").click()')
    wait('!!document.querySelector(".composer textarea")')
    evaluate("""(()=>{const e=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(e,'Existing unsent draft');e.dispatchEvent(new Event('input',{bubbles:true}));})()""")
    wait('document.querySelector(".composer textarea").value==="Existing unsent draft"')
    # The composer's button hands its own session to the panel as the
    # destination; the workspace nav item (the last open below) hands none.
    codex=json.dumps(['shot-project','shot-feature','shot-codex'],separators=(',',':'))
    click('Prompt library','.composer button')
    wait('document.querySelectorAll(".library-item").length===4')
    capture('001-four-library-sources.png','Opened from the Codex composer, the feature library reads user SQLite templates plus worktree, project and global config sources through real Rust IPC. Opening it preserves the existing unsent Codex draft.',['Prompt library','User repair','Worktree audit','Invoice repair','Global review'])
    field('Search prompts','#bug')
    wait('document.querySelectorAll(".library-item").length===2')
    choose('Invoice repair')
    assert_destination(codex)
    field('Area (required)','')
    wait('Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==="Add to draft").disabled')
    capture('002-required-field-blocked.png','Tag search selects two bug templates, and the Codex session whose composer opened the library is already the destination. The required Area field blocks insertion until filled; original and resolved prompt previews remain available.',['Invoice repair','Fill the required fields','Prompt preview','Agent session'])
    field('Area (required)','invoice totals')
    field('Notes','Keep tests.\nCheck rounding.')
    field('env','prod')
    field('Choose an option','go')
    wait('document.querySelector(\'[aria-label="Resolved prompt"]\').textContent==="Fix invoice totals in prod.\\nKeep tests.\\nCheck rounding.\\nUse go."')
    capture('003-filled-preview.png','Text, multiline and choice fields resolve locally into a live preview. Add to draft remains explicit, and no prompt has been sent.',['Invoice repair','Fix invoice totals in prod.','Check rounding.','Use go.'])
    click('Cancel')
    wait('!document.querySelector(\'[role=dialog][aria-label="Prompt library"]\')')
    assert evaluate('document.querySelector(".composer textarea").value')=='Existing unsent draft'
    click('Prompt library','.composer button')
    choose('Invoice repair')
    assert_destination(codex)
    # An external config edit retires the selected content key on refresh.
    project_config['prompt_templates'][0]['body']='Updated repair {{area}}.'
    (repo/'amf.json').write_text(json.dumps(project_config))
    click('Refresh')
    wait('document.body.innerText.includes("was changed or removed outside this window")')
    capture('004-external-edit-retired.png','Refreshing after an external project template edit retires the old selection and refuses insertion until the current version is selected. The existing composer draft is unchanged.',['was changed or removed outside this window','Select a prompt'])
    choose('Invoice repair')
    assert_destination(codex)
    field('Area (required)','invoice totals')
    wait('document.querySelector(\'[aria-label="Resolved prompt"]\').textContent==="Updated repair invoice totals."')
    click('Add to draft')
    wait('!document.querySelector(\'[role=dialog][aria-label="Prompt library"]\')')
    wait('document.querySelector(".composer textarea").value.includes("Updated repair invoice totals.")')
    draft=evaluate('document.querySelector(".composer textarea").value')
    assert draft.startswith('Existing unsent draft') and draft.count('Updated repair invoice totals.')==1,draft
    capture('005-appended-unsent-draft.png','Explicit insertion appends the reselected template to the existing Codex draft once. Both private offline agent receivers remain empty, proving that insertion never sends the prompt.',['Round invoice totals','Codex'],'document.querySelector(".composer textarea").value.includes("Existing unsent draft") && document.querySelector(".composer textarea").value.includes("Updated repair invoice totals.")')
    for filename in ['claude-received.json','codex-received.json']:
        assert json.loads((repo/filename).read_text())==[],f'Prompt library insertion submitted a prompt ({filename})'
    click('Prompt library','.nav-item')
    choose('User repair')
    assert_destination('')
    field('Library scope',json.dumps({'kind':'global'},separators=(',',':')))
    wait('document.querySelectorAll(".library-item").length===2')
    capture('006-global-scope.png','Reopened from workspace navigation, which preselects no session. Switching to User and global prompts hides project/worktree entries while retaining the two shared sources. Browsing and cancellation leave both live sessions and the appended unsent draft intact.',['User repair','Global review','Select a prompt'],'!Array.from(document.querySelectorAll(".library-item strong")).some(e=>["Invoice repair","Worktree audit"].includes(e.textContent))')
    click('Cancel')
    assert evaluate('document.querySelector(".composer textarea").value')==draft
    with sqlite3.connect(dbpath) as db:
        assert db.execute('SELECT count(*) FROM prompt_templates').fetchone()[0]==1
        assert db.execute('SELECT count(*) FROM feature_sessions').fetchone()[0]==2
    (out/'capture-notes.jsonl').write_text(''.join(json.dumps({'file':n['file'],'note':n['note']})+'\n' for n in notes))
    print('PASS: sources, tag search, required/multiline/choice fields, cancellation, external edits and append-only unsent handoff',flush=True)
except Exception:
    # Best effort. A dead inspector must not replace the failure being reported.
    try:
        ws.settimeout(2)
        (out / "last-state.txt").write_text(evaluate("document.body.innerText") or "")
    except Exception:
        pass
    raise
finally:
    ws.close()
    x.close()
