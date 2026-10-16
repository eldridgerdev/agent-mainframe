#!/usr/bin/env python3
"""Native project defaults through maintained driver controls and real IPC."""
import importlib.util
import json
import os
import pathlib
import sqlite3
import subprocess
import sys
import time

from gui_frames import GuiFrames

scripts = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('amf_native_driver', scripts.parent/'gui-driver/amf-gui-driver.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
out = pathlib.Path(sys.argv[1]); dbpath = pathlib.Path(sys.argv[4]); scratch = pathlib.Path(sys.argv[6])
tmux = [os.environ['AMF_TMUX_BIN'], '-S', os.environ['AMF_TMUX_SOCKET']]
def query(sql, params=()):
    with sqlite3.connect(dbpath) as db: return db.execute(sql, params).fetchall()
def preference(): return query("SELECT preferred_agent FROM projects WHERE id='proof-project'")[0][0]
def rows():
    return query("SELECT id,kind,label,tmux_window,claude_session_id,stopped,command,on_stop,token_usage_source,token_usage_source_match FROM feature_sessions WHERE feature_id='proof-feature' ORDER BY sort_order")
def original_panes():
    return sorted(line for line in subprocess.check_output(tmux+['list-panes','-a','-F','#{session_name}:#{window_name}:#{pane_pid}'],text=True).splitlines() if line.startswith('amf-harness-proof:'))
def wait_for(condition, label):
    for _ in range(150):
        if condition(): return
        time.sleep(.1)
    raise AssertionError(label)

with GuiFrames.from_argv() as frames:
    class InspectorAdapter:
        def evaluate(self, expression, deadline): return frames.evaluate(expression)
    driver = module.Driver(InspectorAdapter()); driver.inject(time.monotonic()+10)
    def action(op, **kwargs): return driver.run({'op':op, **kwargs})
    def click(name, scope=None): return action('click',name=name, **({'scope':scope} if scope else {}))
    def select(name,value): return action('select',name=name,value=value,scope='[role=dialog]')
    def fill(name,value): return action('fill',name=name,value=value,scope='[role=dialog]')
    def wait(expression): return action('wait',expression=expression)
    def selected(): return frames.evaluate('document.querySelector(`[role=dialog] select`).value')
    def dismiss_toasts(): frames.evaluate('Array.from(document.querySelectorAll(`.toast button[aria-label="Dismiss"]`)).forEach(button=>button.click())')
    def open_project():
        action('click',selector='nav[aria-label=Workspace] [aria-describedby="tree-project-proof-project"]')
        action('wait',text='Preferred harness',scope='main')
    frames.window.configure(width=1320,height=960)
    action('wait',text='Billing API',scope='nav[aria-label=Workspace]')
    open_project()
    click('Preferred harness')
    wait('!!document.querySelector(`[role=dialog] select`) && !document.querySelector(`[role=dialog] select`).disabled')
    initial_rows, initial_panes = rows(), original_panes()
    assert preference()=='claude' and selected()=='claude'
    frames.capture('001-project-preference.png','The project preference is a separate action from agent availability and changes only new-feature defaults.', ['Project preferred harness','Existing sessions keep their harness','Nothing starts or stops'])
    select('Preferred harness','opencode'); click('Cancel','[role=dialog]')
    frames.capture('002-unsaved-preference.png','Cancel protects an edited preference; choosing OpenCode alone changes no saved bindings or running panes.', ['Discard unsaved settings','Keep editing','Discard changes'])
    assert preference()=='claude' and rows()==initial_rows and original_panes()==initial_panes
    click('Keep editing')
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE projects SET preferred_agent='pi' WHERE id='proof-project'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    click('Save settings','[role=dialog]')
    action('wait',text='changed. Reload before saving.',scope='[role=dialog]')
    frames.capture('003-stale-preference.png','A real external SQLite preference edit invalidates the open form without touching any existing session.', ['changed. Reload before saving.','Project preferred harness'],allow_alert=True)
    assert preference()=='pi' and rows()==initial_rows and original_panes()==initial_panes
    click('Reload settings','[role=dialog]'); click('Discard changes')
    wait('document.querySelector(`[role=dialog] select`)?.value === "pi"')
    select('Preferred harness','opencode'); click('Save settings','[role=dialog]')
    wait('!document.querySelector(`[role=dialog]`)')
    assert preference()=='opencode' and rows()==initial_rows and original_panes()==initial_panes
    assert query("SELECT agent,mode FROM features WHERE id='proof-feature'")==[('claude','supervibe')]
    frames.capture('004-preference-saved.png','Saving the project preference preserves the running Claude feature and every session binding and pane PID.', ['Billing API','Project preference saved','Existing sessions keep their harness'])
    dismiss_toasts(); click('New feature')
    wait('Array.from(document.querySelectorAll(`[role=dialog] label`)).find(e=>e.textContent.startsWith("Harness"))?.querySelector("select").value === "opencode" && !Array.from(document.querySelectorAll(`[role=dialog] button`)).find(b=>b.textContent.trim()==="Reload harnesses")?.disabled')
    assert frames.evaluate('Array.from(document.querySelectorAll(`[role=dialog] label`)).find(e=>e.textContent.startsWith("Harness")).querySelector("select").value')=='opencode'
    fill('Branch / feature name','preferred-new-feature')
    frames.capture('005-ordinary-default.png','An ordinary new feature initializes from the project OpenCode preference rather than the first supported harness.', ['New feature','Branch / feature name','Harness'],
        expression='document.querySelector(`[role=dialog] input.mono`).value === "preferred-new-feature"')
    select('Feature preset','0'); fill('Branch / feature name','preferred-new-feature')
    click('Reload harnesses','[role=dialog]')
    wait('!Array.from(document.querySelectorAll(`[role=dialog] button`)).find(b=>b.textContent.trim()==="Reload harnesses").disabled')
    assert frames.evaluate('Array.from(document.querySelectorAll(`[role=dialog] label`)).find(e=>e.textContent.startsWith("Harness")).querySelector("select").value')=='claude'
    frames.capture('006-explicit-preset.png','A deliberate Claude preset remains authoritative after refreshing project choices; the edited branch is retained.', ['Feature preset','New feature'],
        expression='document.querySelector(`[role=dialog] input.mono`).value === "preferred-new-feature" && document.querySelector(`[aria-label="Feature preset"]`).value === "0"')
    select('Harness','opencode'); select('Mode','supervibe'); click('Create feature','[role=dialog]')
    wait('!document.querySelector(`[role=dialog]`)')
    wait_for(lambda: query("SELECT agent FROM features WHERE branch='preferred-new-feature'")==[('opencode',)], 'Ordinary feature did not save OpenCode')
    action('wait',text='Opencode 1',scope='main')
    frames.capture('007-ordinary-created.png','The ordinary form launches its selected offline OpenCode feature while the original Claude panes remain untouched.', ['preferred-new-feature','Opencode 1'])
    assert rows()==initial_rows and original_panes()==initial_panes
    dismiss_toasts(); open_project()
    action('wait',expression='!!document.querySelector(`.todo-panel[aria-label="Project TODOs"] .todo-actions button`)')
    click('Start','.todo-panel[aria-label="Project TODOs"]')
    action('wait',text='Start work on a TODO',scope='[role=dialog]')
    select('Where','__new__'); action('click',selector='[role=dialog] button.btn-primary')
    wait('!!document.querySelector(`[role=dialog] [aria-label="Feature preset"]`)')
    wait('Array.from(document.querySelectorAll(`[role=dialog] label`)).find(e=>e.textContent.startsWith("Harness"))?.querySelector("select").value === "opencode"')
    click('Quick Plan','[role=dialog]')
    frames.capture('008-todo-default.png','TODO creation and planning initialize with the same project OpenCode default; selecting Quick Plan does not replace the chosen harness.', ['Plan in a new feature','Quick Plan','Prepare tax audit'])
    click('Start directly','[role=dialog]'); select('Mode','supervibe'); click('Create and start','[role=dialog]')
    wait('!document.querySelector(`[role=dialog]`)')
    wait('document.querySelector(`textarea[aria-label="Draft prompt"]`)?.value.includes("Prepare tax audit")')
    wait_for(lambda: query("SELECT agent FROM features WHERE branch='prepare-tax-audit'")==[('opencode',)], 'TODO feature did not use project default')
    assert query("SELECT status,agent_session_id IS NOT NULL FROM todos WHERE id='proof-todo'")==[('in_progress',1)]
    assert rows()==initial_rows and original_panes()==initial_panes and preference()=='opencode'
    assert not (scratch/'unexpected-prompt').exists()
    frames.capture('009-todo-created-draft.png','The TODO launches an offline OpenCode feature with an unsent editable prompt; the original sessions and project preference remain unchanged.', ['Compose prompt','Opencode 1'],
        expression='document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Prepare tax audit")')
    frames.write_notes()
    (out/'assertions.json').write_text(json.dumps({'frames':len(frames.notes),'separate_preference_save':True,'cancel_no_mutation':True,
        'external_sqlite_stale_refused':True,'existing_bindings_and_pids_preserved':True,'ordinary_default':True,
        'explicit_preset_survives_refresh':True,'ordinary_creation_opencode':True,'todo_planning_default':True,
        'todo_creation_opencode':True,'todo_prompt_unsent':True,'native_actions':'maintained amf-interact GUI driver'},indent=2)+'\n')
