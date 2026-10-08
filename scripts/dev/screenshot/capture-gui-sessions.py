#!/usr/bin/env python3
"""Run native GUI proof for VS Code and configured custom sessions.

The database, config, Git repository/worktree and tmux server (its own socket)
are private to this run. The project's committed `amf.json` defines two custom
sessions: "Dev server" (Nerd Font icon, working_dir, pre_check, on_stop,
autolaunch) and "Database", whose pre_check runs `docker info`. Both `code` and
`docker` resolve to stand-ins in a private bin directory placed first on PATH:
the `docker` stand-in always reports that the daemon is down, an `npm`
stand-in prints a dev-server banner for the custom session, and the `code`
stand-in starts a stand-in "window" process with VS Code's argv shape, which
this script owns. The user's real VS Code, Docker and editor windows are never
run, recorded, signalled or killed. A second runner-owned process is recorded
as a window AMF did not open, and must survive. No agent is launched: the
feature runs a plain shell. Only owned GUI/Vite process groups, the stand-ins
and the private tmux server are stopped on exit.
"""

import datetime
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

out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]


def iso(delta):
    moment = datetime.datetime.now(datetime.timezone.utc) - delta
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


AMF_JSON = {
    "custom_sessions": [
        {
            "name": "Dev server",
            "description": "Vite with hot reload for the storefront",
            "command": "npm run dev",
            "working_dir": "web",
            "window_name": "dev-server",
            "icon": "web",
            "icon_nerd": "nf-md-web",
            "pre_check": "test -f package.json",
            "on_stop": "rm -f .vite-dev.pid",
            "autolaunch": True,
        },
        {
            "name": "Database",
            "description": "Postgres for local checkout tests",
            "command": "docker compose up db",
            "icon": "db",
            "pre_check": "docker info --format '{{.ServerVersion}}'",
        },
    ]
}

with tempfile.TemporaryDirectory(prefix="amf-gui-sessions-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    config = scratch / "config"
    state = scratch / "state"
    repo = scratch / "storefront"
    worktree = scratch / "worktrees" / "checkout-redesign"
    bin_dir = scratch / "bin"
    standin_dir = scratch / "standin"
    for directory in [config / "amf", state / "amf", repo, bin_dir, standin_dir]:
        directory.mkdir(parents=True)
    code_log = scratch / "code-calls.log"
    code_log.write_text("")

    # The stand-in window: VS Code's argv shape (a `code` binary, then
    # `--new-window <worktree>`), with a child standing in for the extension
    # host. AMF attributes it exactly as it would a real window.
    window_binary = standin_dir / "code"
    os.symlink("/bin/bash", window_binary)
    (bin_dir / "code").write_text(f"""#!/bin/bash
# Stand-in VS Code CLI for the screenshot capture. Never a real editor.
if [ "$1" = "--version" ]; then echo "1.99.0 (AMF capture stand-in)"; exit 0; fi
printf '%s\\n' "$*" >> {json.dumps(str(code_log))}
setsid {json.dumps(str(window_binary))} -c 'sleep 600 & wait' "$@" >/dev/null 2>&1 </dev/null &
exit 0
""")
    (bin_dir / "docker").write_text("""#!/bin/sh
# Stand-in Docker CLI: the daemon is always down. Never the real Docker.
echo 'Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?' >&2
exit 1
""")
    (bin_dir / "npm").write_text("""#!/bin/sh
# Stand-in npm for the custom session: a dev-server banner, then idle.
printf '\\n> storefront-web@0.1.0 dev\\n> vite\\n\\n  VITE v7.3.6  ready in 312 ms\\n\\n  Local:   http://localhost:5173/\\n  Network: use --host to expose\\n\\n'
exec sleep 3600
""")
    for tool in ["code", "docker", "npm"]:
        (bin_dir / tool).chmod(0o755)

    env = os.environ.copy()
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        AMF_TMUX_SOCKET=str(scratch / "sessions-tmux.sock"),
        PATH=f"{bin_dir}:{os.environ.get('PATH', '')}",
        SHELL="/bin/bash",
        PS1="$ ",
    )
    resolved = subprocess.run(["sh", "-c", "command -v code; command -v docker; command -v npm"], env=env,
                              capture_output=True, text=True, check=True).stdout.split()
    assert resolved == [str(bin_dir / tool) for tool in ["code", "docker", "npm"]], resolved

    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo",
        "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo",
        "GIT_COMMITTER_EMAIL": "demo@example.com",
    }
    subprocess.run(["git", "init", "-q", "-b", "main"], cwd=repo, env=git_env, check=True)
    (repo / "README.md").write_text("# Storefront\n")
    (repo / "web").mkdir()
    (repo / "web/package.json").write_text('{ "name": "storefront-web", "scripts": { "dev": "vite" } }\n')
    (repo / "amf.json").write_text(json.dumps(AMF_JSON, indent=2) + "\n")
    subprocess.run(["git", "add", "."], cwd=repo, env=git_env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "Initial"], cwd=repo, env=git_env, check=True)
    worktree.parent.mkdir()
    subprocess.run(["git", "worktree", "add", "-q", "-b", "checkout-redesign", str(worktree)],
                   cwd=repo, env=git_env, check=True)

    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    foreign_editor = None
    vite = gui = None
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    try:
        # The feature is already running a plain shell, so nothing here
        # starts it (and no agent could be launched).
        # Plain, rc-free shells: the user's own shell startup files never run.
        shell = "bash --noprofile --norc"
        subprocess.run(
            tmux + ["new-session", "-d", "-s", "amf-checkout-redesign", "-n", "shell", "-x", "120", "-y", "30",
                    "-c", str(worktree), shell],
            env=env, check=True,
        )
        subprocess.run(tmux + ["set-option", "-g", "default-command", shell], env=env, check=True)
        # A VS Code window the user opened themselves, which AMF only knows
        # of as "not ours": it must still be running at the end.
        foreign_editor = subprocess.Popen(
            ["sleep", "600"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
        )

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
            created = iso(datetime.timedelta(days=2))
            db.execute(
                "INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)",
                ("shot-project", "Storefront", str(repo), 1, created),
            )
            db.execute(
                "INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,status,collapsed,created_at,last_accessed,sort_order) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                ("checkout", "shot-project", "checkout-redesign", "checkout-redesign", str(worktree), 1,
                 "amf-checkout-redesign", "idle", 0, created, created, 0),
            )
            db.execute(
                "INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                ("checkout-shell", "checkout", "terminal", "Shell", "shell", created, 0),
            )
            db.execute(
                "INSERT INTO launched_editors(id,feature_id,session_id,kind,pid,worktree_path,dedicated,command,proc_started_at,started_at) VALUES(?,?,?,?,?,?,?,?,?,?)",
                ("editor-foreign", "checkout", None, "vscode", foreign_editor.pid, str(worktree), 0,
                 "code", "", iso(datetime.timedelta(hours=3))),
            )
            version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
            db.execute(
                "INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
                ((int(version[0]) if version else 0) + 1,),
            )

        subprocess.run(
            ["/usr/bin/python3", os.environ.get("AMF_GUI_CAPTURE_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-sessions-frames.py")),
             str(out), str(gui.pid), inspector_address, str(dbpath), env["AMF_TMUX_SOCKET"],
             str(worktree), str(code_log), str(window_binary), str(foreign_editor.pid)],
            env=env, check=True,
        )
        assert foreign_editor.poll() is None, "AMF must never close a window it did not open"
        print("PASS: Custom sessions and VS Code verified with stand-ins; the foreign window was left running", flush=True)
    finally:
        subprocess.run(tmux + ["kill-server"], check=False, stderr=subprocess.DEVNULL)
        # Any stand-in window AMF did not close: identified by our own
        # binary path, so nothing else can match.
        for proc in pathlib.Path("/proc").iterdir():
            if not proc.name.isdigit():
                continue
            try:
                cmdline = (proc / "cmdline").read_bytes()
            except OSError:
                continue
            if cmdline.startswith(str(window_binary).encode() + b"\0"):
                subprocess.run(["pkill", "-TERM", "-P", proc.name], check=False)
                try:
                    os.kill(int(proc.name), signal.SIGTERM)
                except OSError:
                    pass
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
