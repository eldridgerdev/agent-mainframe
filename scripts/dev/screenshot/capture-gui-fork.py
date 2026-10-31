#!/usr/bin/env python3
"""Run desktop fork proof with private SQLite and Git fixtures.

The forked feature is created stopped and no agent is ever started, so no
harness (paid or not) runs. The user's database, repositories and tmux server
are untouched.
"""

import datetime
import os
import pathlib
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

with tempfile.TemporaryDirectory(prefix="amf-gui-fork-") as temporary:
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
    # The fork dialog lists only harnesses that answer `--version`. A stub shadows
    # any real harness (CI has none), so the proof never runs a real agent; the
    # fork is created stopped, so even the stub is never started.
    stub_bin = scratch / "bin"
    stub_bin.mkdir()
    for harness in ["claude", "codex", "opencode", "pi"]:
        stub = stub_bin / harness
        stub.write_text('#!/bin/sh\necho "0.0.0-stub"\n')
        stub.chmod(0o755)
    env["PATH"] = f"{stub_bin}{os.pathsep}{env['PATH']}"
    env.update(GDK_BACKEND="x11", WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_SOCKET=str(scratch / "tmux.sock"))
    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo", "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo", "GIT_COMMITTER_EMAIL": "demo@example.com",
    }
    repo = scratch / "billing-api"
    repo.mkdir()

    def git(*args, cwd=None):
        return subprocess.check_output(["git", *args], cwd=cwd or repo, env=git_env, text=True).strip()

    git("init", "-q", "-b", "billing-retry")
    (repo / "README.md").write_text("# Billing API\n")
    (repo / "retry.py").write_text("RETRIES = 3\n")
    (repo / "deploy.sh").write_text("#!/bin/sh\necho deploy\n")
    git("add", ".")
    git("commit", "-q", "-m", "Initial")
    # Uncommitted work the fork must inherit; the staged edit must not stay staged.
    (repo / "retry.py").write_text("RETRIES = 5  # working copy\n")
    git("add", "retry.py")
    (repo / "retry.py").write_text("RETRIES = 7  # unstaged on top of staged\n")
    (repo / "notes.md").write_text("untracked idea\n")
    (repo / "deploy.sh").chmod(0o755)
    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    vite = gui = None
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    try:
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
        stamp = (datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(days=1)).isoformat()
        with sqlite3.connect(dbpath) as db:
            db.execute("INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)",
                       ("shot-project", "Billing API", str(repo), 1, stamp))
            db.execute("INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,created_at,last_accessed,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                       ("source-feature", "shot-project", "billing-retry", "billing-retry", str(repo), 0,
                        "amf-billing-retry", "stopped", stamp, stamp, 0))
            for index, (kind, label) in enumerate([("claude", "Claude 1"), ("codex", "Codex 1")]):
                db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                           (f"source-{kind}", "source-feature", kind, label, kind, stamp, index))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(["/usr/bin/python3", str(workspace / "scripts/dev/screenshot/capture-gui-fork-frames.py"),
                        str(out), str(gui.pid), inspector, str(dbpath), str(repo)],
                       env=env, check=True)
        fork = repo / ".worktrees" / "billing-retry-fork"
        assert (fork / "retry.py").read_text() == "RETRIES = 7  # unstaged on top of staged\n"
        assert (fork / "notes.md").read_text() == "untracked idea\n"
        assert os.access(fork / "deploy.sh", os.X_OK)
        assert git("diff", "--cached", "--name-only", cwd=fork) == ""
        assert git("status", "--porcelain", cwd=repo).count("\n") >= 1
        with sqlite3.connect(dbpath) as db:
            rows = db.execute("SELECT name,status,is_worktree FROM features ORDER BY sort_order,name").fetchall()
        assert ("billing-retry-fork", "stopped", 1) in rows and len(rows) == 2, rows
        listed = subprocess.run(tmux + ["list-panes", "-a", "-F", "#{session_name}:#{window_name}:#{pane_current_command}"], capture_output=True, text=True); others = [l for l in listed.stdout.splitlines() if not l.startswith("_amf-observer:")]; assert not others, "agent tmux sessions exist: " + repr(others)
        print("PASS: working content inherited, index empty, fork stopped, nothing launched", flush=True)
    finally:
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
