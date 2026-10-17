#!/usr/bin/env python3
"""Real GUI IPC, shared grammar installer/cc/parser load, and retained Learning draft."""
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
root = scratch/'config/amf/tree-sitter'; source = root/'sources/json'; library = root/'parsers/json.so'
tmux = [os.environ['AMF_TMUX_BIN'], '-S', os.environ['AMF_TMUX_SOCKET']]
def rows():
    with sqlite3.connect(dbpath) as db:
        return db.execute("SELECT id,kind,label,tmux_window,claude_session_id,stopped,command,on_stop,token_usage_source,token_usage_source_match FROM feature_sessions WHERE feature_id='proof-feature' ORDER BY sort_order").fetchall()
def panes():
    return sorted(line for line in subprocess.check_output(tmux+['list-panes','-a','-F','#{session_name}:#{window_name}:#{pane_pid}'],text=True).splitlines() if line.startswith('amf-harness-proof:'))
def replace_library(content):
    library.parent.mkdir(parents=True,exist_ok=True)
    replacement = library.with_suffix('.replacement'); replacement.write_bytes(content); replacement.replace(library)
def wait_file(path):
    for _ in range(200):
        if path.exists(): return
        time.sleep(.1)
    raise AssertionError(f'Missing fixture gate: {path}')

with GuiFrames.from_argv() as frames:
    class InspectorAdapter:
        def evaluate(self, expression, deadline): return frames.evaluate(expression)
    driver = module.Driver(InspectorAdapter()); driver.inject(time.monotonic()+10)
    def action(op, **kwargs): return driver.run({'op':op, **kwargs})
    def click(name,scope=None): return action('click',name=name,**({'scope':scope} if scope else {}))
    def wait(expression): return action('wait',expression=expression)
    def manager(): click('Syntax languages'); action('wait',text='JSON',scope='[role=dialog][aria-label="Syntax languages"]')
    def state_is(name):
        wait('Array.from(document.querySelectorAll(`.syntax-languages tbody tr`)).find(r=>r.querySelector(`th`)?.textContent==="JSON")?.querySelector(`td`)?.textContent === '+json.dumps(name))
    def parser_action(name): click(name+' JSON parser','[role=dialog][aria-label="Syntax languages"]')
    def confirm(name): click(name+' parser','[role=alertdialog]')
    def capture(name,note,texts,allow_alert=False):
        frames.capture(name,note,texts,allow_alert=allow_alert)
        assert rows()==initial_rows and panes()==initial_panes, 'Parser operation changed original sessions or running panes'
    frames.resize(1360,980)
    action('wait',text='Invoice rounding',scope='nav[aria-label=Workspace]')
    action('click',selector='nav[aria-label=Workspace] [aria-describedby="tree-feature-proof-feature"]')
    action('wait',text='Learning',scope='main')
    initial_rows, initial_panes = rows(), panes()
    manager(); state_is('Not installed')
    assert frames.evaluate('document.querySelectorAll(`.syntax-languages tbody tr`).length')==16
    capture('001-shared-catalog.png','The manager lists every shared syntax language and its current disk/load state without changing the active feature.', ['Syntax languages','JSON','Not installed','Refresh languages'])
    parser_action('Install')
    capture('002-install-confirmation.png','Choosing Install stages a confirmation; no parser or source checkout exists until explicitly confirmed.', ['Install the JSON parser?','Install parser','Cancel'])
    assert not library.exists() and not source.exists()
    click('Cancel','[role=alertdialog]'); assert not library.exists() and not source.exists()
    (scratch/'hold-compiler').touch(); parser_action('Install'); confirm('Install'); wait_file(scratch/'compiler-waiting')
    capture('003-install-busy.png','Confirmed Install uses the real shared installer and compiler. While the compiler is gated, duplicate actions are disabled and progress is visible.', ['Installing JSON parser','continues if you close'])
    assert frames.evaluate('Array.from(document.querySelectorAll(`.syntax-languages button`)).every(b=>b.disabled)')
    click('Close manager'); manager()
    action('wait',text='Installing JSON parser',scope='[role=dialog][aria-label="Syntax languages"]')
    capture('004-busy-reopened.png','Closing and reopening the manager observes the same still-running install worker; it does not start a second operation.', ['Installing JSON parser','Refresh languages','Close manager'])
    (scratch/'hold-compiler').unlink(); state_is('Installed')
    assert source.exists() and library.stat().st_size>1000
    actual_sha = subprocess.check_output(['git','-C',str(source),'rev-parse','HEAD'],text=True).strip()
    assert actual_sha=='ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24'
    capture('005-parser-installed.png','The pinned official JSON grammar was cloned from the private mirror, compiled with the real C compiler, and loaded successfully by AMF.', ['JSON','Installed','Installed JSON tree-sitter parser'])
    parser_action('Uninstall'); replace_library(b'externally replaced parser')
    confirm('Uninstall'); action('wait',text='Parser files changed.',scope='[role=dialog][aria-label="Syntax languages"]')
    assert library.read_bytes()==b'externally replaced parser' and source.exists()
    capture('006-stale-refusal.png','An external library replacement invalidates the confirmed revision. The stale uninstall is refused and preserves the replacement.', ['Parser files changed.','Uninstall the JSON parser?'],allow_alert=True)
    click('Refresh languages'); state_is('Needs repair'); parser_action('Repair'); confirm('Repair'); state_is('Installed')
    click('Close manager'); click('Learning','main')
    action('wait',text='invoice.json',scope='[role=dialog][aria-label="Learning"]')
    click('invoice.json','[role=dialog][aria-label="Learning"]')
    wait('!!document.querySelector(`.learning-reader .syn-string`) || !!document.querySelector(`.learning-content .syn-string`) || !!document.querySelector(`[role=dialog][aria-label="Learning"] .syn-string`)')
    action('fill',name='Question',value='Keep this unsent question while repairing JSON.',scope='[role=dialog][aria-label="Learning"]')
    capture('007-learning-highlighted.png','Learning uses the installed parser for real JSON token spans. Its unsent question remains local and no answering harness runs.', ['Learning','JSON','Question'])
    click('Manage syntax languages','[role=dialog][aria-label="Learning"]'); state_is('Installed')
    replace_library(b'broken external parser replacement')
    click('Refresh languages'); state_is('Needs repair')
    capture('008-external-broken.png','Explicit refresh discovers an externally replaced, unloadable parser as Needs repair while Learning stays mounted with its unsent question.', ['JSON','Needs repair','Repair…','Uninstall…'])
    parser_action('Repair'); confirm('Repair'); state_is('Installed')
    parser_action('Uninstall')
    capture('009-repaired-confirm-uninstall.png','Repair restores a loadable parser through the same real installer. Uninstall is a new explicit confirmation and has not yet removed either managed artifact.', ['JSON','Installed','Uninstall the JSON parser?','Uninstall parser'])
    assert source.exists() and library.exists()
    confirm('Uninstall'); state_is('Not installed')
    assert not source.exists() and not library.exists()
    capture('010-uninstalled.png','Confirmed uninstall removes only the private shared parser and managed grammar checkout, then refreshes the catalog to Not installed.', ['JSON','Not installed','Removed JSON tree-sitter parser'])
    click('Close manager')
    wait('document.querySelector(`[role=dialog][aria-label="Learning"] textarea`)?.value === "Keep this unsent question while repairing JSON."')
    action('wait',text='JSON parser not installed',scope='[role=dialog][aria-label="Learning"]')
    capture('011-learning-draft-preserved.png','Returning to Learning retains the exact unsent question and selected file; the removed parser now renders as plain text without sending anything.', ['Learning','Plain text','JSON parser not installed','Question'])
    assert not (scratch/'unexpected-prompt').exists()
    frames.write_notes()
    (out/'assertions.json').write_text(json.dumps({'frames':11,'catalog_languages':16,'pinned_grammar':actual_sha,'actual_compile_and_load':True,'stale_uninstall_preserved_replacement':True,'busy_close_reopen':True,'external_broken_refresh':True,'repair_and_uninstall':True,'learning_draft_preserved':True,'original_rows_and_pane_pids_preserved':True,'no_paid_harnesses':True},indent=2)+'\n')
