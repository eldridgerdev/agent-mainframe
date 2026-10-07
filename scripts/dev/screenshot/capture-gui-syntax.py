#!/usr/bin/env python3
"""Run native GUI syntax-highlighting proof against an isolated database and Git fixture.

Parsers install into the private XDG config through the GUI, from GitHub.

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
with tempfile.TemporaryDirectory(prefix="amf-gui-syntax-") as temporary:
    scratch = pathlib.Path(temporary)
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector_address = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    config = scratch / "config"
    state = scratch / "state"
    repo = scratch / "invoice-api"
    (config / "amf").mkdir(parents=True)
    (state / "amf").mkdir(parents=True)
    repo.mkdir()
    env = os.environ.copy()
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
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
    # Several languages, each changed inside a multi-line construct whose
    # opening line sits outside the diff's three context lines, plus an
    # unknown extension and a binary file that must stay plain.
    files = {
        "src/lib.rs": """//! Invoice helpers shared by the API and the CLI.

/* Rounding policy, agreed with finance:
   - totals are kept in whole cents
   - tax is applied before rounding
   - halves round away from zero
   - currency symbols are never stored
   - every total is non-negative
   - credit notes are separate documents
   - the policy applies to every region
*/
pub const POLICY: &str = r#"
cents: true
rounding: half-away
regions:
  - eu
  - us
"#;

pub fn total_cents(subtotal: f64, rate: f64) -> i64 {
    let tax = subtotal * rate;
    ((subtotal + tax) * 100.0).round() as i64
}
""",
        "web/receipt.ts": """import { totalCents } from "./invoice";

export interface Receipt {
  id: string;
  cents: number;
}

const template = `
<section class="receipt">
  <h1>Receipt</h1>
  <p>Thanks for your order.</p>
  <p>Totals include tax.</p>
  <footer>Questions? Reply to this email.</footer>
</section>
`;

export function render(receipt: Receipt): string {
  return template.replace("Receipt", `Receipt ${receipt.id}`);
}
""",
        "tools/report.py": '''"""Monthly invoice report.

Reads every invoice for the month and prints
a summary grouped by customer, largest first.
Customers without invoices are left out.
Totals are integers to avoid float drift.
"""

import json
from collections import defaultdict


def summarize(rows):
    totals = defaultdict(int)
    for row in rows:
        totals[row["customer"]] += row["cents"]
    return dict(totals)
''',
        "README.md": """# Invoice API

Helpers for invoice totals.

## Usage

- Call `total_cents` with a subtotal and a rate.
- Render receipts with `render`.
""",
        "deploy/settings.weird": """[service]
name = invoice-api
replicas = 2
""",
    }
    for name, text in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_text(text)
    git("add", ".")
    git("commit", "-m", "Initial invoice API")
    git("checkout", "-b", "credit-notes")

    def edit(name, old, new):
        path = repo / name
        text = path.read_text()
        assert old in text, (name, old)
        path.write_text(text.replace(old, new))

    edit("src/lib.rs", "   - credit notes are separate documents\n", "   - credit notes carry a negative total\n   - refunds reference their invoice\n")
    edit("src/lib.rs", "  - us\n", "  - us\n  - apac\n")
    edit("src/lib.rs", "    ((subtotal + tax) * 100.0).round() as i64\n", "    let cents = ((subtotal + tax) * 100.0).round() as i64;\n    cents.max(0)\n")
    git("add", ".")
    git("commit", "-m", "Document credit notes")
    edit("web/receipt.ts", "  <footer>Questions? Reply to this email.</footer>\n", "  <p>Credit notes appear as negative totals.</p>\n  <footer>Questions? Reply to this email.</footer>\n")
    edit("web/receipt.ts", '  return template.replace("Receipt", `Receipt ${receipt.id}`);\n', '  const title = receipt.cents < 0 ? "Credit note" : "Receipt";\n  return template.replace("Receipt", `${title} ${receipt.id}`);\n')
    edit("tools/report.py", "Totals are integers to avoid float drift.\n", "Credit notes subtract from the customer total.\nTotals are integers to avoid float drift.\n")
    edit("tools/report.py", "    return dict(totals)\n", "    return dict(sorted(totals.items(), key=lambda item: -item[1]))\n")
    edit("README.md", "- Render receipts with `render`.\n", "- Render receipts with `render`.\n- Credit notes are receipts with a negative total.\n")
    edit("deploy/settings.weird", "replicas = 2\n", "replicas = 3\n")
    (repo / "logo.bin").write_bytes(b"\0logo\0image")
    git("add", "logo.bin")
    vite_log = (out / "vite.log").open("w")
    gui_log = (out / "gui.log").open("w")
    # Explicit opt-in permits capture beside a dev server from this checkout.
    # It serves frontend assets only; GUI IPC/SQLite remain in the scratch process.
    reuse_frontend = os.environ.get("AMF_GUI_CAPTURE_REUSE_FRONTEND") == "1"
    vite = None if reuse_frontend else subprocess.Popen(
        ["npm", "run", "dev"],
        cwd=workspace / "gui",
        stdout=vite_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    gui = None
    try:
        for _ in range(100):
            if vite is not None and vite.poll() is not None:
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
                    "Credit notes",
                    "credit-notes",
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
                os.environ.get("AMF_GUI_CAPTURE_FRAMES", str(workspace / "scripts/dev/screenshot/capture-gui-syntax-frames.py")),
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
                == "stopped"
            )
            assert (
                db.execute("SELECT count(*) FROM feature_sessions").fetchone()[0] == 0
            )
    finally:
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
