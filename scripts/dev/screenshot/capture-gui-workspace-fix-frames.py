#!/usr/bin/env python3
"""Exercise authoritative PR Workspace fix handoffs with offline GH and receivers."""
import importlib.util
import json
import os
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
    def action(op, **kwargs):
        try: return driver.run({'op':op, **kwargs})
        except Exception:
            (out/'failed-state.txt').write_text('Action: '+repr((op,kwargs))+'\n'+frames.evaluate('document.body.innerText'))
            raise
    def click(name, scope=None): return action('click', name=name, **({'scope':scope} if scope else {}))
    def wait(text):
        try: return action('wait', text=text)
        except Exception:
            (out/'failed-state.txt').write_text('Waiting for: '+text+'\n'+frames.evaluate('document.body.innerText'))
            raise
    def capture(name, note, texts, **kwargs):
        frames.capture(name, note, texts, **kwargs)
        frames.write_notes()
    frames.window.configure(x=0,y=0,width=1500,height=1000); frames._x.sync(); time.sleep(.2)
    action('wait',text='Invoice rounding',scope='nav[aria-label=Workspace]')
    click('Invoice rounding','nav[aria-label=Workspace]')
    action('wait',text='Test agent')
    click('Test agent')
    action('wait',expression='!!document.querySelector(`textarea[aria-label="Draft prompt"]`)')
    action('fill', name='Draft prompt', value='Existing unsent draft. 🦀\nKeep this context.')
    click('PR Workspace (experimental)')
    action('wait',text='Round invoice totals to cents')
    action('wait',expression='!!Array.from(document.querySelectorAll(`.pr-entry`)).find(e=>e.textContent.includes(`Round invoice totals to cents`))')
    frames.evaluate('Array.from(document.querySelectorAll(`.pr-entry`)).find(e=>e.textContent.includes(`Round invoice totals to cents`)).click()')
    wait('3 open of 4')
    action('wait',expression='!!Array.from(document.querySelectorAll(`button`)).find(b=>b.textContent.trim()===`AI Review` && !b.disabled)')
    click('AI Review'); wait('Handle negative totals correctly.')
    capture('001-findings.png','Seeded offline AI findings require an explicitly selected agent before preparing a fix.', ['Handle negative totals correctly.','Add a regression test.','Fix agent'], expression='document.querySelector(`.pr-ai-batch-toolbar select`).value==="" && Array.from(document.querySelectorAll(`.pr-ai-finding button`)).every(b=>b.disabled)')
    action('select',name='Fix agent',value='proof-primary',scope='[aria-label="AI PR review"]')
    (scratch/'gh-state/head.txt').write_text('b'*40+'\n')
    click('Prepare fix', '.pr-ai-finding:first-child'); wait('The PR changed; refresh before preparing a fix')
    capture('002-moved-head-refusal.png','A moved remote PR head refuses the handoff without opening a composer.', ['The PR changed; refresh before preparing a fix','Handle negative totals correctly.'],allow_alert=True)
    (scratch/'gh-state/head.txt').write_text('c0ffee'+'0'*34+'\n')
    with sqlite3.connect(dbpath) as db:
        db.execute("DELETE FROM feature_sessions WHERE id='proof-primary'")
        db.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
    click('Refresh agents','[aria-label="AI PR review"]'); action('wait',expression='document.querySelector(`.pr-ai-batch-toolbar select`)?.selectedOptions[0]?.textContent.includes(`Previously selected session unavailable`)')
    capture('003-unavailable-agent.png','Refreshing keeps the missing selected agent visible and does not silently select the other live session.', ['Fix agent','Handle negative totals correctly.'],allow_alert=True,expression='document.querySelector(`.pr-ai-batch-toolbar select`).value==="proof-primary" && document.querySelector(`.pr-ai-batch-toolbar select`).selectedOptions[0].textContent.includes("unavailable")')
    action('select',name='Fix agent',value='proof-other-agent',scope='[aria-label="AI PR review"]')
    click('Select all','[aria-label="AI PR review"]'); click('Prepare batch fix (2)','[aria-label="AI PR review"]')
    wait('Compose prompt')
    action('wait',expression='!!document.querySelector(`textarea[aria-label="Draft prompt"]`) && document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Add a regression test.")')
    capture('004-unsent-handoff.png','The selected agent receives the two authoritative findings appended to the existing unsent composer draft.', ['Compose prompt','Send prompt'], expression='document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Existing unsent draft. 🦀") && document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Handle negative totals correctly.") && document.querySelector(`textarea[aria-label="Draft prompt"]`).value.includes("Add a regression test.")')
    assert not (scratch/'unexpected-prompt').exists()
    assert not (scratch/'gh-state/writes.jsonl').read_text()
    assert not (scratch/'launches.jsonl').exists() or len((scratch/'launches.jsonl').read_text().splitlines()) == 2
