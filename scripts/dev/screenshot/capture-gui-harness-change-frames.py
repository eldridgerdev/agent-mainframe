#!/usr/bin/env python3
"""Native controls use the maintained AMF interaction driver; assertions inspect private DB/tmux.

The driver attaches to the runner's existing private webview (HOME preserved).
GuiFrames owns the PID-scoped X11 capture and WebKit transport.
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
spec = importlib.util.spec_from_file_location('amf_native_driver', scripts.parent/'gui-driver/amf-gui-driver.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)

out = pathlib.Path(sys.argv[1]); dbpath = pathlib.Path(sys.argv[4]); repo = pathlib.Path(sys.argv[5]); scratch = pathlib.Path(sys.argv[6])
tmux = [os.environ['AMF_TMUX_BIN'],'-S',os.environ['AMF_TMUX_SOCKET']]
def rows():
    with sqlite3.connect(dbpath) as db:
        return db.execute('SELECT id,kind,label,tmux_window,claude_session_id,stopped,command,on_stop,token_usage_source,token_usage_source_match FROM feature_sessions WHERE feature_id=? ORDER BY sort_order',('proof-feature',)).fetchall()
def state():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT agent,status FROM features WHERE id='proof-feature'").fetchone()
def panes():
    return subprocess.check_output(tmux+['list-panes','-a','-F','#{session_name}:#{window_name}:#{pane_pid}'],text=True).splitlines()
def feature_panes():
    return sorted(line for line in panes() if line.startswith('amf-harness-proof:'))
def wait_for(condition, label):
    for _ in range(150):
        if condition(): return
        time.sleep(.1)
    raise AssertionError(label)

with GuiFrames.from_argv() as frames:
    class InspectorAdapter:
        def evaluate(self, expression, deadline): return frames.evaluate(expression)
    driver = module.Driver(InspectorAdapter())
    driver.inject(time.monotonic()+10)
    def action(op, **kwargs): return driver.run({'op':op, **kwargs})
    def batch(*actions): return driver.run({'op':'batch','actions':list(actions)})
    def click(name, scope=None): return action('click',name=name, **({'scope':scope} if scope else {}))
    def choose(value): return action('select',name='Replacement harness',value=value,scope='[role=dialog]')
    def consent(): return click('I understand and confirm this harness change','[role=dialog]')
    def open_dialog():
        batch({'op':'click','name':'More feature actions'},{'op':'click','name':'Change harness…'},
              {'op':'wait','expression':'!!document.querySelector("[role=dialog] select") && !document.querySelector("[role=dialog] select").disabled'})
    def close_dialog(): click('Cancel','[role=dialog]')
    def dismiss_toasts():
        # Precisely scoped cleanup only; normal UI actions use driver controls.
        frames.evaluate('Array.from(document.querySelectorAll(".toast button[aria-label] ")).forEach(b=>b.click())')
    frames.window.configure(width=1180,height=900)
    action('wait',text='Invoice rounding',scope='nav[aria-label=Workspace]')
    click('Invoice rounding','nav[aria-label=Workspace]')
    action('wait',expression='!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    initial_rows = rows(); initial_panes = feature_panes()
    assert all(row[8] is not None and row[9]=="exact" for row in initial_rows[:2])
    frames.capture('001-running-claude.png','Running offline Claude worktree with one individually stopped agent, a terminal and a custom watcher.', ['Invoice rounding','Primary agent','Paused agent','Terminal','Watcher'])
    action('fill',name='Draft prompt',value='Keep this unsent invoice investigation.')
    open_dialog(); choose('opencode'); consent()
    frames.capture('002-draft-guard.png','An unsent agent draft blocks harness replacement; selecting and confirming does not mutate.', ['unsent agent drafts','All 2 agent session rows','Previously runnable sessions'], allow_alert=True,
                   expression='Array.from(document.querySelectorAll("[role=dialog] button")).find(b=>b.textContent==="Change harness").disabled')
    assert rows()==initial_rows and feature_panes()==initial_panes
    close_dialog()
    assert frames.evaluate('document.querySelector(`textarea[aria-label="Draft prompt"]`).value')=='Keep this unsent invoice investigation.'
    action('fill',name='Draft prompt',value='')
    open_dialog(); choose('opencode'); consent()
    frames.capture('003-confirm-running-switch.png','Explicit replacement confirmation describes all agent rows, cleared conversations, stop/restart and individually stopped choices.', ['All 2 agent session rows','conversation IDs','all its sessions will stop','individually stopped sessions remain stopped'])
    close_dialog(); assert rows()==initial_rows and feature_panes()==initial_panes
    open_dialog(); choose('opencode'); consent()
    # A second AMF/SQLite writer changes a persisted row while confirmation is
    # open. The command must refresh before any stop or hook mutation.
    with sqlite3.connect(dbpath) as db:
        db.execute("UPDATE feature_sessions SET claude_session_id='external-conversation' WHERE id='proof-0'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    click('Change harness','[role=dialog]')
    action('wait',text='changed. Reload and confirm again',scope='[role=dialog]')
    frames.capture('004-stale-refusal.png','A real external SQLite change invalidates the open confirmation before stopping any session.', ['changed. Reload and confirm again','nothing was stopped'],allow_alert=True)
    assert feature_panes()==initial_panes and state()[0]=='claude'
    click('Reload','[role=dialog]'); action('wait',expression='!!document.querySelector("[role=dialog] select")'); choose('opencode'); consent()
    click('Change harness','[role=dialog]')
    wait_for(lambda: state()==('opencode','idle'),'OpenCode replacement did not finish')
    action('wait',expression='!document.querySelector("[role=dialog]")')
    after = rows(); new_panes = feature_panes()
    assert [row[0] for row in after]==[row[0] for row in initial_rows]
    assert [row[1] for row in after[:2]]==['opencode','opencode'] and all(row[4] is None for row in after[:2])
    assert all(row[8] is None and row[9] is None for row in after[:2])
    assert after[0][5]==0 and after[1][5]==1 and after[2:]==initial_rows[2:]
    assert any(':opencode:' in pane for pane in new_panes) and not any(':opencode-2:' in pane for pane in new_panes)
    assert all(new.split(':')[-1] not in [old.split(':')[-1] for old in initial_panes] for new in new_panes)
    wait_for(lambda: (scratch/'custom-stopped').exists(),'Custom on_stop was not invoked')
    assert (scratch/'checkout/.opencode/plugins/sidebar-state.js').is_file()
    frames.capture('005-open-code-restarted.png','All agent bindings switched and conversations cleared; runnable sessions restarted with new PIDs and the paused agent stayed stopped.', ['Invoice rounding','Opencode 1','Opencode 2','Terminal','Watcher'])
    # Feature-level Stop is an ordinary user action. The subsequent conversion
    # must update bindings/hooks without launching any replacement processes.
    dismiss_toasts(); click('Stop')
    wait_for(lambda: state()[1]=='stopped','Feature did not stop')
    wait_for(lambda: not feature_panes(),'Feature panes remained after Stop')
    launch_count = len((scratch/'launches.jsonl').read_text().splitlines())
    open_dialog(); choose('claude'); consent()
    frames.capture('006-confirm-stopped-switch.png','Stopped-feature confirmation explicitly says no sessions launch.', ['feature will remain stopped','does not launch any sessions'])
    click('Change harness','[role=dialog]')
    wait_for(lambda: state()==('claude','stopped'),'Stopped Claude conversion did not save')
    action('wait',expression='!document.querySelector("[role=dialog]")')
    assert not feature_panes()
    assert len((scratch/'launches.jsonl').read_text().splitlines()) == launch_count
    assert not (scratch/'unexpected-prompt').exists()
    final = rows(); assert all(row[1]=='claude' and row[4] is None for row in final[:2])
    assert final[1][5]==1 and final[2:]==initial_rows[2:]
    assert not (scratch/'checkout/.opencode/plugins/sidebar-state.js').exists()
    assert (scratch/'checkout/.claude/settings.local.json').is_file()
    frames.capture('007-stopped-claude-saved.png','Stopped conversion saved Claude hooks and bindings without launching; the individually stopped choice and nonagent rows remain.', ['Invoice rounding','Claude 1','Claude 2','Terminal','Watcher'])
    frames.write_notes()
    (out/'assertions.json').write_text(json.dumps({'frames':len(frames.notes),'cancel_no_mutation':True,'draft_preserved':True,'external_sqlite_stale_refused':True,
        'all_agent_bindings_changed':True,'conversation_ids_cleared':True,'token_bindings_cleared':True,'stopped_flag_preserved':True,'nonagent_definitions_preserved':True,'custom_on_stop_invoked':True,
        'running_pids_replaced':True,'stopped_change_no_launch':True,'native_actions':'maintained amf-interact GUI driver'},indent=2)+'\n')
