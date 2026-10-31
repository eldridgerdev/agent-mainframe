#!/usr/bin/env python3
"""Exercise a private PR Triage companion integration through native IPC."""
import importlib.util
import pathlib
import subprocess
import sys
import time
from gui_frames import GuiFrames
scripts=pathlib.Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('driver',scripts.parent/'gui-driver/amf-gui-driver.py'); module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
out=pathlib.Path(sys.argv[1]); scratch=pathlib.Path(sys.argv[6]); source=scratch/'source'; companion=scratch/'companion'; remote=scratch/'origin.git'
def git(path,*args): return subprocess.check_output(['git','-C',str(path),*args],text=True).strip()
original=git(source,'rev-parse','HEAD'); companion_head=git(companion,'rev-parse','HEAD'); pr_tip=git(remote,'rev-parse','refs/heads/invoice-pr-head')
with GuiFrames.from_argv() as frames:
    class Inspector:
        def evaluate(self,expression,deadline): return frames.evaluate(expression)
    driver=module.Driver(Inspector()); driver.inject(time.monotonic()+10)
    def action(op,**kwargs):
        try: return driver.run({'op':op,**kwargs})
        except Exception:
            (out/'failed-state.txt').write_text('Action: '+repr((op,kwargs))+'\n'+frames.evaluate('document.body.innerText')); raise
    def click(name,scope=None): return action('click',name=name,**({'scope':scope} if scope else {}))
    def wait(text): return action('wait',text=text)
    def capture(name,note,texts,**kwargs): frames.capture(name,note,texts,**kwargs); frames.write_notes()
    frames.window.configure(x=0,y=0,width=1500,height=1000);frames._x.sync();time.sleep(.2)
    action('wait',text='Invoice fixes',scope='nav[aria-label=Workspace]');click('Invoice fixes','nav[aria-label=Workspace]')
    wait('Integrate PR #42 fixes…');click('Integrate PR #42 fixes…');click('Preview integration')
    wait('companion checkout has uncommitted changes')
    capture('001-preview.png','The PR Triage companion preview names PR 42, the PR branch, the differing source branch, the exact commit and excluded uncommitted companion changes before any integration.', ['PR Triage companion for','not the PR branch',companion_head,'uncommitted changes'],expression='!!document.querySelector(`[aria-label="Integration preview"]`)')
    action('click',selector='[role=dialog] .modal-footer button:first-child');assert git(source,'rev-parse','HEAD')==original
    click('Integrate PR #42 fixes…');click('Preview integration');wait('companion checkout has uncommitted changes')
    (source/'new-untracked.txt').write_text('Preserve this change\n')
    click('Confirm cherry-pick');wait('Integration changed; refresh the preview')
    capture('002-stale-refusal.png','A source edit after preview refuses the operation and leaves the new file untouched.', ['Integration changed; refresh the preview'],allow_alert=True)
    assert git(source,'rev-parse','HEAD')==original and (source/'new-untracked.txt').read_text()=='Preserve this change\n'
    (source/'new-untracked.txt').unlink()
    # Push is previewed and confirmed against the private bare remote only.
    action('select',name='Integration method',value='push')
    click('Preview integration');wait('Origin branch tip')
    capture('003-push-preview.png','The push preview shows the private origin PR branch tip that the fast-forward update is pinned to.', ['Origin branch tip',pr_tip,'invoice-pr-head'],expression='!!document.querySelector(`[aria-label="Integration preview"]`)')
    assert git(remote,'rev-parse','refs/heads/invoice-pr-head')==pr_tip
    click('Confirm push');wait('Pushed 1 companion commit')
    capture('004-push-completed.png','The confirmed push advanced only the private origin PR branch to the companion head.', ['for PR #42','invoice-pr-head'])
    assert git(remote,'rev-parse','refs/heads/invoice-pr-head')==companion_head
    assert git(source,'rev-parse','HEAD')==original and not (scratch/'launches.jsonl').exists()
