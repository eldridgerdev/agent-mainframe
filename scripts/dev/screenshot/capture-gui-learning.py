#!/usr/bin/env python3
"""Native Learning proof using private SQLite/Git/tmux and offline CLIs."""
import json
import os
import pathlib
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request

out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]
port = int(os.environ.get('AMF_GUI_CAPTURE_PORT', '1420'))
with tempfile.TemporaryDirectory(prefix='amf-gui-learning-') as temporary:
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
    (config / 'config.json').write_text(json.dumps({'max_concurrent_agents': 1, 'low_memory_warn_mb': 0, 'dormant_idle_minutes': 0}))
    env.update(GDK_BACKEND='x11', WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_BIN=shutil.which('tmux'), AMF_TMUX_SOCKET=str(scratch / 'tmux.sock'), AMF_TMUX_DEDICATED_SOCKET='1', AMF_GUI_LEARNING_CALLS=str(scratch / 'calls.jsonl'))
    pathlib.Path(env['AMF_GUI_LEARNING_CALLS']).write_text('')
    fixture_bin = scratch / 'bin'
    fixture_bin.mkdir()
    for harness in ['claude', 'codex', 'opencode', 'pi']:
        binary = fixture_bin / harness
        shutil.copyfile(workspace / 'scripts/dev/screenshot/fixtures/gui-learning-harness.py', binary)
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
    (repo / '.amf-gui-learning-fixture').write_text('offline capture only\n')
    (repo / 'quota.py').write_text('def within_quota(used, limit):\n    return used < limit\n')
    git('add', '.')
    git('commit', '-q', '-m', 'Initial invoice API')
    host = scratch / 'worktrees/host'
    git('worktree', 'add', '-q', '-b', 'host', str(host))
    (host / 'quota.py').write_text('def within_quota(used, limit):\n    # Reject accounts at or above their quota.\n    return used < limit\n')
    tmux = ['tmux', '-S', env['AMF_TMUX_SOCKET']]
    binary = os.environ.get('AMF_GUI_CAPTURE_BINARY', str(workspace / 'target/debug/amf-gui'))
    gui_pid_path = scratch / 'gui.pid'
    command = ['/bin/sh', '-c', 'echo "$$" > "$1"; exec "$2"', 'learning-capture', str(gui_pid_path), binary]
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
        subprocess.run(tmux + ['new-session', '-d', '-s', 'amf-fixture-host', '-n', 'codex', 'sleep 3600 & wait'], env=env, check=True)
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
        with sqlite3.connect(dbpath) as db:
            db.execute('INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)', ('shot-project', 'Invoice API', str(repo), 1, stamp))
            db.execute('INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,agent,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?,?,?)',
                       ('host', 'shot-project', 'host', 'host', str(host), 1, 'amf-fixture-host', 'idle', 'codex', stamp, stamp))
            for index, kind in enumerate(['codex', 'todos']):
                db.execute('INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)',
                           ('host-' + kind, 'host', kind, kind.capitalize(), kind, stamp, index))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(['/usr/bin/python3', str(workspace / 'scripts/dev/screenshot/capture-gui-learning-frames.py'),
                        str(out), gui_pid_path.read_text().strip(), inspector, str(dbpath), str(repo), str(scratch)], env=env, check=True)
    finally:
        subprocess.run(tmux + ['kill-server'], check=False, stderr=subprocess.DEVNULL)
        for child in [gui, vite]:
            if child and child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
        gui_log.close()
        vite_log.close()
