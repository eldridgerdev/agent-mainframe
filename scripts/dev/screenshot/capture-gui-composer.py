#!/usr/bin/env python3
"""Run native GUI composer proof against an isolated database and Git fixture.

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

out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
workspace = pathlib.Path(__file__).resolve().parents[3]
with tempfile.TemporaryDirectory(prefix="amf-gui-composer-") as temporary:
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
        AMF_TMUX_SOCKET=str(scratch / "composer-tmux.sock"),
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

    git("init", "-b", "main")
    base = """export type Invoice = {
  subtotal: number;
  taxRate: number;
};

export function invoiceTotal(invoice: Invoice): number {
  const tax = invoice.subtotal * invoice.taxRate;
  return invoice.subtotal + tax;
}

export function formatTotal(total: number): string {
  return total.toFixed(2);
}
"""
    (repo / "invoice.ts").write_text(base)
    (repo / "README.md").write_text("# Invoice API\n\nSimple invoice calculations.\n")
    git("add", ".")
    git("commit", "-m", "Initial invoice API")
    git("checkout", "-b", "round-invoice-totals")
    committed = base.replace(
        "  return invoice.subtotal + tax;",
        "  const total = invoice.subtotal + tax;\n  return Math.round(total * 100) / 100;",
    )
    (repo / "invoice.ts").write_text(committed)
    git("add", ".")
    git("commit", "-m", "Round invoice totals to cents")
    (repo / "invoice.ts").write_text(
        committed.replace(
            "  return total.toFixed(2);",
            '  return new Intl.NumberFormat("en-US", {\n    style: "currency",\n    currency: "USD",\n  }).format(total);',
        )
    )
    git("mv", "README.md", "GUIDE.md")
    (repo / "invoice.test.ts").write_text(
        'import { invoiceTotal } from "./invoice";\n\nconst invoice = { subtotal: 19.99, taxRate: 0.0825 };\nconsole.assert(invoiceTotal(invoice) === 21.64);\n'
    )
    (repo / "receipt.bin").write_bytes(b"\0receipt\0image")
    git("add", "receipt.bin")
    tmux_name = "amf-gui-composer-proof"
    tmux = ["tmux", "-S", env["AMF_TMUX_SOCKET"]]
    harness = str(workspace / "scripts/dev/screenshot/fixtures/gui-composer-harness.py")
    import shlex
    def command(label, filename):
        return shlex.join(["/usr/bin/python3", harness, str(repo / filename), label])
    # tauri.conf.json's devUrl pins the GUI to Vite on 1420 (strictPort). If
    # something already serves it, the readiness probe below could succeed
    # against that server before ours exits, capturing the wrong frontend.
    try:
        with socket.create_connection(("localhost", 1420), timeout=1):
            raise RuntimeError(
                "Port 1420 is already in use; stop the other Vite/Tauri dev server first"
            )
    except OSError:
        pass
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    vite = subprocess.Popen(
        ["npm", "run", "dev"],
        cwd=workspace / "gui",
        stdout=vite_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    gui = None
    try:
        subprocess.run(tmux + ["new-session", "-d", "-s", tmux_name, "-n", "claude", "-x", "120", "-y", "30", command("Claude tab", "claude-received.json")], check=True)
        subprocess.run(tmux + ["new-window", "-d", "-t", tmux_name, "-n", "codex", command("Codex tab", "codex-received.json")], check=True)
        for _ in range(100):
            if vite.poll() is not None:
                raise RuntimeError(
                    "The isolated Vite server failed; ensure port 1420 is free"
                )
            try:
                with urllib.request.urlopen("http://localhost:1420/", timeout=1):
                    break
            except OSError:
                time.sleep(0.25)
        else:
            raise RuntimeError("The isolated frontend did not become ready")
        gui = subprocess.Popen(
            [str(workspace / "target/debug/amf-gui")],
            cwd=scratch,
            env=env,
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
            db.execute(
                "INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)",
                ("shot-project", "Invoice API", str(repo), 1, stamp),
            )
            db.execute(
                "INSERT INTO features(id,project_id,name,branch,workdir,tmux_session,status,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?)",
                (
                    "shot-feature",
                    "shot-project",
                    "Round invoice totals",
                    "round-invoice-totals",
                    str(repo),
                    tmux_name,
                    "idle",
                    stamp,
                    stamp,
                ),
            )
            for index, kind in enumerate(["claude", "codex"]):
                db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)",
                    ("shot-" + kind, "shot-feature", kind, kind.capitalize(), kind, stamp, index))
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
                os.environ.get("AMF_GUI_CAPTURE_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-composer-frames.py")),
                str(out),
                str(gui.pid),
                inspector_address,
                str(repo),
            ],
            env=env,
            check=True,
        )
        with sqlite3.connect(dbpath) as db:
            assert (
                db.execute(
                    "SELECT status FROM features WHERE id='shot-feature'"
                ).fetchone()[0]
                == "idle"
            )
            assert (
                db.execute("SELECT count(*) FROM feature_sessions").fetchone()[0] == 2
            )
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
