#!/usr/bin/env python3
"""Real Claude waiting hook plus restricted offline explanation receiver."""
import importlib.util
import json
import os
import pathlib
import signal
import sqlite3
import subprocess
import sys
import time
from gui_frames import GuiFrames
from owned_process import stop_owned_process_group

scripts = pathlib.Path(__file__).resolve().parent
workspace = scripts.parents[2]
spec = importlib.util.spec_from_file_location('amf_native_driver', scripts.parent/'gui-driver/amf-gui-driver.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
out = pathlib.Path(sys.argv[1]); dbpath = pathlib.Path(sys.argv[4]); repo = pathlib.Path(sys.argv[5]); scratch = pathlib.Path(sys.argv[6]); host = scratch/'checkout'
tmux = [os.environ['AMF_TMUX_BIN'], '-S', os.environ['AMF_TMUX_SOCKET']]
def rows():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT id,kind,label,tmux_window,claude_session_id,stopped,command,on_stop,token_usage_source,token_usage_source_match FROM feature_sessions WHERE feature_id='proof-feature' ORDER BY sort_order").fetchall()
def panes():
    return sorted(line for line in subprocess.check_output(tmux+['list-panes','-a','-F','#{session_name}:#{window_name}:#{pane_pid}'],text=True).splitlines() if line.startswith('amf-harness-proof:'))
def launched():
    path = scratch/'headless.jsonl'
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
def wait_for(predicate, description):
    deadline = time.monotonic()+15
    while not predicate():
        assert time.monotonic()<deadline, description
        time.sleep(.05)
def alive(pid):
    try: os.kill(pid, 0); return True
    except ProcessLookupError: return False

original = (host/'invoice.ts').read_bytes()
(repo/'.git/info/exclude').open('a').write('\n.claude/\n')
notes = host/'.claude/review-notes.md'; notes.parent.mkdir(exist_ok=True)
notification_dir = host/'.claude/notifications'
hook = None
try:
    with GuiFrames.from_argv() as frames:
        class InspectorAdapter:
            def evaluate(self, expression, deadline): return frames.evaluate(expression)
        driver = module.Driver(InspectorAdapter()); driver.inject(time.monotonic()+10)
        def action(op, **kwargs):
            try: return driver.run({'op':op, **kwargs})
            except BaseException:
                (out/'last-state.txt').write_text(frames.evaluate('document.body.innerText'))
                (out/'headless-diagnostics.json').write_text(json.dumps(launched(),indent=2)+'\n')
                raise
        def click(name, scope=None):
            # Text can paint before the preceding IPC clears its busy state.
            # Wait for the actual control instead of retrying a failed click.
            root = 'document.querySelector('+json.dumps(scope)+')' if scope else 'document'
            action('wait',expression='Array.from(('+root+' ?? document.createElement("div")).querySelectorAll("button")).some(button => button.textContent.trim() === '+json.dumps(name)+' && !button.disabled)')
            return action('click',name=name,**({'scope':scope} if scope else {}))
        def wait(expression): return action('wait',expression=expression)
        popup = '[role=dialog][aria-label="Supervised edits"]'
        def capture(name, caption, expected, allow_alert=False):
            wait('document.querySelector(`textarea[aria-label="Feedback for the agent"]`)?.value === "Keep this unsent rejection draft — café."')
            frames.capture(name,caption,expected,allow_alert=allow_alert)
            assert rows()==initial_rows and panes()==initial_panes, 'Explanation changed original session bindings or pane ownership'
            assert (host/'invoice.ts').read_bytes()==original
            assert hook.poll() is None, 'Explaining answered the waiting hook'
            assert not pathlib.Path(request['proceed_signal']).exists() and not pathlib.Path(request['response_file']).exists()
        frames.resize(1420,980)
        action('wait',text='Invoice rounding',scope='nav[aria-label=Workspace]')
        action('click',selector='nav[aria-label=Workspace] [aria-describedby="tree-feature-proof-feature"]')
        action('wait',text='Invoice rounding',scope='main')
        initial_rows, initial_panes = rows(), panes()
        hook = subprocess.Popen(['bash',str(workspace/'plugins/diff-review/scripts/custom-diff-review.sh'),str(host)],
            cwd=host,env=os.environ|{'AMF_ACTIVE':'1','AMF_SESSION':'amf-harness-proof'},stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,start_new_session=True)
        hook.stdin.write(json.dumps({'tool_name':'Edit','session_id':'explanation-'+scratch.name,'cwd':str(host),'tool_input':{
            'file_path':str(host/'invoice.ts'),'old_string':'return subtotal * 0.2;', 'new_string':'return Math.max(0, subtotal * 0.2);'}})); hook.stdin.close()
        wait_for(lambda: notification_dir.exists() and bool(list(notification_dir.glob('*.json'))), 'Real hook did not publish its waiting notification')
        notification = next(notification_dir.glob('*.json')); request = json.loads(notification.read_text())
        wait('!!document.querySelector('+json.dumps(popup)+')')
        wait('!!document.querySelector(`textarea[aria-label="Feedback for the agent"]`)')
        action('fill',name='Feedback for the agent',value='Keep this unsent rejection draft — café.',scope=popup)
        capture('001-waiting-draft.png','The real Claude hook is still waiting; the reviewer has an unsent rejection draft before requesting an explanation.', ['Supervised edits','invoice.ts','Feedback for the agent','Explain this edit'])
        notes.write_text('## invoice.ts\n\nSaved rationale: credits must not generate negative tax.\n')
        click('Explain this edit',popup)
        action('wait',text='Saved rationale: credits must not generate negative tax.',scope=popup)
        assert launched()==[]
        capture('002-saved-note-no-call.png','Saved review rationale is shown without a pre-call notice or headless process; the rejection draft and waiting hook are unchanged.', ['Saved review note','Saved rationale: credits must not generate negative tax.'])
        notes.unlink()
        click('Explain this edit',popup)
        action('wait',text='Call Claude to explain this edit?',scope=popup)
        action('click',selector='[aria-label="Explanation AI call"] summary',scope=popup)
        preview = frames.evaluate('document.querySelector(`[aria-label="Explanation AI call"] details pre`).textContent')
        assert 'return subtotal * 0.2;' in preview and 'Math.max(0, subtotal * 0.2)' in preview
        assert launched()==[]
        capture('003-mandatory-preview.png','Explain stages a mandatory, exact prompt preview with the configured model and reasoning; no process starts until Continue.', ['Call Claude to explain this edit?','offline-review-model','high','Exact prompt','Continue explanation'])
        click('Cancel explanation',popup); action('wait',text='Explanation cancelled before starting an AI call.',scope=popup); assert launched()==[]
        capture('004-notice-cancelled.png','Cancelling the notice launches nothing and retains the unsent feedback and waiting edit.', ['Explanation cancelled before starting an AI call.','Feedback for the agent','Reject edit'])
        click('Explain this edit',popup); action('wait',text='Continue explanation',scope=popup); click('Continue explanation',popup)
        action('wait',text='This change prevents negative invoice subtotals from producing negative tax.',scope=popup)
        assert len(launched())==1 and launched()[0]['prompt']==preview
        capture('005-explanation-result.png','The restricted offline receiver consumed the exact approved prompt and configured model/effort, then returned a separate AI explanation without answering the edit.', ['AI explanation','This change prevents negative invoice subtotals from producing negative tax.'])
        (scratch/'hold-headless').touch(); click('Explain this edit',popup); action('wait',text='Continue explanation',scope=popup); click('Continue explanation',popup)
        wait_for(lambda: len(launched())==2,'Second explanation did not start'); running_pid = launched()[-1]['pid']
        action('wait',text='Explaining this edit…',scope=popup)
        capture('006-running-cancellable.png','A second restricted explanation runs independently while the reviewer can retain and edit the rejection draft; Cancel affects only this owned call.', ['Explaining this edit…','Cancel explanation','Feedback for the agent'])
        click('Cancel explanation',popup); wait_for(lambda: not alive(running_pid),'Cancelled explanation process was not reaped')
        (scratch/'hold-headless').unlink()
        action('wait',text='Explanation call cancelled. The edit is still waiting.',scope=popup)
        capture('007-call-cancelled.png','Cancel acknowledges the matching worker and its process is reaped; the original interactive panes, hook and unsent feedback remain untouched.', ['Explanation call cancelled. The edit is still waiting.','Feedback for the agent','Reject edit'])
        click('Explain this edit',popup); action('wait',text='Continue explanation',scope=popup)
        # Prompt resolution uses the feature worktree's Git root, so mutate the
        # effective config there rather than the separate project checkout.
        project_config = host/'amf.json'; original_config = project_config.read_text()
        config = json.loads(original_config); config['prompt_overrides']['review.diff_explain']['template']+='\nChanged externally.'; project_config.write_text(json.dumps(config))
        click('Continue explanation',popup)
        action('wait',text='The edit, checkout or explanation settings changed.',scope=popup)
        assert len(launched())==2
        capture('008-stale-confirmation-refused.png','An external prompt edit invalidates the prepared notice. Continue refuses the stale approval and starts no replacement call.', ['explanation settings changed','Feedback for the agent'],allow_alert=True)
        project_config.write_text(original_config)
        click('Cancel explanation',popup)
        click('Explain this edit',popup); action('wait',text='Continue explanation',scope=popup)
        (scratch/'hold-headless').touch(); click('Continue explanation',popup)
        wait_for(lambda: len(launched())==3,'Third explanation did not start'); stale_pid = launched()[-1]['pid']
        with sqlite3.connect(dbpath) as db:
            db.execute("UPDATE features SET status='stopped' WHERE id='proof-feature'")
            db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
        action('wait',text='The waiting edit or checkout changed.',scope=popup)
        wait_for(lambda: not alive(stale_pid),'Stale explanation process was not reaped')
        capture('009-running-stale-discarded.png','An external SQLite stopped-state update invalidates the running explanation. Its output is discarded, its process is reaped, and the unsent rejection draft remains visible.', ['The waiting edit or checkout changed.','Feedback for the agent'],allow_alert=True)
        action('click',selector='footer button',scope=popup)
        action('wait',text='Discard unsent feedback',scope=popup)
        capture('010-draft-close-guard.png','Closing after explanation cancellation still asks before discarding unsent feedback; the hook remains waiting and no decision has been sent.', ['Discard unsent feedback','Keep editing','Discard and close'],allow_alert=True)
        click('Keep editing',popup)
        assert not (scratch/'unexpected-prompt').exists()
        frames.write_notes()
        (out/'assertions.json').write_text(json.dumps({'frames':10,'real_claude_waiting_hook':True,'saved_note_zero_call':True,'mandatory_precall_cancel_zero_call':True,'configured_model_effort_exact_prompt':True,'restricted_runner_capabilities':True,'owned_cancel_and_reap':True,'stale_precall_refusal':True,'running_stale_discard':True,'unsent_feedback_and_hook_preserved':True,'original_rows_and_pane_pids_preserved':True,'no_paid_harnesses':True},indent=2)+'\n')
finally:
    stop_owned_process_group(hook)
