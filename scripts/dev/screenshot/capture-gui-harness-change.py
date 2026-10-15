#!/usr/bin/env python3
"""Offline native harness replacement; private Git, DB, XDG, tmux and HOME/.amf mount.

HOME is preserved. No paid harness runs; only private Claude/OpenCode receivers
launch. User harness configuration and AMF runtime inventory must stay unchanged.
"""
import hashlib
import json
import os
import pathlib
import shlex
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request

from owned_process import stop_owned_process_group

workspace = pathlib.Path(__file__).resolve().parents[3]
out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
port = int(os.environ.get('AMF_GUI_CAPTURE_PORT', '1420'))

def inventory(path):
    if not path.exists():
        return 'missing'
    files = [path] if path.is_file() else sorted(path.rglob('*'))
    rows = []
    for item in files:
        if item.is_symlink():
            rows.append((str(item.relative_to(path.parent)), 'link', os.readlink(item)))
        elif item.is_file():
            rows.append((str(item.relative_to(path.parent)), 'file', item.stat().st_mode & 0o777, hashlib.sha256(item.read_bytes()).hexdigest()))
    return hashlib.sha256(json.dumps(rows).encode()).hexdigest()

home = pathlib.Path.home()
protected = [home/'.claude/settings.json', home/'.claude/settings.local.json', home/'.codex/config.toml',
             pathlib.Path(os.environ.get('XDG_CONFIG_HOME', home/'.config'))/'opencode', home/'.amf']
before = [inventory(path) for path in protected]
assert shutil.which('bwrap'), 'Private HOME runtime mount requires bubblewrap'
with tempfile.TemporaryDirectory(prefix='amf-gui-harness-change-') as temporary:
    scratch = pathlib.Path(temporary)
    env = os.environ.copy()
    for name in ['CONFIG', 'STATE', 'DATA', 'CACHE']:
        folder = scratch/name.lower(); folder.mkdir(); env[f'XDG_{name}_HOME'] = str(folder)
    config = scratch/'config/amf'; config.mkdir()
    (config/'config.json').write_text(json.dumps({'config_version':1, 'low_memory_warn_mb':0, 'dormant_idle_minutes':0,
                                                'extension':{'allowed_agents':['claude','opencode']}}))
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0)); inspector = f'127.0.0.1:{listener.getsockname()[1]}'
    env.update(GDK_BACKEND='x11', WEBKIT_INSPECTOR_HTTP_SERVER=inspector, AMF_TMUX_BIN=shutil.which('tmux'),
               AMF_TMUX_SOCKET=str(scratch/'tmux.sock'), AMF_TMUX_DEDICATED_SOCKET='1', AMF_HARNESS_FIXTURE=str(scratch))
    fixture_bin = scratch/'bin'; fixture_bin.mkdir()
    receiver = '''#!/usr/bin/python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
if sys.argv[1:] and sys.argv[1] in ['--version', '--help']:
    print('Offline fixture 2.1.60'); sys.exit(0)
assert name in ['claude', 'opencode'], 'Unexpected harness launch'
root = pathlib.Path(os.environ['AMF_HARNESS_FIXTURE'])
with (root/'launches.jsonl').open('a') as stream: stream.write(json.dumps({'harness':name,'args':sys.argv[1:]})+'\\n')
print('Offline '+name+' session · ready', flush=True)
for line in sys.stdin:
    (root/'unexpected-prompt').write_text(line)
    raise AssertionError('Proof must not send prompts to harnesses')
'''
    for name in ['claude','opencode','codex','pi']:
        path = fixture_bin/name; path.write_text(receiver); path.chmod(0o755)
    env['PATH'] = str(fixture_bin)+os.pathsep+env['PATH']
    env.update(GIT_CONFIG_GLOBAL='/dev/null', GIT_CONFIG_NOSYSTEM='1', GIT_AUTHOR_NAME='AMF fixture', GIT_COMMITTER_NAME='AMF fixture',
               GIT_AUTHOR_EMAIL='fixture@example.test', GIT_COMMITTER_EMAIL='fixture@example.test')
    repo = scratch/'billing-api'; repo.mkdir()
    def git(*args):
        subprocess.run(['git', *args], cwd=repo, env=env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    git('init', '-b', 'main'); (repo/'README.md').write_text('# Billing API\n'); git('add','.'); git('commit','-m','Offline fixture')
    host = scratch/'checkout'; git('worktree','add','-b','invoice-rounding',str(host))
    tmux = [shutil.which('tmux'),'-S',env['AMF_TMUX_SOCKET']]
    binary = os.environ.get('AMF_GUI_CAPTURE_BINARY', str(workspace/'target/debug/amf-gui'))
    pid_path = scratch/'gui.pid'
    private_runtime = scratch/'home-amf'; private_runtime.mkdir()
    # A mount needs an existing target. When absent, this runner owns only the
    # empty placeholder, removes it after children close, and verifies inventory.
    runtime = home/'.amf'; created_runtime = not runtime.exists()
    if created_runtime: runtime.mkdir()
    command = ['bwrap','--die-with-parent','--dev-bind','/','/','--bind',str(private_runtime),str(runtime)]
    versions = home/'.local/share/claude/versions'
    if versions.exists():
        empty = scratch/'empty-versions'; empty.mkdir(); command += ['--bind',str(empty),str(versions)]
    command += ['--','/bin/sh','-c','echo "$$" > "$1"; exec "$2"','harness-change-proof',str(pid_path),binary]
    gui = vite = None
    try:
        with socket.socket() as probe:
            assert probe.connect_ex(('127.0.0.1', port)) != 0, f'Port {port} is occupied'
        subprocess.run(tmux+['new-session','-d','-s','fixture-keeper','sleep 3600'],env=env,check=True)
        subprocess.run(tmux+['set-option','-g','default-shell','/bin/bash'],env=env,check=True)
        subprocess.run(tmux+['set-option','-g','default-command','exec /bin/bash --noprofile --norc'],env=env,check=True)
        subprocess.run(tmux+['new-session','-d','-s','amf-harness-proof','-n','claude',str(fixture_bin/'claude')],env=env,check=True)
        for window in ['terminal','watcher']:
            subprocess.run(tmux+['new-window','-t','amf-harness-proof','-n',window,'sleep 3600'],env=env,check=True)
        with (out/'vite.log').open('w') as log:
            vite = subprocess.Popen(['npm','run','dev','--','--port',str(port)],cwd=workspace/'gui',stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        for _ in range(100):
            assert vite.poll() is None, 'Private Vite exited'
            try:
                urllib.request.urlopen(f'http://localhost:{port}',timeout=1).close(); break
            except OSError: time.sleep(.2)
        else: raise RuntimeError('Private frontend did not start')
        with (out/'gui.log').open('w') as log:
            gui = subprocess.Popen(command,cwd=scratch,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        dbpath = config/'amf.db'
        for _ in range(100):
            assert gui.poll() is None, (out/'gui.log').read_text()
            try:
                with sqlite3.connect(dbpath) as db:
                    # store_meta exists before later migrations add stopped.
                    # Seed only after this source's complete schema is present.
                    version = db.execute('SELECT COALESCE(MAX(version),0) FROM schema_version').fetchone()[0]
                    if version < 47:
                        time.sleep(.2)
                        continue
                    db.execute('SELECT stopped,token_usage_source,token_usage_source_match FROM feature_sessions LIMIT 0')
                break
            except sqlite3.OperationalError: time.sleep(.2)
        else: raise RuntimeError('Private database did not initialize')
        stamp = '2026-10-01T12:00:00Z'
        with sqlite3.connect(dbpath) as db:
            db.execute('INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)',('proof-project','Billing API',str(repo),1,stamp))
            db.execute('INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,agent,mode,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)',
                       ('proof-feature','proof-project','Invoice rounding','invoice-rounding',str(host),1,'amf-harness-proof','idle','claude','supervibe',stamp,stamp))
            for index,(name,kind,window,stopped) in enumerate([('Primary agent','claude','claude',0),('Paused agent','claude','claude-2',1),('Terminal','terminal','terminal',0),('Watcher','custom','watcher',0)]):
                db.execute('INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,claude_session_id,stopped,created_at,sort_order,command,on_stop) VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                           (f'proof-{index}','proof-feature',kind,name,window,f'old-conversation-{index}' if kind=='claude' else None,stopped,stamp,index,
                            shlex.quote(str(fixture_bin/'opencode')) if kind=='custom' else None, 'touch '+shlex.quote(str(scratch/'custom-stopped')) if kind=='custom' else None))
            for index in [0,1]:
                db.execute('UPDATE feature_sessions SET token_usage_source=?,token_usage_source_match=? WHERE id=?',
                           # Match db/store.rs::source_to_json byte-for-byte;
                           # ordinary background saves normalize JSON whitespace.
                           (json.dumps({'provider':'claude','id':f'old-conversation-{index}'}, separators=(',', ':')),'exact',f'proof-{index}'))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(['/usr/bin/python3',str(workspace/'scripts/dev/screenshot/capture-gui-harness-change-frames.py'),str(out),pid_path.read_text().strip(),inspector,str(dbpath),str(repo),str(scratch)],env=env,check=True)
    finally:
        subprocess.run(tmux+['kill-server'],stderr=subprocess.DEVNULL,check=False)
        for child in [gui,vite]: stop_owned_process_group(child)
        if created_runtime: runtime.rmdir()
        assert [inventory(path) for path in protected] == before, 'Real user harness configuration or AMF runtime inventory changed'
    (out/'isolation.json').write_text(json.dumps({'home_preserved':True,'private_home_amf_mount':True,'protected_inventory_unchanged':True,'paid_harnesses':False},indent=2)+'\n')
