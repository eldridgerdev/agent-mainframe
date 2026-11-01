#!/usr/bin/env python3
"""Exercise keep-as-TODO and AI finding skip/edit in the native PR Workspace, offline."""
import importlib.util
import json
import pathlib
import sqlite3
import sys
import time
from gui_frames import GuiFrames

scripts = pathlib.Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('driver', scripts.parent/'gui-driver/amf-gui-driver.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
out, dbpath, scratch = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[4]), pathlib.Path(sys.argv[6])
with GuiFrames.from_argv() as frames:
    class Inspector:
        def evaluate(self, expression, deadline): return frames.evaluate(expression)
    driver = module.Driver(Inspector()); driver.inject(time.monotonic()+10)
    def fail(label, detail):
        (out/'failed-state.txt').write_text(label+': '+repr(detail)+'\n'+frames.evaluate('document.body.innerText'))
    def action(op, **kwargs):
        try: return driver.run({'op':op, **kwargs})
        except Exception:
            fail('Action', (op, kwargs)); raise
    def click(name, scope=None): return action('click', name=name, **({'scope':scope} if scope else {}))
    def wait(text): return action('wait', text=text)
    def capture(name, note, texts, **kwargs):
        frames.capture(name, note, texts, **kwargs)
        frames.write_notes()
    def todos():
        with sqlite3.connect(dbpath) as db: return db.execute('SELECT title, body FROM todos').fetchall()
    def cached_findings():
        with sqlite3.connect(dbpath) as db:
            return json.loads(db.execute('SELECT json FROM ai_review_cache WHERE pr_number=12').fetchone()[0])['findings']
    frames.window.configure(x=0,y=0,width=1500,height=1000); frames._x.sync(); time.sleep(.2)
    action('wait',text='Invoice rounding',scope='nav[aria-label=Workspace]')
    click('Invoice rounding','nav[aria-label=Workspace]')
    action('wait',text='Test agent')
    click('PR Workspace (experimental)')
    action('wait',expression='!!Array.from(document.querySelectorAll(`.pr-entry`)).find(e=>e.textContent.includes(`Round invoice totals to cents`))')
    frames.evaluate('Array.from(document.querySelectorAll(`.pr-entry`)).find(e=>e.textContent.includes(`Round invoice totals to cents`)).click()')
    wait('3 open of 4')
    frames.evaluate('Array.from(document.querySelectorAll(".pr-comment")).find(b=>b.textContent.includes("Does this round negative totals")).click()')
    wait('Investigation (read-only)')
    assert todos() == []
    frames.evaluate('document.querySelector("[aria-label=Investigation]").scrollIntoView({block:"center"})'); time.sleep(.3)
    capture('001-investigation-keep.png','A finished investigation on a review thread offers Keep as a TODO. No TODO exists yet.',['Investigation (read-only)','Keep as a TODO'])
    click('Keep as a TODO')
    action('wait',expression='document.body.innerText.includes("Kept as a TODO on")')
    rows = todos()
    assert len(rows) == 1 and 'AMF source: PR #12 comment 7001 investigation (answer ' in rows[0][1], rows
    assert not frames.evaluate('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Keep as a TODO")')
    frames.evaluate('document.querySelector("[aria-label=Investigation]").scrollIntoView({block:"center"})'); time.sleep(.3)
    capture('002-kept-as-todo.png','Keeping writes one TODO to the feature worktree list, tagged with its PR, comment and answer. The button is replaced by a kept marker, so a repeat cannot add a second item.',['Kept as a TODO'])
    action('wait',expression='!!Array.from(document.querySelectorAll(`button`)).find(b=>b.textContent.trim()===`AI Review` && !b.disabled)')
    click('AI Review'); wait('Handle negative totals correctly.')
    capture('003-findings-skip-edit.png','Each unposted AI finding has Skip and Edit. Nothing has changed yet.',['Handle negative totals correctly.','Add a regression test.'],expression='!!document.querySelector(`[aria-label="Skip finding 1"]`) && !!document.querySelector(`[aria-label="Edit finding 1"]`)')
    click('Edit finding 1')
    action('wait',expression='!!document.querySelector(`textarea[aria-label="Finding 1 body"]`)')
    action('fill', name='Finding 1 body', value='Handle negative totals: round the magnitude, then restore the sign.')
    capture('004-edit-body.png','Editing a finding shows its length limit and states that nothing is sent until posting.',['Save body','characters. This is the text that would be posted'],expression='document.querySelector(`textarea[aria-label="Finding 1 body"]`).value.startsWith("Handle negative totals: round")')
    click('Save body'); wait('round the magnitude, then restore the sign.')
    assert cached_findings()[0]['body'].startswith('Handle negative totals: round the magnitude')
    click('Skip finding 2'); action('wait',expression='document.body.innerText.includes("Skipped")')
    assert cached_findings()[1]['skipped'] is True
    click('Post to GitHub…')
    action('wait',expression='!!document.querySelector(`[role=alertdialog]`)')
    capture('005-skipped-left-out-of-post.png','The edited text is what the post confirmation lists, and the skipped finding is absent from it. This confirmation is cancelled; nothing is posted.',['Post 1 finding to PR #12','round the magnitude'],expression='!document.querySelector(`[role=alertdialog]`).innerText.includes("Add a regression test.")')
    click('Cancel','[role=alertdialog]')
    click('Restore finding 2'); action('wait',expression='!!document.querySelector(`[aria-label="Skip finding 2"]`)')
    assert cached_findings()[1]['skipped'] is False
    capture('006-restored.png','Restore returns the skipped finding to the postable set; the edited body is kept.',['Add a regression test.','round the magnitude'],expression='!!document.querySelector(`[aria-label="Skip finding 2"]`)')
    assert not (scratch/'gh-state/writes.jsonl').read_text()
    assert not (scratch/'unexpected-prompt').exists()
    assert len(todos()) == 1
