#!/usr/bin/env python3
"""Assert the native GUI sidebar through WebKit IPC and capture its X11 window.

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

import websocket
from Xlib import display

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
dbpath = pathlib.Path(sys.argv[4])
tmux = ["tmux", "-S", str(dbpath.parents[2] / "sidebar-tmux.sock")]
# The fixture makes the session name unique per run; read it back.
with sqlite3.connect(dbpath) as db:
    round_tmux = db.execute("SELECT tmux_session FROM features WHERE id='f-round'").fetchone()[0]

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
ws = websocket.create_connection(f"ws://{sys.argv[3]}{inspector_path}", timeout=15, suppress_origin=True)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def evaluate(expression):
    global seq
    seq += 1
    request = {"id": seq, "method": "Runtime.evaluate", "params": {"expression": expression, "returnByValue": True}}
    ws.send(json.dumps({"id": seq + 10000, "method": "Target.sendMessageToTarget",
                        "params": {"targetId": target, "message": json.dumps(request)}}))
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                assert not message["result"].get("wasThrown"), message
                return message["result"]["result"].get("value")


def wait(expression, tries=200):
    for _ in range(tries):
        if evaluate(expression):
            return
        time.sleep(0.1)
    raise AssertionError(expression + "\n" + str(evaluate("document.querySelector('.sidebar').innerText")))


NAV = "document.querySelector('nav[aria-label=Workspace]')"


def nav_has(text):
    return f"({NAV}?.innerText ?? '').includes({json.dumps(text)})"


def click_nav(name):
    """Click a tree button by its exact accessible text or aria-label."""
    evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('button')).find(b=>(b.getAttribute('aria-label')||b.textContent.trim())==={json.dumps(name)});if(!b)throw new Error('no button '+{json.dumps(name)});b.click();}})()""")


def click_session(label):
    evaluate(f"""Array.from({NAV}.querySelectorAll('.tree-session-label')).find(l=>l.textContent==={json.dumps(label)}).closest('button').click()""")


def click(text):
    evaluate(f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()')


def glyph(feature):
    return evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('.tree-name')).find(b=>b.textContent.trim()==={json.dumps(feature)});return b.closest('.tree-row').querySelector('.tree-glyph').getAttribute('aria-label');}})()""")


def marker(feature):
    return evaluate(f"""(()=>{{const b=Array.from({NAV}.querySelectorAll('.tree-name')).find(b=>b.textContent.trim()==={json.dumps(feature)});return !!b.closest('.tree-row').querySelector('.tree-pending');}})()""")


def db_value(sql):
    with sqlite3.connect(dbpath) as db:
        return db.execute(sql).fetchone()[0]


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


def capture(name, note, expects, expression=None):
    wait('document.fonts.status==="loaded"')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    time.sleep(0.4)
    g = window.get_geometry()
    subprocess.run(
        ["/usr/bin/python3", "-c",
         """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
         str(window.id), str(g.width), str(g.height), str(out / name)],
        check=True,
    )
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note})
    print("PASS:", name, note, flush=True)


SB = "document.querySelector('.agent-sidebar')"
BODY = SB + ".querySelector('.agent-sidebar-body')"


def aria(name):
    evaluate(f"document.querySelector('button[aria-label={json.dumps(name)}]').click()")


def sidebar_button(section, name):
    evaluate(f"Array.from(document.querySelector('.sb-{section}').querySelectorAll('button')).find(b=>b.textContent.trim()==={json.dumps(name)}).click()")


def fill(value):
    evaluate(f"""(()=>{{const el=document.querySelector('.composer textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def pane_width():
    return int(subprocess.check_output(tmux + ["display-message", "-p", "-t", f"{round_tmux}:claude", "#{pane_width}"], text=True).strip())


try:
    window.configure(x=0, y=0, width=1440, height=960)
    x.sync()
    wait(nav_has("Round totals"))
    click_nav("Round totals")
    wait(f"{SB}?.innerText.includes('Claude Sidebar')")
    wait(f"{SB}?.innerText.includes('claude-sonnet')")
    wait("!!document.querySelector('.sb-todos [role=progressbar]')")
    wait("!!document.querySelector('.sb-prompt button') && !!document.querySelector('.sb-active_todo button')")
    wait("document.querySelector('.xterm-rows')?.innerText.includes('Waiting for the next instruction')")
    wait("!!document.querySelector('.sb-context') && document.querySelector('.sb-status').innerText.includes('Effective')")
    wait("document.querySelector('.sb-pr_triage')?.innerText.includes('#321')")
    sections = evaluate("Array.from(document.querySelectorAll('.sb-section')).map(s=>s.getAttribute('aria-label'))")
    assert sections == ['Status','Context','Plan','Issue','PR Triage','Work','Summary','Prompt','Todos','Active TODO'], sections
    assert evaluate("document.querySelector('.sb-context').dataset.band") == 'warning'
    capture('001-claude-full-sidebar.png',
        'Claude sidebar runs its own collectors without a TUI: input/output/effective tokens, cost/model, warning estimated context, current plan, issue/PR, thinking marker, summary, last prompt and agent TODOs. Unsupported quota is omitted.',
        ['Claude Sidebar','Input','Output','Effective','Cost','Model','AMF_PLAN.md','PR#321','thinking'],
        "!!document.querySelector('.sb-todos [role=progressbar]') && !document.querySelector('.sb-usage')")

    evaluate(f"{BODY}.scrollTop={BODY}.scrollHeight")
    capture('002-claude-todos-and-prompt.png',
        'The lower sidebar shows the clamped prompt with View/Reuse, a 1/3 TODO bar, and the linked AMF TODO title clamped to two lines with completion. TUI-only signals are explicitly unavailable.',
        ['Finish: invoice rounding','TODOS','ACTIVE TODO','Complete','running tool calls'],
        "document.querySelector('.sb-todos [role=progressbar]').getAttribute('aria-valuenow')==='1'")
    fill('Keep this unsent draft.')
    sidebar_button('prompt','Reuse')
    wait("document.querySelector('.composer textarea').value.includes('Keep this unsent draft.') && document.querySelector('.composer textarea').value.includes('Round invoice totals, check negative')")
    draft = evaluate("document.querySelector('.composer textarea').value")
    evaluate("document.querySelector('.term-surface').dispatchEvent(new WheelEvent('wheel',{deltaY:-300,deltaMode:0,bubbles:true,cancelable:true}))")
    wait("!!document.querySelector('.term-scrollback')")
    time.sleep(.5)
    before = evaluate("document.querySelector('.xterm-rows').innerText")
    width = pane_width()
    aria('Hide agent sidebar')
    wait("!!document.querySelector('.agent-sidebar-collapsed')")
    for _ in range(60):
        if pane_width() > width: break
        time.sleep(.1)
    wider = pane_width()
    assert wider > width, (width,wider)
    assert evaluate("document.querySelector('.xterm-rows').innerText") == before
    capture('003-collapsed-terminal-resized.png',
        f'Collapsing widens the real tmux pane from {width} to {wider} columns through resize_terminal. Earlier output and the unsent reused-prompt draft stay in place; the rail remains available.',
        ['Viewing earlier output','Jump to latest','Sidebar'],
        "document.querySelector('.composer textarea').value===" + json.dumps(draft))
    aria('Show agent sidebar')
    for _ in range(60):
        if pane_width() == width: break
        time.sleep(.1)
    assert pane_width() == width
    assert evaluate("document.querySelector('.xterm-rows').innerText") == before
    capture('004-expanded-position-kept.png',
        'Restoring the sidebar narrows the same tmux pane back without reattaching or leaving scrollback, and keeps the composer draft.',
        ['Claude Sidebar','Viewing earlier output'],
        "document.querySelector('.composer textarea').value===" + json.dumps(draft))

    sidebar_button('plan','Open')
    wait("document.querySelector('[role=dialog]')?.innerText.includes('Invoice rounding plan')")
    capture('005-open-plan.png','Open reads the current plan file in a native GUI modal without editing it.',
        ['Current plan','Invoice rounding plan','Round negative totals'])
    aria('Close')
    evaluate(f"{BODY}.scrollTop={BODY}.scrollHeight")
    sidebar_button('active_todo','Complete')
    wait("!!document.querySelector('.sb-confirm')")
    evaluate("Array.from(document.querySelector('.sb-confirm').querySelectorAll('button')).find(b=>b.textContent.trim()==='Complete').click()")
    wait("document.querySelector('.sb-active_todo')?.innerText.includes('completed')")
    assert db_value("SELECT status FROM todos WHERE id='sidebar-todo'") == 'completed'
    capture('006-linked-todo-completed.png','Completion updates only the linked TODO through the shared TUI engine; its session association is retained.',
        ['ACTIVE TODO','completed'],"!document.querySelector('.sb-active_todo .sb-action')")

    evaluate("Array.from(document.querySelectorAll('[role=tab]')).find(b=>b.textContent.trim()==='Codex 1').click()")
    wait(f"{SB}?.innerText.includes('Codex Sidebar')")
    wait("document.querySelector('.sb-usage')?.innerText.includes('62%')")
    wait("document.querySelector('.sb-prompt')?.innerText.includes('Check the rounding')")
    wait("!!document.querySelector('.sb-context') && document.querySelector('.sb-status').innerText.includes('gpt-5-codex')")
    capture('007-codex-usage-sidebar.png',
        'Codex reads its own rollout model/prompt/context and account quota offline: 5h at 62% and 7d at 91%, with reset times and medium/high colours. It has no Claude TODO list.',
        ['Codex Sidebar','5h','62%','7d','91%','gpt-5-codex'],
        "!document.querySelector('.sb-todos') && document.querySelector('.usage-high')!==null")

    click_nav('release-prep')
    wait(f"{SB}?.innerText.includes('Pi Sidebar')")
    wait(f"{SB}?.innerText.includes('No plan selected')")
    assert evaluate("Array.from(document.querySelectorAll('.sb-section')).map(s=>s.getAttribute('aria-label'))") == ['Status','Plan','PR Triage']
    capture('008-pi-sparse-sidebar.png',
        'A sparse Pi tab omits unavailable tokens, usage, context, prompt, work, summary and TODOs. The same shared TUI plan placeholder and persisted merged-PR state remain.',
        ['Pi Sidebar','No plan selected','#12 merged'],
        "!document.querySelector('.sb-context') && !document.querySelector('.sb-prompt')")
    (out / 'native-notes.json').write_text(json.dumps(notes,indent=2))
finally:
    ws.close()
    x.close()
