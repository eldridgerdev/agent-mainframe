#!/usr/bin/env python3
"""Run native GUI issue browser proof against private SQLite, Git and tmux fixtures.

`gh` is an offline fixture that refuses every GitHub write, and no harness is
launched: the issue feature is created stopped. HOME and the user's database,
repositories and tmux server are untouched.
"""

import datetime
import os
import json
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

with tempfile.TemporaryDirectory(prefix="amf-gui-issue-browser-") as temporary:
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
    # An offline `gh` first on PATH. Nothing here can reach GitHub.
    fixture_bin = scratch / "bin"
    fixture_bin.mkdir()
    gh = fixture_bin / "gh"
    shutil.copyfile(workspace / "scripts/dev/screenshot/fixtures/gui-issue-browser-gh.py", gh)
    gh.chmod(0o755)
    gh_state = scratch / "gh-state"
    gh_state.mkdir()
    (gh_state / "mode.txt").write_text("ok\n")
    topics = [
        ("Retry failed invoice webhooks", ["bug", "billing"], "Webhook deliveries that time out are never retried, so customers miss invoices.\n\nSteps: 1. Point a webhook at a slow endpoint. 2. Create an invoice."),
        ("Round refund totals to cents", ["bug"], "Refunds of negative amounts are off by a cent."),
        ("Document the CSV export", ["docs", "good first issue"], "The export columns are not described anywhere in the README."),
        ("Add a dark theme to the portal", ["enhancement"], "Customers asked for a dark theme."),
        ("Tax rate rounding differs in the PDF", ["bug", "billing"], "PDF totals disagree with the API by one cent."),
    ]
    issues = []
    for index in range(28):
        title, labels, body = topics[index] if index < len(topics) else (f"Housekeeping task {index + 1}", ["chore"], f"Routine cleanup item {index + 1}.")
        issues.append({"number": 101 + index, "title": title, "body": body, "state": "open",
                       "html_url": f"https://github.com/acme/billing-api/issues/{101 + index}",
                       "labels": [{"name": name} for name in labels],
                       "updated_at": f"2026-10-{30 - min(index, 20):02d}T10:00:00Z"})
    # The issues endpoint also serves pull requests; the browser must hide them.
    issues.insert(1, {"number": 900, "title": "Pull request masquerading as an issue", "body": "", "state": "open",
                      "html_url": "https://github.com/acme/billing-api/pull/900", "labels": [],
                      "updated_at": "2026-10-30T09:00:00Z", "pull_request": {"url": "x"}})
    (gh_state / "issues.json").write_text(json.dumps(issues))
    env["PATH"] = str(fixture_bin) + os.pathsep + env["PATH"]
    env.update(GDK_BACKEND="x11", WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_SOCKET=str(scratch / "tmux.sock"), AMF_GUI_GH_STATE=str(gh_state))
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
    git("remote", "add", "origin", "https://github.com/acme/billing-api.git")
    (repo / "README.md").write_text("# Billing API\n")
    git("add", ".")
    git("commit", "-q", "-m", "Initial")
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
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('store_version',COALESCE((SELECT CAST(value AS INTEGER) FROM store_meta WHERE key='store_version'),0)+1)")
        subprocess.run(["/usr/bin/python3", str(workspace / "scripts/dev/screenshot/capture-gui-issue-browser-frames.py"),
                        str(out), str(gui.pid), inspector, str(dbpath), env["AMF_TMUX_SOCKET"], str(gh_state), str(repo)],
                       env=env, check=True)
        shutil.copyfile(gh_state / "calls.jsonl", out / "gh-calls.jsonl")
        assert not (gh_state / "writes.jsonl").exists(), "the fixture gh saw a GitHub write"
        calls = [json.loads(line) for line in (gh_state / "calls.jsonl").read_text().splitlines()]
        assert all(call[0] in ("--version", "auth", "api") for call in calls)
        listing = subprocess.run(tmux + ["list-sessions"], capture_output=True, text=True)
        # `_amf-observer` is AMF's own helper session; a feature session is `amf-<name>`.
        started = [line for line in listing.stdout.splitlines() if line.startswith("amf-")]
        assert listing.returncode != 0 or not started, f"an agent session was started: {started!r}"
        print("PASS: only read-only gh calls, no GitHub write, no agent session", flush=True)
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
