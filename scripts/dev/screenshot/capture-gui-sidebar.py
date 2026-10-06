#!/usr/bin/env python3
"""Run the native GUI sidebar-parity proof against isolated fixtures.

The GUI gets a private HOME (agent transcripts are read from it), private XDG
config/state (database, global notifications), a private tmux server and an
offline `gh` that answers only the PR sweep's read-only queries. No agent is
launched and nothing reaches GitHub. Only owned GUI/Vite process groups and the
private tmux server are stopped on exit.
"""

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
import threading
import time
import urllib.request
from datetime import datetime, timedelta, timezone

out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]


def ago(**delta):
    return (datetime.now(timezone.utc) - timedelta(**delta)).isoformat()


def encode(path):
    return "".join(c if c.isalnum() and c.isascii() else "-" for c in str(path))


with tempfile.TemporaryDirectory(prefix="amf-gui-sidebar-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    home = scratch / "home"
    config = scratch / "config"
    state = scratch / "state"
    fixture_bin = scratch / "bin"
    for directory in [home, config / "amf/notifications", state / "amf", fixture_bin]:
        directory.mkdir(parents=True)
    shutil.copyfile(workspace / "scripts/dev/screenshot/fixtures/gui-sidebar-gh.py", fixture_bin / "gh")
    (fixture_bin / "gh").chmod(0o755)
    gh_calls = scratch / "gh-calls.jsonl"
    gh_calls.write_text("")
    env = os.environ.copy()
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        XDG_DATA_HOME=str(home / ".local/share"),
        XDG_CACHE_HOME=str(home / ".cache"),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        AMF_TMUX_SOCKET=str(scratch / "sidebar-tmux.sock"),
        AMF_GUI_GH_CALLS=str(gh_calls),
        PATH=str(fixture_bin) + os.pathsep + os.environ["PATH"],
    )
    git_env = env | {
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "AMF demo",
        "GIT_AUTHOR_EMAIL": "demo@example.com",
        "GIT_COMMITTER_NAME": "AMF demo",
        "GIT_COMMITTER_EMAIL": "demo@example.com",
    }

    def git(cwd, *args):
        return subprocess.check_output(
            ["git", *args], cwd=cwd, env=git_env, stderr=subprocess.DEVNULL, text=True
        ).strip()

    code = home / "code"
    repo = code / "invoice-api"
    docs = code / "docs-site"
    sandbox = code / "sandbox"
    for checkout in [repo, docs, sandbox]:
        checkout.mkdir(parents=True)
        git(checkout, "init", "-b", "main")
        (checkout / "README.md").write_text(f"# {checkout.name}\n")
        git(checkout, "add", ".")
        git(checkout, "commit", "-m", "Initial commit")
    # A GitHub-shaped remote so the shared PR sweep runs; only the offline
    # `gh` fixture ever answers for it.
    git(repo, "remote", "add", "origin", "https://github.com/acme/invoice-api.git")
    git(repo, "checkout", "-b", "release-prep")
    (repo / "amf.json").write_text(json.dumps({"custom_sessions": [
        {"name": "Dev server", "icon": "S", "icon_nerd": "nf-md-server", "command": "true"},
    ]}))
    worktree = {}
    for branch in ["round-totals", "fix-rounding-bug", "closed-experiment", "supervised-edit"]:
        worktree[branch] = repo / ".worktrees" / branch
        git(repo, "worktree", "add", "-b", branch, str(worktree[branch]))
    git(docs, "checkout", "-b", "landing-copy")

    # Agent transcripts the shared collector reads: one session at 74% of the
    # 900k default Claude window, one at 91%.
    transcripts = home / ".claude/projects" / encode(worktree["round-totals"])
    transcripts.mkdir(parents=True)
    now = datetime.now(timezone.utc).isoformat()
    for conversation, used in [("sess-round", 666_000), ("sess-currency", 819_000)]:
        line = {
            "type": "assistant", "timestamp": now, "sessionId": conversation, "requestId": f"req-{conversation}",
            "message": {"id": f"msg-{conversation}", "model": "claude-sonnet-4-5", "usage": {
                "input_tokens": used - 20_000, "cache_read_input_tokens": 20_000,
                "cache_creation_input_tokens": 0, "output_tokens": 3_000,
            }},
        }
        (transcripts / f"{conversation}.jsonl").write_text(json.dumps(line) + "\n")
    custom_status = worktree["round-totals"] / ".amf/session-status"
    custom_status.mkdir(parents=True)
    (custom_status / "s-dev.txt").write_text("listening on :5173\n")

    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    sessions = {
        "amf-sidebar-round-totals": ["claude", "claude-2", "shell", "dev-server"],
        "amf-sidebar-fix-rounding-bug": ["claude"],
        "amf-sidebar-supervised-edit": ["claude"],
    }
    idle = "printf 'Offline fixture window: no agent runs here.\\n'; exec sleep 100000"

    # The hooks' thinking marker for round-totals, kept fresh the way a busy
    # agent's tool calls keep it fresh.
    marker = pathlib.Path("/tmp/amf-thinking/amf-sidebar-round-totals")
    stop_marker = threading.Event()

    def keep_thinking():
        marker.parent.mkdir(exist_ok=True)
        while not stop_marker.is_set():
            marker.touch()
            stop_marker.wait(0.5)

    try:
        with socket.create_connection(("localhost", 1420), timeout=1):
            raise RuntimeError("Port 1420 is already in use; stop the other Vite/Tauri dev server first")
    except OSError:
        pass
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    vite = subprocess.Popen(["npm", "run", "dev"], cwd=workspace / "gui", stdout=vite_log,
                            stderr=subprocess.STDOUT, start_new_session=True)
    gui = None
    thinker = threading.Thread(target=keep_thinking, daemon=True)
    try:
        for name, windows in sessions.items():
            subprocess.run(tmux + ["new-session", "-d", "-s", name, "-n", windows[0], "-x", "120", "-y", "30", idle], check=True)
            for window in windows[1:]:
                subprocess.run(tmux + ["new-window", "-d", "-t", name, "-n", window, idle], check=True)
        thinker.start()
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
        gui = subprocess.Popen([str(workspace / "target/debug/amf-gui")], cwd=scratch, env=env,
                               stdout=gui_log, stderr=subprocess.STDOUT, start_new_session=True)
        dbpath = config / "amf/amf.db"
        for _ in range(60):
            if gui.poll() is not None:
                raise RuntimeError(f"The isolated GUI exited ({gui.returncode}):\n" + (out / "gui.log").read_text())
            if dbpath.exists():
                try:
                    # Wait for the newest migration's columns, not just the
                    # first tables: seeding races the remaining migrations.
                    with sqlite3.connect(dbpath) as db:
                        db.execute("SELECT issue_source FROM features LIMIT 0")
                        db.execute("SELECT stopped FROM feature_sessions LIMIT 0")
                        db.execute("SELECT repo FROM pr_terminal_state LIMIT 0")
                        db.execute("SELECT value FROM store_meta LIMIT 0")
                    break
                except sqlite3.OperationalError:
                    pass
            time.sleep(0.25)
        else:
            raise RuntimeError("GUI database never initialized:\n" + (out / "gui.log").read_text())

        # Pending requests on disk: an input request in the feature's own
        # directory, and a supervised-edit review in the global directory
        # matched by AMF session.
        local = worktree["fix-rounding-bug"] / ".claude/notifications"
        local.mkdir(parents=True)
        (local / "input.json").write_text(json.dumps({
            "type": "input-request", "message": "Agent finished and is waiting for input",
            "cwd": str(worktree["fix-rounding-bug"]),
        }))
        (config / "amf/notifications/edit.json").write_text(json.dumps({
            "type": "diff-review", "amf_session": "amf-sidebar-supervised-edit",
            "cwd": str(worktree["supervised-edit"]), "message": "Review edit to invoice.ts",
        }))

        issue = json.dumps({"host": "github.com", "owner": "acme", "repository": "invoice-api",
                            "number": 42, "comment_status": {"state": "posted"}})
        features = [
            # id, project, name, branch, workdir, is_worktree, tmux, mode, review, plan,
            # status, summary, summary_at, nickname, collapsed, created, ready, issue
            ("f-round", "p-invoice", "round-totals", "round-totals", worktree["round-totals"], 1,
             "amf-sidebar-round-totals", "vibeless", 1, 1, "idle", None, None, "Round totals", 0, ago(minutes=5), 0, None),
            ("f-fix", "p-invoice", "fix-rounding-bug", "fix-rounding-bug", worktree["fix-rounding-bug"], 1,
             "amf-sidebar-fix-rounding-bug", "vibe", 0, 0, "idle", "Investigating rounding for negative totals",
             ago(minutes=20), None, 1, ago(hours=3), 0, issue),
            ("f-release", "p-invoice", "release-prep", "release-prep", repo, 0,
             "amf-sidebar-release-prep", "supervibe", 0, 0, "stopped", None, None, None, 1, ago(days=2), 1, None),
            ("f-closed", "p-invoice", "closed-experiment", "closed-experiment", worktree["closed-experiment"], 1,
             "amf-sidebar-closed-experiment", "vibeless", 0, 0, "stopped", None, None, None, 1, ago(days=9), 0, None),
            ("f-edit", "p-invoice", "supervised-edit", "supervised-edit", worktree["supervised-edit"], 1,
             "amf-sidebar-supervised-edit", "vibeless", 0, 0, "idle", None, None, None, 1, ago(minutes=40), 0, None),
            ("f-landing", "p-docs", "landing-copy", "landing-copy", docs, 0,
             "amf-sidebar-landing-copy", "vibe", 0, 0, "stopped", None, None, None, 1, ago(days=1), 0, None),
        ]
        sessions_rows = [
            # id, feature, kind, label, window, claude id, stopped
            ("s-claude", "f-round", "claude", "Claude 1", "claude", "sess-round", 0),
            ("s-currency", "f-round", "claude", "Claude 2 · refactor currency formatting across every invoice view",
             "claude-2", "sess-currency", 0),
            ("s-shell", "f-round", "terminal", "Shell", "shell", None, 0),
            ("s-codex", "f-round", "codex", "Codex 1", "codex", None, 1),
            ("s-dev", "f-round", "custom", "Dev server", "dev-server", None, 0),
            ("s-todos", "f-round", "todos", "TODOs", "todos", None, 0),
            ("s-fix", "f-fix", "claude", "Claude 1", "claude", None, 0),
            ("s-release", "f-release", "claude", "Claude 1", "claude", None, 0),
            ("s-release-shell", "f-release", "terminal", "Shell", "shell", None, 0),
            ("s-closed", "f-closed", "claude", "Claude 1", "claude", None, 0),
            ("s-edit", "f-edit", "claude", "Claude 1", "claude", None, 0),
            ("s-landing", "f-landing", "codex", "Codex 1", "codex", None, 0),
        ]
        stamp = ago(days=10)
        with sqlite3.connect(dbpath) as db:
            for index, (pid, name, path, collapsed) in enumerate([
                ("p-invoice", "Invoice API", repo, 0), ("p-docs", "Docs site", docs, 1), ("p-sandbox", "Sandbox", sandbox, 0),
            ]):
                db.execute("INSERT INTO projects(id,name,repo,collapsed,preferred_agent,is_git,created_at,sort_order) VALUES(?,?,?,?,?,?,?,?)",
                           (pid, name, str(path), collapsed, "claude", 1, stamp, index))
            for index, row in enumerate(features):
                (fid, pid, name, branch, workdir, is_worktree, tmux_name, mode, review, plan, status,
                 summary, summary_at, nickname, collapsed, created, ready, issue_json) = row
                db.execute(
                    "INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,mode,review,plan_mode,agent,"
                    "enable_chrome,status,summary,summary_updated_at,nickname,collapsed,created_at,last_accessed,ready,sort_order,issue_source)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                    (fid, pid, name, branch, str(workdir), is_worktree, tmux_name, mode, review, plan, "claude", 0, status,
                     summary, summary_at, nickname, collapsed, created, created, ready, index, issue_json))
            for index, (sid, fid, kind, label, window, claude_id, stopped) in enumerate(sessions_rows):
                db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,claude_session_id,created_at,sort_order,stopped)"
                           " VALUES(?,?,?,?,?,?,?,?,?)", (sid, fid, kind, label, window, claude_id, stamp, index, stopped))
            for branch, number, pr_state in [("release-prep", 12, "MERGED"), ("closed-experiment", 7, "CLOSED")]:
                db.execute("INSERT INTO pr_terminal_state(repo,branch,pr_number,state,at) VALUES(?,?,?,?,?)",
                           (str(repo), branch, number, pr_state, "2026-10-01T12:00:00Z"))
            version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
            db.execute("INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
                       ((int(version[0]) if version else 0) + 1,))

        subprocess.run(
            ["/usr/bin/python3", os.environ.get("AMF_GUI_CAPTURE_FRAMES",
                                                str(workspace / "scripts/dev/screenshot/capture-gui-sidebar-frames.py")),
             str(out), str(gui.pid), inspector_address, str(dbpath)],
            # The driver is not AMF: it keeps the real HOME, where the capture
            # helpers' Python modules may be installed per user.
            env=os.environ.copy(), check=True,
        )
        calls = [json.loads(line) for line in gh_calls.read_text().splitlines() if line.strip()]
        assert calls, "The shared PR sweep never ran"
        assert all(call[:2] == ["api", "graphql"] for call in calls), calls
        assert all(not part.startswith("query=mutation") for call in calls for part in call), calls
        # Reading notifications never consumes them.
        assert (local / "input.json").exists()
        assert (config / "amf/notifications/edit.json").exists()
        print(f"PASS: {len(calls)} offline read-only gh call(s); notification files untouched", flush=True)
    finally:
        stop_marker.set()
        try:
            marker.unlink()
        except FileNotFoundError:
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
