#!/usr/bin/env python3
"""Run the native GUI session-sidebar proof against isolated fixtures.

The GUI gets a private HOME (agent transcripts are read from it), private XDG
config/state (database, global notifications), a private tmux server and an
offline `gh` that answers only the PR sweep's read-only queries. No agent is
launched and nothing reaches GitHub. Only owned GUI/Vite process groups and the
private tmux server are stopped on exit.

One file lives outside the scratch directory: the thinking marker. AMF's hooks
and readers hard-code `/tmp/amf-thinking/<tmux session>`, so the fixture writes
one there, under a tmux session name made unique with this run's PID so it can
never be a real AMF session's marker, and removes it on exit.
"""

import json
import shlex
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
    for checkout in [repo]:
        checkout.mkdir(parents=True)
        git(checkout, "init", "-b", "main")
        (checkout / "README.md").write_text(f"# {checkout.name}\n")
        git(checkout, "add", ".")
        git(checkout, "commit", "-m", "Initial commit")
    # A GitHub-shaped remote so the shared PR sweep runs; only the offline
    # `gh` fixture ever answers for it.
    git(repo, "remote", "add", "origin", "https://github.com/acme/invoice-api.git")
    git(repo, "checkout", "-b", "release-prep")
    worktree = {"round-totals": repo / ".worktrees/round-totals"}
    git(repo, "worktree", "add", "-b", "round-totals", str(worktree["round-totals"]))

    # Agent transcripts the shared collector reads: one session at 74% of the
    # 900k default Claude window, one at 91%.
    transcripts = home / ".claude/projects" / encode(worktree["round-totals"])
    transcripts.mkdir(parents=True)
    now = datetime.now(timezone.utc).isoformat()
    for conversation, used in [("sess-round", 666_000)]:
        line = {
            "type": "assistant", "timestamp": now, "sessionId": conversation, "requestId": f"req-{conversation}",
            "message": {"id": f"msg-{conversation}", "model": "claude-sonnet-4-5", "usage": {
                "input_tokens": used - 20_000, "cache_read_input_tokens": 20_000,
                "cache_creation_input_tokens": 0, "output_tokens": 3_000,
            }},
        }
        (transcripts / f"{conversation}.jsonl").write_text(json.dumps(line) + "\n")
    # Real collector inputs; private HOME has no OAuth credentials, so no
    # network usage request is possible. Codex quota comes from its rollout.
    prompt = "Round invoice totals, check negative amounts and preserve the existing API."
    transcript = transcripts / "sess-round.jsonl"
    transcript.write_text(json.dumps({"type": "user", "timestamp": now,
        "message": {"content": prompt}}) + "\n" + transcript.read_text())
    tasks = home / ".claude/tasks/sess-round"
    tasks.mkdir(parents=True)
    for index, (title, status) in enumerate([
        ("Read the invoice API", "completed"), ("Round negative totals", "in_progress"),
        ("Capture the session sidebar", "pending"),
    ], 1):
        (tasks / f"{index}.json").write_text(json.dumps({"id": str(index), "subject": title, "status": status}))
    (worktree["round-totals"] / "AMF_PLAN.md").write_text("# Invoice rounding plan\n\n- [x] Read the API\n- [ ] Round negative totals\n- [ ] Capture the sidebar\n")
    codex_id = "sidebar-codex-fixture"
    codex_dir = home / ".codex/sessions" / datetime.now().strftime("%Y/%m/%d")
    codex_dir.mkdir(parents=True)
    limits = {"primary": {"used_percent": 62, "window_minutes": 300,
        "resets_at": int(time.time()) + 10800}, "secondary": {"used_percent": 91,
        "window_minutes": 10080, "resets_at": int(time.time()) + 172800}}
    events = [
        {"type": "session_meta", "payload": {"id": codex_id, "cwd": str(worktree["round-totals"]), "model": "gpt-5-codex"}},
        {"type": "event_msg", "payload": {"type": "user_message", "message": "Check the rounding edge cases and report the results."}},
        {"type": "event_msg", "payload": {"type": "token_count", "rate_limits": limits,
            "info": {"model_context_window": 200000,
                "total_token_usage": {"input_tokens": 147000, "cached_input_tokens": 20000, "output_tokens": 1000, "total_tokens": 148000},
                "last_token_usage": {"input_tokens": 147000, "output_tokens": 1000, "total_tokens": 148000}}}},
    ]
    (codex_dir / f"rollout-{codex_id}.jsonl").write_text("".join(json.dumps(event | {"timestamp": now}) + "\n" for event in events))
    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    # PID-suffixed: the name keys the shared thinking marker below.
    tmux_name = f"amf-session-sidebar-round-totals-{os.getpid()}"
    harness = str(workspace / "scripts/dev/screenshot/fixtures/gui-scroll-harness.py")

    def offline_window(window):
        received_dir = scratch / window
        received_dir.mkdir()
        return shlex.join(["/usr/bin/python3", harness, "transcript", str(received_dir)])

    if os.environ.get("AMF_GUI_FRESH_CONTEXT") == "1":
        (config / "amf/config.json").write_text(json.dumps({
            "low_memory_warn_mb": 0, "max_concurrent_agents": 100,
        }))
        received_dir = scratch / "fresh-context"
        received_dir.mkdir()
        fake_claude = fixture_bin / "claude"
        fake_claude.write_text("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'Claude Code 2.0.0'; exit 0; fi\n" +
            "exec " + shlex.join(["/usr/bin/python3", harness, "transcript", str(received_dir)]) + "\n")
        fake_claude.chmod(0o755)
        # The shared launcher prefers native versions and returns this absolute
        # fixture path, independent of the tmux server's inherited PATH.
        versions = home / ".local/share/claude/versions"
        versions.mkdir(parents=True)
        shutil.copyfile(fake_claude, versions / "offline-fixture")
        (versions / "offline-fixture").chmod(0o755)

    # The hooks' thinking marker for round-totals, kept fresh the way a busy
    # agent's tool calls keep it fresh.
    marker = pathlib.Path("/tmp/amf-thinking") / tmux_name
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
        subprocess.run(tmux + ["new-session", "-d", "-s", tmux_name, "-n", "claude", "-x", "120", "-y", "30", offline_window("claude")], check=True)
        subprocess.run(tmux + ["new-window", "-d", "-t", tmux_name, "-n", "codex", offline_window("codex")], check=True)
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

        issue = json.dumps({"host": "github.com", "owner": "acme", "repository": "invoice-api",
                            "number": 42, "comment_status": {"state": "posted"}})
        features = [
            # id, project, name, branch, workdir, is_worktree, tmux, mode, review, plan,
            # status, summary, summary_at, nickname, collapsed, created, ready, issue
            ("f-round", "p-invoice", "round-totals", "round-totals", worktree["round-totals"], 1,
             tmux_name, "vibeless", 1, 1, "idle", None, None, "Round totals", 0, ago(minutes=5), 0, None),
            ("f-release", "p-invoice", "release-prep", "release-prep", repo, 0,
             f"amf-session-sidebar-release-prep-{os.getpid()}", "supervibe", 0, 0, "stopped", None, None, None, 1, ago(days=2), 1, None),
        ]
        sessions_rows = [
            ("s-claude", "f-round", "claude", "Claude 1", "claude", "sess-round", 0),
            ("s-codex", "f-round", "codex", "Codex 1", "codex", None, 0),
            ("s-release", "f-release", "pi", "Pi sparse", "pi", None, 1),
        ]
        stamp = ago(days=10)
        with sqlite3.connect(dbpath) as db:
            for index, (pid, name, path, collapsed) in enumerate([
                ("p-invoice", "Invoice API", repo, 0),
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
            for branch, number, pr_state in [("release-prep", 12, "MERGED")]:
                db.execute("INSERT INTO pr_terminal_state(repo,branch,pr_number,state,at) VALUES(?,?,?,?,?)",
                           (str(repo), branch, number, pr_state, "2026-10-01T12:00:00Z"))
            db.execute("UPDATE features SET summary=?, issue_source=? WHERE id='f-round'",
                ("Rounded totals; checking negative invoices", issue))
            db.execute("UPDATE feature_sessions SET token_usage_source=?, token_usage_source_match='exact' WHERE id='s-codex'",
                (json.dumps({"provider": "codex", "id": codex_id}),))
            db.execute("INSERT INTO todo_lists(id,project_id,scope,created_at,updated_at) VALUES(?,?,?,?,?)",
                ("sidebar-list", "p-invoice", "project", stamp, stamp))
            db.execute("INSERT INTO todos(id,list_id,title,status,agent_session_id,created_at,updated_at) VALUES(?,?,?,?,?,?,?)",
                ("sidebar-todo", "sidebar-list", "Finish: invoice rounding and capture the native session sidebar for reviewers", "in_progress", "s-claude", stamp, stamp))
            db.execute("UPDATE feature_sessions SET todo_id='sidebar-todo',todo_launched_from_menu=1 WHERE id='s-claude'")
            version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
            db.execute("INSERT OR REPLACE INTO store_meta (key,value) VALUES ('store_version',?)",
                       ((int(version[0]) if version else 0) + 1,))

        subprocess.run(
            ["/usr/bin/python3", os.environ.get("AMF_GUI_CAPTURE_FRAMES",
                                                str(workspace / "scripts/dev/screenshot/capture-gui-session-sidebar-frames.py")),
             str(out), str(gui.pid), inspector_address, str(dbpath)],
            # The driver is not AMF: it keeps the real HOME, where the capture
            # helpers' Python modules may be installed per user.
            env=os.environ.copy(), check=True,
        )
        calls = [json.loads(line) for line in gh_calls.read_text().splitlines() if line.strip()]
        assert calls, "The shared PR sweep never ran"
        assert all(call[:2] == ["api", "graphql"] for call in calls), calls
        assert all(not part.startswith("query=mutation") for call in calls for part in call), calls
        for window in ["claude", "codex"]:
            assert json.loads((scratch / window / "transcript-received.json").read_text()) == []
        if os.environ.get("AMF_GUI_FRESH_CONTEXT") == "1":
            assert json.loads((scratch / "fresh-context/transcript-received.json").read_text()) == []
        print(f"PASS: {len(calls)} offline read-only gh call(s); no input delivered to either harness fixture", flush=True)
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
