#!/usr/bin/env python3
"""Run native GUI dormancy proof against private fixtures.

HOME is preserved. The database, config, worktrees and tmux server (its own
socket) are private to this run. Dormancy is made real rather than faked: the
features' `last_accessed` is seeded in the past and their tmux windows run
silent `sleep` processes, so tmux itself reports them idle once the configured
one-minute threshold passes. The two "editors" are stand-in processes this
script starts: one recorded as AMF-owned (AMF closes it), one recorded as not
owned (AMF must leave it running). No real editor or user process is ever
recorded, signalled or killed; only owned GUI/Vite process groups, the
stand-ins and the private tmux server are stopped on exit.
"""

import datetime
import os
import pathlib
import shlex
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


def iso(delta):
    moment = datetime.datetime.now(datetime.timezone.utc) - delta
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


with tempfile.TemporaryDirectory(prefix="amf-gui-dormancy-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    config = scratch / "config"
    state = scratch / "state"
    repo = scratch / "billing-api"
    (config / "amf").mkdir(parents=True)
    (state / "amf").mkdir(parents=True)
    repo.mkdir()
    # The smallest thresholds the config allows: idle over one minute and
    # unopened over one hour.
    (config / "amf/config.json").write_text(
        '{"dormant_idle_minutes": 1, "dormant_last_accessed_hours": 1, "kill_editor_on_stop": true}\n'
    )
    env = os.environ.copy()
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        AMF_TMUX_SOCKET=str(scratch / "dormancy-tmux.sock"),
    )
    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo",
        "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo",
        "GIT_COMMITTER_EMAIL": "demo@example.com",
    }
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repo, env=git_env, check=True)
    (repo / "README.md").write_text("# Billing API\n")
    subprocess.run(["git", "add", "."], cwd=repo, env=git_env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "Initial"], cwd=repo, env=git_env, check=True)

    # (id, name, hours since last opened, window command)
    features = [
        ("billing-retry", "billing-retry", 49, "sleep 3600"),
        ("docs-refresh", "docs-refresh", 9, "sleep 3600"),
        ("search-index", "search-index", 6, "sleep 3600"),
        # Untouched for a day, but its agent is printing output: not dormant.
        ("checkout-flow", "checkout-flow", 30, "while true; do date; sleep 2; done"),
    ]
    worktrees = {}
    for fid, _, _, _ in features:
        worktrees[fid] = scratch / "worktrees" / fid
        worktrees[fid].mkdir(parents=True)

    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    # Stand-in editors. The owned one has VS Code's shape (a `code` binary,
    # `--new-window <worktree>`, a child holding the "language server").
    fake_code = scratch / "bin" / "code"
    fake_code.parent.mkdir()
    os.symlink("/bin/bash", fake_code)
    owned_editor = None
    foreign_editor = None
    vite = gui = None
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    try:
        for fid, _, _, command in features:
            subprocess.run(
                tmux + ["new-session", "-d", "-s", f"amf-{fid}", "-n", "claude", "-x", "120", "-y", "30",
                        "bash -c " + shlex.quote(command)],
                check=True,
            )
        tmux_started = time.monotonic()
        # Detached through a short-lived shell, as a real window outlives the
        # `code` CLI: once AMF closes it, init reaps it rather than leaving a
        # zombie of this script's that still looks alive.
        owned_editor = int(subprocess.check_output(
            ["setsid", "sh", "-c", '"$@" >/dev/null 2>&1 </dev/null & echo $!', "sh",
             str(fake_code), "-c", "sleep 600 & wait", "--new-window", str(worktrees["billing-retry"])],
            text=True,
        ).strip())
        # A window the user opened themselves: AMF has a record, but it is
        # not AMF's to close.
        foreign_editor = subprocess.Popen(
            ["sleep", "600"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        )

        # tauri.conf.json pins the frontend to Vite on 1420 (strictPort).
        try:
            with socket.create_connection(("localhost", 1420), timeout=1):
                raise RuntimeError("Port 1420 is already in use; stop the other Vite/Tauri dev server first")
        except OSError:
            pass
        vite = subprocess.Popen(
            ["npm", "run", "dev"], cwd=workspace / "gui", stdout=vite_log,
            stderr=subprocess.STDOUT, start_new_session=True,
        )
        for _ in range(100):
            if vite.poll() is not None:
                raise RuntimeError("The isolated Vite server failed; ensure port 1420 is free")
            try:
                with urllib.request.urlopen("http://localhost:1420/", timeout=1):
                    break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("The isolated frontend did not become ready")
        gui = subprocess.Popen(
            [str(workspace / "target/debug/amf-gui")], cwd=scratch, env=env,
            stdout=gui_log, stderr=subprocess.STDOUT, start_new_session=True,
        )
        dbpath = config / "amf/amf.db"
        for _ in range(60):
            if gui.poll() is not None:
                raise RuntimeError(f"The isolated GUI exited ({gui.returncode}):\n" + (out / "gui.log").read_text())
            if dbpath.exists():
                try:
                    with sqlite3.connect(dbpath) as db:
                        db.execute("SELECT id FROM launched_editors")
                    break
                except sqlite3.OperationalError:
                    pass
            time.sleep(0.25)
        else:
            raise RuntimeError("GUI database never initialized:\n" + (out / "gui.log").read_text())

        with sqlite3.connect(dbpath) as db:
            created = iso(datetime.timedelta(days=3))
            db.execute(
                "INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)",
                ("shot-project", "Billing API", str(repo), 1, created),
            )
            for index, (fid, name, hours, _) in enumerate(features):
                db.execute(
                    "INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,created_at,last_accessed,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                    (fid, "shot-project", name, fid, str(worktrees[fid]), 1, f"amf-{fid}", "idle",
                     created, iso(datetime.timedelta(hours=hours)), index),
                )
                db.execute(
                    "INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                    (f"{fid}-claude", fid, "claude", "Claude", "claude", created, 0),
                )
            for record, fid, pid, dedicated, command in [
                ("editor-owned", "billing-retry", owned_editor, 1, "code --new-window"),
                ("editor-foreign", "docs-refresh", foreign_editor.pid, 0, "code"),
            ]:
                db.execute(
                    "INSERT INTO launched_editors(id,feature_id,session_id,kind,pid,worktree_path,dedicated,command,proc_started_at,started_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                    (record, fid, None, "vscode", pid, str(worktrees[fid]), dedicated, command, "", created),
                )
            version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
            db.execute(
                "INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
                ((int(version[0]) if version else 0) + 1,),
            )

        # tmux must report the silent windows idle for over a minute.
        remaining = 65 - (time.monotonic() - tmux_started)
        if remaining > 0:
            time.sleep(remaining)

        subprocess.run(
            ["/usr/bin/python3", os.environ.get("AMF_GUI_CAPTURE_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-dormancy-frames.py")),
             str(out), str(gui.pid), inspector_address, str(dbpath), env["AMF_TMUX_SOCKET"],
             str(owned_editor), str(foreign_editor.pid)],
            env=env, check=True,
        )
        with sqlite3.connect(dbpath) as db:
            statuses = dict(db.execute("SELECT id,status FROM features").fetchall())
            assert statuses == {
                "billing-retry": "stopped", "docs-refresh": "stopped",
                "search-index": "idle", "checkout-flow": "idle",
            }, statuses
            remaining_editors = [row[0] for row in db.execute("SELECT id FROM launched_editors")]
            # The closed window's record is forgotten; the foreign one is kept
            # so `amf doctor` can still point at it.
            assert remaining_editors == ["editor-foreign"], remaining_editors
        assert foreign_editor.poll() is None, "AMF must never close a window it did not open"
        print("PASS: Owned stand-in editor closed, foreign editor and active features left running", flush=True)
    finally:
        subprocess.run(tmux + ["kill-server"], check=False, stderr=subprocess.DEVNULL)
        if owned_editor:
            # Normally already closed by AMF. Otherwise end the stand-in and
            # its child, checking it is still ours so a recycled PID is safe.
            try:
                cmdline = pathlib.Path(f"/proc/{owned_editor}/cmdline").read_bytes()
            except OSError:
                cmdline = b""
            if str(fake_code).encode() in cmdline:
                subprocess.run(["pkill", "-TERM", "-P", str(owned_editor)], check=False)
                os.kill(owned_editor, signal.SIGTERM)
        for child in [gui, vite, foreign_editor]:
            if child and child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
        gui_log.close()
        vite_log.close()
