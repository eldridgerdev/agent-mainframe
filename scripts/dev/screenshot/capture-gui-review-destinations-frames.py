#!/usr/bin/env python3
"""Assert explicit native review destinations over private Git/SQLite/tmux.

The production coordinator handles every UI action. Fixture-only external writes
exercise stale confirmations; no prompt is submitted to an offline receiver.
"""
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
spec = importlib.util.spec_from_file_location('amf_destination_driver', scripts.parent/'gui-driver/amf-gui-driver.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
out = pathlib.Path(sys.argv[1]); dbpath = pathlib.Path(sys.argv[4]); repo = pathlib.Path(sys.argv[5]); scratch = pathlib.Path(sys.argv[6])
source = scratch/'checkout'
config = pathlib.Path(os.environ['XDG_CONFIG_HOME'])/'amf/config.json'
dialog = '[role=dialog][aria-label="Feedback destination"]'
tmux = [os.environ['AMF_TMUX_BIN'],'-S',os.environ['AMF_TMUX_SOCKET']]
def query(sql, params=()):
    with sqlite3.connect(dbpath) as db: return db.execute(sql, params).fetchall()
def edit(sql, params=()):
    with sqlite3.connect(dbpath) as db:
        db.execute(sql, params)
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
def wait_for(condition, label):
    deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        if condition(): return
        time.sleep(.1)
    raise AssertionError(label)
def settings(**changes):
    value=json.loads(config.read_text()); value.update(changes); config.write_text(json.dumps(value))
def launch_count():
    path=scratch/'launches.jsonl'
    return len(path.read_text().splitlines()) if path.exists() else 0

with GuiFrames.from_argv() as frames:
    class InspectorAdapter:
        def evaluate(self, expression, deadline): return frames.evaluate(expression)
    driver=module.Driver(InspectorAdapter()); driver.inject(time.monotonic()+10)
    def action(op, **kwargs): return driver.run({'op':op,**kwargs})
    def click(name, scope=None): return action('click',name=name,**({'scope':scope} if scope else {}))
    def select(name,value): return action('select',name=name,value=value,scope=dialog)
    def wait(text): return action('wait',text=text)
    def dismiss_toasts(): frames.evaluate('Array.from(document.querySelectorAll(".toast button[aria-label]")).forEach(b=>b.click())')
    def open_picker():
        click('Choose feedback destination…')
        action('wait',expression='!!document.querySelector(`[aria-label="Feedback destination"] select`) && !document.querySelector(`[aria-label="Feedback destination"] select`).disabled')
    def preview():
        click('Preview destination',dialog)
        action('wait',expression='!!Array.from(document.querySelectorAll(`[aria-label="Feedback destination"] button`)).find(b=>b.textContent==="Prepare destination" && !b.disabled)')
    def prepare():
        click('Prepare destination',dialog)
        action('wait',expression='!document.querySelector(`[role=dialog][aria-label="Feedback destination"]`)')
    def choose_other(target):
        select('Destination type','existing')
        select('Agent session',json.dumps(target,separators=(',',':')))
    def capture(name,note,texts,**kwargs):
        dismiss_toasts(); frames.capture(name,note,texts,**kwargs)
    frames.window.configure(x=0,y=0,width=1280,height=1000)
    frames._x.sync()
    time.sleep(.2)
    action('wait',text='Invoice tests',scope='nav[aria-label=Workspace]')
    click('Invoice tests','nav[aria-label=Workspace]')
    action('wait',expression='!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    draft='Keep this unsent investigation. 🦀\nCheck negative invoice totals.'
    action('fill',name='Draft prompt',value=draft)
    click('Invoice rounding','nav[aria-label=Workspace]')
    wait('Final Review'); click('Final Review')
    action('wait',expression='!!document.querySelector(".diff-file")')
    click('Pre-finish summary'); wait('Complete review…')
    capture('001-current-destination.png','Final Review names its current agent destination and keeps completion and destination preparation separate.', ['Complete review','Feedback destination: Primary agent','Choose feedback destination…'])
    feedback=source/'.claude/final-review-feedback.md'
    original_progress=(source/'.claude/final-review-progress.json').read_bytes()
    assert not feedback.exists()

    # Change the global hook after GUI startup. The later catalog and preview
    # must show this current title/options, rather than the startup snapshot.
    current=json.loads(config.read_text())
    current['extension']['lifecycle_hooks']['on_worktree_created']['prompt']={'title':'Companion setup','options':['offline','full']}
    config.write_text(json.dumps(current))
    settings(low_memory_warn_mb=18446744073709551615)
    open_picker(); select('Destination type','dedicated'); select('Agent harness','opencode')
    click('Preview destination',dialog); wait('Proceed despite this resource warning')
    capture('002-resource-confirmation.png','A fresh dedicated OpenCode session requires explicit approval of the private fixture memory warning before any launch.', ['Invoice rounding / Final Review (Opencode)','resource warning','Proceed despite this resource warning','Creates and starts a dedicated Final Review session.'],allow_alert=True,
            expression='Array.from(document.querySelectorAll(`[aria-label="Feedback destination"] button`)).find(b=>b.textContent==="Prepare destination").disabled')
    before=launch_count(); assert len(query("SELECT id FROM feature_sessions WHERE label='Final Review'"))==0
    click('Proceed despite this resource warning',dialog)
    prepare(); wait('Feedback destination: Billing API / Invoice rounding / Final Review')
    wait_for(lambda: launch_count()>before,'Dedicated offline receiver did not start')
    assert len(query("SELECT id FROM feature_sessions WHERE feature_id='proof-feature' AND label='Final Review' AND kind='opencode'"))==1
    capture('003-dedicated-ready.png','Confirmed preparation creates one dedicated OpenCode session; the review and saved progress remain open and no feedback was submitted.', ['Feedback destination: Billing API / Invoice rounding / Final Review','Complete review…'])
    assert not feedback.exists() and not (scratch/'unexpected-prompt').exists()
    settings(low_memory_warn_mb=0)

    target={'project_id':'proof-project','feature_id':'proof-other','session_id':'proof-other-agent'}
    open_picker(); choose_other(target); preview()
    before=launch_count()
    edit("UPDATE feature_sessions SET claude_session_id='external-conversation' WHERE id='proof-other-agent'")
    click('Prepare destination',dialog)
    wait('changed. Refresh its preview')
    capture('004-stale-destination.png','An external session-row change refuses the delayed destination confirmation; the chosen fields remain visible and no receiver starts.', ['Preparation was refused','changed. Refresh its preview','Agent session'],allow_alert=True)
    assert launch_count()==before and not feedback.exists()
    preview(); prepare()
    wait('Feedback destination: Billing API / Invoice tests / Test agent')
    capture('005-other-checkout.png','An explicit existing target is identified by project, feature, session and checkout; feedback still belongs to the original reviewed source.', ['Feedback destination: Billing API / Invoice tests / Test agent','Destination checkout: '+str(scratch/'other-checkout')])

    open_picker(); select('Destination type','companion')
    action('fill',name='Companion branch',value='invoice-rounding-review-fixes',scope=dialog)
    select('Mode','supervibe'); select('Companion setup','offline'); preview()
    capture('006-companion-preview.png','After the global hook changes on disk, a fresh companion preview shows its current offline setup choice, exact branch HEAD and SuperVibe mode; uncommitted reviewed changes are explicitly excluded.', ['Branch basis: invoice-rounding at','Uncommitted reviewed changes are not copied','Creates a companion checkout and stopped feature'],
            expression='document.querySelector(`[aria-label="Feedback destination"] select`).value==="companion" && Array.from(document.querySelectorAll(`[aria-label="Feedback destination"] select`)).some(s=>s.value==="supervibe")')
    prepare(); wait('Worktree creation or its confirmed hook is running in the background.')
    wait_for(lambda:(scratch/'hook-choice').exists(),'Confirmed hook did not start')
    assert (scratch/'hook-choice').read_text()=='offline'
    companions=query("SELECT id,status,workdir FROM features WHERE branch='invoice-rounding-review-fixes'")
    assert len(companions)==1 and companions[0][1]=='stopped'
    before=launch_count()
    capture('007-background-hook.png','The confirmed worktree hook runs off the coordinator; its created companion is retained by stable identity and its pending launch can be cancelled.', ['Preparing New companion','running in the background','Cancel pending launch'])
    click('Cancel pending launch'); wait('Launch cancelled; waiting for setup to settle.')
    (scratch/'hook-release').touch()
    wait('launch was not completed')
    capture('008-cancelled-retained.png','Cancelling prevents the pending agent launch while preserving the created stopped companion and its source link for recovery.', ['Pending launch was cancelled','retained for recovery','Choose feedback destination…'],allow_alert=True)
    assert launch_count()==before
    assert query('SELECT status FROM features WHERE id=?',(companions[0][0],))[0][0]=='stopped'
    assert len(query("SELECT id FROM features WHERE branch='invoice-rounding-review-fixes'"))==1

    companion_session=query('SELECT id FROM feature_sessions WHERE feature_id=?',(companions[0][0],))[0][0]
    open_picker(); choose_other({'project_id':'proof-project','feature_id':companions[0][0],'session_id':companion_session}); preview(); prepare()
    wait('Feedback destination: Billing API / invoice-rounding-review-fixes / Final Review')
    capture('009-retained-exact-target.png','The retained stopped companion can be explicitly selected as an existing destination without recreating or launching it.', ['Feedback destination: Billing API / invoice-rounding-review-fixes / Final Review','Complete review…'])
    assert launch_count()==before
    assert len(query("SELECT id FROM features WHERE branch='invoice-rounding-review-fixes'"))==1
    open_picker(); choose_other(target); preview(); prepare()
    wait('Feedback destination: Billing API / Invoice tests / Test agent')
    click('Complete review…'); wait('Record this round')
    click('Complete and hand off to Billing API / Invoice tests / Test agent')
    action('wait',expression='!document.querySelector(".review-summary") && !!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    wait_for(lambda:feedback.exists(),'Review feedback was not saved')
    actual=frames.evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).value')
    assert actual.startswith(draft),actual
    assert str(source/'.claude/final-review-feedback.md') in actual,actual
    assert str(scratch/'other-checkout') in actual,actual
    assert not (source/'.claude/final-review-progress.json').exists()
    assert not (scratch/'unexpected-prompt').exists()
    assert launch_count()==before
    frames.evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).scrollTop=0')
    capture('010-source-aware-unsent.png','Completion appends source-aware feedback to the existing Unicode/multiline unsent draft in the selected other checkout; no input is sent even with TUI auto-submit configured.', ['Invoice tests','Test agent'],expression='document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("original feedback file")')
    # Scroll the real composer to expose the original path without editing its
    # value or applying presentation-only styles. The preceding frame proves
    # the retained draft at the start; this one proves the cross-checkout path.
    frames.evaluate('''(() => { const field=document.querySelector(`textarea[aria-label="Draft prompt"]`); field.scrollTop=6*parseFloat(getComputedStyle(field).lineHeight); })()''')
    capture('011-original-feedback-path.png','The scrolled unsent composer names the original reviewed checkout feedback file, rather than a different review in the destination checkout.', ['Invoice tests','Test agent'],expression='document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes('+json.dumps(str(feedback))+')')
    (out/'destination-proof.json').write_text(json.dumps({'source_feedback':str(feedback),'existing_draft_preserved':True,'gui_auto_submit_ignored':True,'no_receiver_input':True,'companion_retained_stopped':True,'single_companion':True,'hook_choice':'offline','post_startup_global_hook_refreshed':True},indent=2)+'\n')
    frames.write_notes()
