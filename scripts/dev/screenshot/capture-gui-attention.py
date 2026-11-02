#!/usr/bin/env python3
"""Run native GUI needs-attention proof with isolated config and GUI fixtures.

HOME is preserved. Only owned GUI/Vite process groups are stopped on exit.
"""

import json
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

port = int(os.environ.get("AMF_GUI_CAPTURE_PORT", "1440"))
# Other proofs reuse this fixture through AMF_GUI_CAPTURE_FRAMES. They name
# themselves here so a failed post-check is reported under the right feature.
proof = os.environ.get("AMF_GUI_CAPTURE_PROOF", "Needs-attention list")
out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]
with tempfile.TemporaryDirectory(prefix="amf-gui-attention-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    config = scratch / "config"
    state = scratch / "state"
    data = scratch / "data"
    cache = scratch / "cache"
    repo = scratch / "demo-api"
    (config / "amf").mkdir(parents=True)
    config_path = config / "amf/config.json"
    config_path.write_text('{"dormant_idle_minutes":0,"dormant_last_accessed_hours":4,"kill_editor_on_stop":true}\n')
    (state / "amf").mkdir(parents=True)
    data.mkdir()
    cache.mkdir()
    repo.mkdir()
    env = os.environ.copy()
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        XDG_DATA_HOME=str(data),
        XDG_CACHE_HOME=str(cache),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        AMF_TMUX_SOCKET=str(scratch / "attention-tmux.sock"),
    )
    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo",
        "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo",
        "GIT_COMMITTER_EMAIL": "demo@example.com",
    }

    def git(*args):
        return subprocess.check_output(
            ["git", *args], cwd=repo, env=git_env, stderr=subprocess.DEVNULL, text=True
        ).strip()

    def make_repo(path, title):
        path.mkdir()
        def g(*args):
            return subprocess.check_output(["git", *args], cwd=path, env=git_env, stderr=subprocess.DEVNULL, text=True).strip()
        g("init", "-b", "main")
        (path / "README.md").write_text(f"# {title}\n")
        g("add", ".")
        g("commit", "-m", "Initial commit")
        return path
    # Private fixture: no real agent runs. Each tmux window runs a harness
    # stand-in that only records what is typed into it.
    repo = make_repo(scratch / "invoice-api", "Invoice API")
    repo2 = make_repo(scratch / "receipts", "Receipts")
    idle_dir = scratch / "idle-worktree"
    idle_dir.mkdir()
    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    harness = os.environ.get("AMF_GUI_CAPTURE_HARNESS", str(workspace / "scripts/dev/screenshot/fixtures/gui-composer-harness.py"))
    import shlex
    def command(label, filename):
        return shlex.join(["/usr/bin/python3", harness, str(scratch / filename), label])
    notifications = {
        repo: [("ask-1.json", {"type": "input-request", "message": "Should totals round half-up or half-even?"})],
        repo2: [
            ("stop-1.json", {"type": "stop", "message": "Finished the receipt exporter; tests pass."}),
            ("edit-1.json", {"type": "diff-review", "message": "Edit receipt.ts", "proceed_signal": str(scratch / "never-created")}),
        ],
        idle_dir: [("stale-stop.json", {"type": "stop", "message": "Left over from a stopped feature."})],
    }
    for workdir, files in notifications.items():
        (workdir / ".claude" / "notifications").mkdir(parents=True)
        for name, payload in files:
            (workdir / ".claude" / "notifications" / name).write_text(json.dumps(payload))
    # A request no feature owns: shows the unmatched-row explanation.
    (config / "amf" / "notifications").mkdir(parents=True)
    (config / "amf" / "notifications" / "orphan.json").write_text(json.dumps({"type": "input-request", "message": "From an unknown session.", "cwd": str(scratch / "nowhere"), "amf_session": "amf-unknown"}))
    import os as _os
    _os.utime(repo2 / ".claude/notifications/stop-1.json", (time.time() - 7200, time.time() - 7200))
    _os.utime(repo / ".claude/notifications/ask-1.json", (time.time() - 420, time.time() - 420))
    # The port must match the compiled devUrl (default 1420; an alternate
    # capture port needs a matching TAURI_CONFIG build override). If something
    # already serves it, the readiness probe below could succeed
    # against that server before ours exits, capturing the wrong frontend.
    try:
        with socket.create_connection(("localhost", port), timeout=1):
            raise RuntimeError(
                f"Port {port} is already in use; choose a separate capture port"
            )
    except OSError:
        pass
    fresh_python = os.environ.get("AMF_GUI_CAPTURE_FRESH_SERVER_PYTHON") == "1"
    gui_env = env.copy()
    if fresh_python:
        image = scratch / "AppDir"
        host_modules = scratch / "host-python"
        host_modules.mkdir()
        (host_modules / "amf_host_python_probe.py").write_text('value = "host-module-imported"\n')
        original_harness = pathlib.Path(harness).read_text()
        harness = str(scratch / "python-receiver.py")
        pathlib.Path(harness).write_text('import amf_host_python_probe; assert amf_host_python_probe.value == "host-module-imported"\n' + original_harness)
        gui_env.update(APPDIR=str(image), PYTHONHOME=str(image / "usr") + "/", PYTHONPATH=str(image / "usr/share/pyshared") + "/:" + str(host_modules))
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    vite = subprocess.Popen(
        ["npm", "run", "dev", "--", "--port", str(port)],
        cwd=pathlib.Path(os.environ.get("AMF_GUI_CAPTURE_FRONTEND", str(workspace / "gui"))),
        stdout=vite_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    gui = None
    try:
        for _ in range(100):
            if vite.poll() is not None:
                raise RuntimeError(
                    f"The isolated Vite server failed; ensure port {port} is free"
                )
            try:
                with urllib.request.urlopen(f"http://localhost:{port}/", timeout=1):
                    break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("The isolated frontend did not become ready")
        if fresh_python:
            gui = subprocess.Popen(
                [os.environ.get("AMF_GUI_CAPTURE_BINARY", str(workspace / "target/debug/amf-gui"))],
                cwd=scratch,
                env=gui_env,
                stdout=gui_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            for _ in range(120):
                if gui.poll() is not None:
                    raise RuntimeError("Fresh-server GUI exited before tmux startup")
                result = subprocess.run(tmux + ["show-environment", "-g", "AMF_TMUX_SOCKET"], text=True, capture_output=True)
                if result.returncode == 0:
                    break
                time.sleep(0.25)
            else:
                raise RuntimeError("GUI did not start its private tmux server")
            home = subprocess.run(tmux + ["show-environment", "-g", "PYTHONHOME"], text=True, capture_output=True)
            assert home.returncode != 0 or home.stdout == "-PYTHONHOME\n", "Bundle Python home reached tmux server"
            path = subprocess.check_output(tmux + ["show-environment", "-g", "PYTHONPATH"], text=True).strip()
            assert path == "PYTHONPATH=" + str(host_modules), "Host Python search path not preserved"
            (out / "python-environment-proof.json").write_text(json.dumps({"launch": "current-source GUI with released AppRun Python redirects", "server_started_by": "GUI before receivers", "bundle_home_removed": True, "host_python_path_preserved": True}) + "\n")
        for name, wd_label in [("amf-attn-invoice", "Invoice"), ("amf-attn-receipts", "Receipts")]:
            subprocess.run(tmux + ["new-session", "-d", "-s", name, "-n", "claude", "-x", "120", "-y", "30", command(wd_label + " agent", name + "-received.json")], check=True)
        if gui is None:
            gui = subprocess.Popen(
                [os.environ.get("AMF_GUI_CAPTURE_BINARY", str(workspace / "target/debug/amf-gui"))],
                cwd=scratch,
                env=gui_env,
                stdout=gui_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        dbpath = config / "amf/amf.db"
        for _ in range(60):
            if gui.poll() is not None:
                raise RuntimeError(
                    f"The isolated GUI exited ({gui.returncode}):\n"
                    + (out / "gui.log").read_text()
                )
            if dbpath.exists():
                try:
                    with sqlite3.connect(dbpath) as db:
                        db.execute("SELECT id FROM features")
                    break
                except sqlite3.OperationalError:
                    pass
            time.sleep(0.25)
        else:
            raise RuntimeError(
                "GUI database never initialized:\n" + (out / "gui.log").read_text()
            )
        with sqlite3.connect(dbpath) as db:
            stamp = "2026-10-02T18:00:00Z"
            for pid, pname, prepo in [("shot-inv", "Invoice API", repo), ("shot-rec", "Receipts", repo2)]:
                db.execute("INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)", (pid, pname, str(prepo), 1, stamp))
            feats = [
                ("feat-inv", "shot-inv", "Round invoice totals", "round-invoice-totals", repo, "amf-attn-invoice"),
                ("feat-rec", "shot-rec", "Add receipt export", "receipt-export", repo2, "amf-attn-receipts"),
                ("feat-idle", "shot-rec", "Parked experiment", "parked", idle_dir, "amf-attn-parked"),
            ]
            for fid, pid, fname, branch, wd, tmux_name in feats:
                status = "stopped" if fid == "feat-idle" else "idle"
                db.execute("INSERT INTO features(id,project_id,name,branch,workdir,tmux_session,status,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?)",
                    (fid, pid, fname, branch, str(wd), tmux_name, status, stamp, stamp))
                db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                    ("sess-" + fid, fid, "claude", "Claude", "claude", stamp, 0))
            version = db.execute(
                "SELECT value FROM store_meta WHERE key='store_version'"
            ).fetchone()
            db.execute(
                "INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
                ((int(version[0]) if version else 0) + 1,),
            )
        subprocess.run(
            [
                "/usr/bin/python3",
                os.environ.get("AMF_GUI_CAPTURE_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-attention-frames.py")),
                str(out),
                str(gui.pid),
                inspector_address,
                str(config_path),
            ],
            env=env,
            check=True,
        )
        # No agent was launched or answered: nothing was ever typed into a window.
        for name in ["amf-attn-invoice", "amf-attn-receipts"]:
            received = scratch / (name + "-received.json")
            if received.is_file():
                assert json.loads(received.read_text()) == [], f"{proof} submitted a prompt"
        with sqlite3.connect(dbpath) as db:
            assert db.execute("SELECT status FROM features WHERE id='feat-idle'").fetchone()[0] == "stopped"
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
