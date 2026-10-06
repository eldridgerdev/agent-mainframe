#!/usr/bin/env python3
"""Run native GUI review AI proof against an isolated database and Git fixture.

HOME is preserved. Only owned GUI/Vite process groups are stopped on exit.
"""

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
with tempfile.TemporaryDirectory(prefix="amf-gui-review-ai-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    config = scratch / "config"
    state = scratch / "state"
    repo = scratch / "demo-api"
    (config / "amf").mkdir(parents=True)
    (state / "amf").mkdir(parents=True)
    repo.mkdir()
    env = os.environ.copy()
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        # Software rendering is reliable on WSLg and the capture-only CI job.
        # These flags affect this offline fixture GUI only.
        WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1",
        WEBKIT_DISABLE_COMPOSITING_MODE="1",
        WEBKIT_DISABLE_DMABUF_RENDERER="1",
        LIBGL_ALWAYS_SOFTWARE="1",
    )
    # CLI fixtures are private to this capture. Never call a paid harness.
    fixture_bin = scratch / "bin"
    fixture_bin.mkdir()
    for harness in ["claude", "codex", "opencode", "pi"]:
        destination = fixture_bin / harness
        shutil.copyfile(workspace / "scripts/dev/screenshot/fixtures/gui-review-ai-harness.py", destination)
        destination.chmod(0o755)
    env["PATH"] = str(fixture_bin) + os.pathsep + env["PATH"]
    env["AMF_GUI_AI_CALLS"] = str(scratch / "ai-calls.jsonl")
    pathlib.Path(env["AMF_GUI_AI_CALLS"]).write_text("")
    env["AMF_TMUX_SOCKET"] = str(scratch / "private-tmux.sock")
    # Claude discovery prefers HOME's native versions over PATH. Mount an
    # empty directory over them only inside this GUI's namespace, preserving
    # HOME and leaving the installed binaries untouched.
    empty_versions = scratch / "empty-versions"
    empty_versions.mkdir()
    gui_pid_path = scratch / "gui.pid"
    command = ["/bin/sh", "-c", 'echo "$$" > "$1"; exec "$2"', "gui-capture",
               str(gui_pid_path), str(workspace / "target/debug/amf-gui")]
    versions = pathlib.Path.home() / ".local/share/claude/versions"
    if versions.exists():
        if not shutil.which("bwrap"):
            raise RuntimeError("Masking installed Claude versions requires bubblewrap (bwrap)")
        # Preserve native display device access (ordinary --bind mounts are nodev).
        command = ["bwrap", "--die-with-parent", "--dev-bind", "/", "/",
                   "--bind", str(empty_versions), str(versions), "--", *command]
    # A clean CI HOME has no native versions to mask. PATH already selects
    # all four private fixtures, so no mount/user namespace is needed there.
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
    (repo / ".git/amf-gui-ai-fixture").write_text("offline fixture")
    # Do not accidentally capture a different Vite server on the pinned port.
    try:
        with socket.create_connection(("localhost", 1420), timeout=1):
            raise RuntimeError("Port 1420 is already in use by another process")
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
            command,
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
                    "amf-gui-shot-unused",
                    "stopped",
                    stamp,
                    stamp,
                ),
            )
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
                env.get("AMF_GUI_AI_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-review-ai-frames.py")),
                str(out),
                gui_pid_path.read_text().strip(),
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
                == "stopped"
            )
            assert (
                db.execute("SELECT count(*) FROM feature_sessions").fetchone()[0] == 0
            )
        shutil.copyfile(env["AMF_GUI_AI_CALLS"], out / "fixture-calls.jsonl")
    finally:
        for child in [gui, vite]:
            if child and child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
        # The GUI's tmux observer daemonizes onto the private socket; stop
        # that server too so no capture process outlives the run.
        subprocess.run(["tmux", "-S", env["AMF_TMUX_SOCKET"], "kill-server"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        gui_log.close()
        vite_log.close()
