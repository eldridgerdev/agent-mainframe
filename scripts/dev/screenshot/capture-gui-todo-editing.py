#!/usr/bin/env python3
"""Run TODO editing and jump proof with private SQLite, Git and tmux fixtures.

Only silent sleep processes run in the feature session; no harness launches.
HOME and the user's database, repositories and tmux server are untouched.
"""

import datetime
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
port = int(os.environ.get("AMF_GUI_CAPTURE_PORT", "1420"))

with tempfile.TemporaryDirectory(prefix="amf-gui-todo-editing-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        inspector = f"127.0.0.1:{listener.getsockname()[1]}"
    env = os.environ.copy()
    for name in ["CONFIG", "STATE", "DATA", "CACHE"]:
        directory = scratch / name.lower()
        directory.mkdir()
        env[f"XDG_{name}_HOME"] = str(directory)
    config = scratch / "config/amf"
    config.mkdir()
    (config / "config.json").write_text("{}\n")
    env.update(GDK_BACKEND="x11", WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_SOCKET=str(scratch / "tmux.sock"))
    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo", "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo", "GIT_COMMITTER_EMAIL": "demo@example.com",
    }
    repo = scratch / "billing-api"
    repo.mkdir()

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=repo, env=git_env, text=True).strip()

    git("init", "-q", "-b", "main")
    (repo / "README.md").write_text("# Billing API\n")
    git("add", ".")
    git("commit", "-q", "-m", "Initial")
    worktree = scratch / "worktrees" / "billing-retry"
    git("worktree", "add", "-q", "-b", "billing-retry", str(worktree))
    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    vite = gui = None
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    try:
        subprocess.run(tmux + ["new-session", "-d", "-s", "amf-billing-retry", "-n", "claude",
                               "-x", "120", "-y", "30", "sleep 3600"], check=True)
        try:
            with socket.create_connection(("localhost", port), timeout=1):
                raise RuntimeError(f"Port {port} is already in use")
        except OSError:
            pass
        vite = subprocess.Popen(["npm", "run", "dev", "--", "--port", str(port)],
                                cwd=workspace / "gui", stdout=vite_log,
                                stderr=subprocess.STDOUT, start_new_session=True)
        for _ in range(100):
            if vite.poll() is not None:
                raise RuntimeError("The isolated Vite server failed")
            try:
                with urllib.request.urlopen(f"http://localhost:{port}/", timeout=1):
                    break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("The isolated frontend did not become ready")
        gui = subprocess.Popen(
            [os.environ.get("AMF_GUI_CAPTURE_BINARY", str(workspace / "target/debug/amf-gui"))],
            cwd=scratch, env=env, stdout=gui_log, stderr=subprocess.STDOUT, start_new_session=True)
        dbpath = config / "amf.db"
        for _ in range(100):
            if gui.poll() is not None:
                raise RuntimeError((out / "gui.log").read_text())
            if dbpath.exists():
                try:
                    with sqlite3.connect(dbpath) as db:
                        db.execute("SELECT scope FROM todo_lists")
                        db.execute("SELECT value FROM store_meta WHERE key='store_version'")
                    break
                except sqlite3.OperationalError:
                    pass
            time.sleep(0.25)
        else:
            raise RuntimeError("GUI database never initialized")
        stamp = datetime.datetime.now(datetime.timezone.utc).isoformat()
        with sqlite3.connect(dbpath) as db:
            db.execute("INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)",
                       ("shot-project", "Billing API", str(repo), 1, stamp))
            db.execute("INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,created_at,last_accessed,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                       ("billing-retry", "shot-project", "billing-retry", "billing-retry", str(worktree), 1,
                        "amf-billing-retry", "idle", stamp, stamp, 0))
            db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                       ("billing-retry-claude", "billing-retry", "claude", "Claude", "claude", stamp, 0))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(["/usr/bin/python3", str(workspace / "scripts/dev/screenshot/capture-gui-todo-editing-frames.py"),
                        str(out), str(gui.pid), inspector, str(dbpath), env["AMF_TMUX_SOCKET"]],
                       env=env, check=True)
        print("PASS: TODO editing, stale-edit refusal, scratchpad and jump-to-work verified", flush=True)
    finally:
        try:
            with sqlite3.connect(config / "amf.db") as src, sqlite3.connect(out / "fixture-final.db") as dst:
                src.backup(dst)
        except (OSError, sqlite3.Error):
            pass
        subprocess.run(tmux + ["kill-server"], check=False, stderr=subprocess.DEVNULL)
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
