#!/usr/bin/env python3
"""Native TODO preset proof using private SQLite/Git/tmux and offline CLIs."""
import hashlib
import json
import os
import pathlib
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request

from owned_process import stop_owned_process_group

# Compare digests only; never retain or print user configuration contents.
def harness_config_snapshot():
    home = pathlib.Path.home()
    paths = [home/'.claude/settings.json', home/'.claude/settings.local.json', home/'.codex/config.toml']
    directory = home/'.config/opencode'
    if directory.is_dir():
        paths += [path for path in directory.rglob('*') if path.is_file()]
    return {str(path): hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None for path in paths}

user_config_before = harness_config_snapshot()

out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]
port = int(os.environ.get('AMF_GUI_CAPTURE_PORT', '1420'))
with tempfile.TemporaryDirectory(prefix='amf-gui-todo-presets-') as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        inspector = f'127.0.0.1:{listener.getsockname()[1]}'
    env = os.environ.copy()
    for name in ['CONFIG', 'STATE', 'DATA', 'CACHE']:
        folder = scratch / name.lower()
        folder.mkdir()
        env[f'XDG_{name}_HOME'] = str(folder)
    config = scratch / 'config/amf'
    config.mkdir()
    (config / 'config.json').write_text(json.dumps({'max_concurrent_agents': 2, 'remote_control_default': True, 'low_memory_warn_mb': 0, 'dormant_idle_minutes': 0, 'extension': {'feature_presets': [
        {'name':'Shared','agent':'claude','branch_prefix':'global','mode':'vibe'},
        {'name':'Remote','agent':'claude','branch_prefix':'remote','mode':'vibe','remote_control':True,'plan_mode':True,'quick_plan':True},
        {'name':'Full','agent':'opencode','branch_prefix':'full','mode':'vibe','plan_mode':True},
        {'name':'Hidden Pi','agent':'pi','mode':'vibe'}]}}))
    env.update(GDK_BACKEND='x11', WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_BIN=shutil.which('tmux'), AMF_TMUX_SOCKET=str(scratch / 'tmux.sock'), AMF_TMUX_DEDICATED_SOCKET='1', AMF_GUI_PLANNING_CALLS=str(scratch / 'calls.jsonl'))
    pathlib.Path(env['AMF_GUI_PLANNING_CALLS']).write_text('')
    fixture_bin = scratch / 'bin'
    fixture_bin.mkdir()
    for harness in ['claude', 'codex', 'opencode', 'pi']:
        binary = fixture_bin / harness
        shutil.copyfile(workspace / 'scripts/dev/screenshot/fixtures/gui-planning-harness.py', binary)
        binary.chmod(0o755)
    env['PATH'] = str(fixture_bin) + os.pathsep + env['PATH']
    git_env = env | {'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1',
                     'GIT_AUTHOR_NAME': 'AMF demo', 'GIT_AUTHOR_EMAIL': 'demo@example.com',
                     'GIT_COMMITTER_NAME': 'AMF demo', 'GIT_COMMITTER_EMAIL': 'demo@example.com'}
    # Git subprocesses spawned by the GUI need fixture-local identity, too.
    env.update({key: value for key, value in git_env.items() if key.startswith('GIT_')})
    repo = scratch / 'invoice-api'
    repo.mkdir()
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=repo, env=git_env, text=True).strip()
    git('init', '-q', '-b', 'main')
    (repo / 'README.md').write_text('# Invoice API\n')
    (repo / '.amf-gui-planning-fixture').write_text('offline capture only\n')
    (repo / 'amf.json').write_text(json.dumps({
        'allowed_agents': ['claude', 'opencode'],
        'feature_presets': [{'name':'Shared','branch_prefix':'project','agent':'opencode','mode':'supervibe','review':True,'enable_chrome':True,'plan_mode':True,'quick_plan':True}],
        'skip_builtin_questions': True,
        'plan_questions': [{'id': 'quota', 'text': 'Which quota rule should be preserved?', 'optional': False}],
        'lifecycle_hooks': {'on_worktree_created': {
            'script': 'printf "%s" "$AMF_HOOK_CHOICE" > hook-choice; printf "%s\\n" "$AMF_HOOK_CHOICE" >> "$AMF_GUI_PLANNING_HOOKS"',
            'prompt': {'title': 'Choose fixture stack', 'options': ['node', 'rust & tools']}}}}))
    env['AMF_GUI_PLANNING_HOOKS'] = str(scratch / 'hooks.txt')
    pathlib.Path(env['AMF_GUI_PLANNING_HOOKS']).write_text('')
    git('add', '.')
    git('commit', '-q', '-m', 'Initial invoice API')
    host = scratch / 'worktrees/host'
    git('worktree', 'add', '-q', '-b', 'host', str(host))
    tmux = ['tmux', '-S', env['AMF_TMUX_SOCKET']]
    binary = os.environ.get('AMF_GUI_CAPTURE_BINARY', str(workspace / 'target/debug/amf-gui'))
    gui_pid_path = scratch / 'gui.pid'
    command = ['/bin/sh', '-c', 'echo "$$" > "$1"; exec "$2"', 'planning-capture', str(gui_pid_path), binary]
    versions = pathlib.Path.home() / '.local/share/claude/versions'
    if versions.exists():
        assert shutil.which('bwrap'), 'Masking installed Claude versions requires bubblewrap'
        empty = scratch / 'empty-versions'
        empty.mkdir()
        command = ['bwrap', '--die-with-parent', '--dev-bind', '/', '/', '--bind', str(empty), str(versions), '--', *command]
    try:
        with socket.create_connection(('localhost', port), timeout=1):
            raise RuntimeError(f'Port {port} is already in use')
    except OSError:
        pass
    gui = vite = None
    vite_log = (out / 'vite.log').open('w')
    gui_log = (out / 'gui.log').open('w')
    try:
        # Keeper makes the first later-created session use this private server
        # and its fixture-only PATH/environment.
        subprocess.run(tmux + ['new-session', '-d', '-s', 'fixture-keeper', 'sleep 3600'], env=env, check=True)
        subprocess.run(tmux + ['new-session', '-d', '-s', 'amf-fixture-host', '-n', 'opencode', 'sleep 3600 & wait'], env=env, check=True)
        vite = subprocess.Popen(['npm', 'run', 'dev', '--', '--port', str(port)], cwd=workspace / 'gui', stdout=vite_log,
                                stderr=subprocess.STDOUT, start_new_session=True)
        for _ in range(100):
            if vite.poll() is not None:
                raise RuntimeError('The isolated Vite server failed')
            try:
                with urllib.request.urlopen(f'http://localhost:{port}/', timeout=1):
                    break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError('The isolated frontend did not become ready')
        gui = subprocess.Popen(command, cwd=scratch, env=env, stdout=gui_log, stderr=subprocess.STDOUT, start_new_session=True)
        dbpath = config / 'amf.db'
        for _ in range(100):
            if gui.poll() is not None:
                raise RuntimeError((out / 'gui.log').read_text())
            try:
                with sqlite3.connect(dbpath) as db:
                    db.execute('SELECT scope FROM todo_lists')
                    db.execute('SELECT value FROM store_meta')
                break
            except sqlite3.OperationalError:
                time.sleep(0.25)
        else:
            raise RuntimeError('GUI database did not initialize')
        stamp = '2026-10-01T12:00:00Z'
        with sqlite3.connect(dbpath, timeout=30) as db:
            db.execute('BEGIN IMMEDIATE')
            db.execute('INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)', ('shot-project', 'Invoice API', str(repo), 1, stamp))
            db.execute('INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,agent,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                       ('host', 'shot-project', 'host', 'host', str(host), 1, 'amf-fixture-host', 'idle', 'opencode', stamp, stamp))
            for index, kind in enumerate(['opencode', 'todos']):
                db.execute('INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)',
                           ('host-' + kind, 'host', kind, kind.capitalize(), kind, stamp, index))
            db.execute('INSERT INTO todo_lists(id,project_id,feature_id,scope,created_at,updated_at) VALUES(?,?,?,?,?,?)',
                       ('project-list', 'shot-project', 'host', 'project', stamp, stamp))
            for index, (tid, title, status, session) in enumerate([
                ('move', 'Keep linked quota work', 'in_progress', 'host-opencode'),
                ('direct', 'Implement invoice guard', 'not_started', None),
                ('plan', 'Plan quota validation', 'not_started', None)]):
                db.execute('INSERT INTO todos(id,list_id,title,status,agent_session_id,sort_order,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?)',
                           (tid, 'project-list', title, status, session, index, stamp, stamp))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(['/usr/bin/python3', str(workspace / 'scripts/dev/screenshot/capture-gui-todo-presets-frames.py'),
                        str(out), gui_pid_path.read_text().strip(), inspector, str(dbpath), str(repo), str(scratch)], env=env, check=True)
    finally:
        for child in [gui, vite]:
            stop_owned_process_group(child)
        subprocess.run(tmux + ['kill-server'], check=False, stderr=subprocess.DEVNULL)
        assert subprocess.run(tmux + ['has-session'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
        gui_log.close()
        vite_log.close()
        assert harness_config_snapshot() == user_config_before, 'User harness configuration changed'
    validation = json.loads((out/'validation.json').read_text())
    validation['owned_gui_vite_private_tmux_closed'] = True
    validation['real_user_harness_config_unchanged'] = True
    (out/'validation.json').write_text(json.dumps(validation,indent=2)+'\n')
    print('PASS: TODO presets, actual native backend, offline receivers and owned cleanup', flush=True)
