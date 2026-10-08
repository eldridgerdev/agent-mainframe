#!/usr/bin/env python3
"""Check screenshot viewing through actual native Tauri IPC; no screenshots are captured.

HOME is preserved. No GitHub write or paid harness call is possible: `gh` and
every harness resolve to private fixtures. Only owned GUI/Vite process groups
are stopped on exit.
"""

import os
import json
import hashlib
import re
import struct
import zlib
import websocket
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
workspace = pathlib.Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix="amf-gui-screenshots-check-") as temporary:
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

    def png(color):
        def chunk(kind, data):
            return (
                struct.pack(">I", len(data))
                + kind
                + data
                + struct.pack(">I", zlib.crc32(kind + data))
            )

        return (
            b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 3, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress((b"\x00" + bytes(color) * 4) * 3))
            + chunk(b"IEND", b"")
        )

    env = os.environ.copy()
    env["AMF_SCREENSHOT_AUTH_GH"] = shutil.which("gh") or "gh"
    if any(
        flag in sys.argv[2:] for flag in ["--public-attachment", "--public-artifact"]
    ):
        # Only these optional live public read uses existing gh authentication.
        # AMF SQLite/state remain isolated; authentication is never printed.
        env["GH_CONFIG_DIR"] = os.environ.get(
            "GH_CONFIG_DIR",
            str(
                pathlib.Path(
                    os.environ.get("XDG_CONFIG_HOME", pathlib.Path.home() / ".config")
                )
                / "gh"
            ),
        )
    env.update(
        XDG_CONFIG_HOME=str(config),
        XDG_STATE_HOME=str(state),
        GDK_BACKEND="x11",
        WEBKIT_INSPECTOR_HTTP_SERVER=inspector_address,
        # Software rendering is reliable on WSLg and the native GUI checks.
        # These flags affect this offline fixture GUI only.
        WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS="1",
        WEBKIT_DISABLE_COMPOSITING_MODE="1",
        WEBKIT_DISABLE_DMABUF_RENDERER="1",
        LIBGL_ALWAYS_SOFTWARE="1",
    )
    # CLI fixtures are private to this check. Never call a paid harness.
    fixture_bin = scratch / "bin"
    fixture_bin.mkdir()
    for harness in ["claude", "codex", "opencode", "pi"]:
        destination = fixture_bin / harness
        shutil.copyfile(
            workspace / "scripts/dev/screenshot/fixtures/gui-pr-triage-harness.py",
            destination,
        )
        destination.chmod(0o755)
    browser = fixture_bin / "xdg-open"
    browser.write_text(
        "#!/usr/bin/python3\nimport os,pathlib,sys\npathlib.Path(os.environ['AMF_SCREENSHOT_BROWSER_LOG']).write_text(sys.argv[1])\n"
    )
    browser.chmod(0o755)
    env["AMF_SCREENSHOT_BROWSER_LOG"] = str(scratch / "browser-url.txt")
    gh = fixture_bin / "gh"
    fallback = scratch / "pr-triage-gh.py"
    fallback_source = (
        workspace / "scripts/dev/screenshot/fixtures/gui-pr-triage-gh.py"
    ).read_text()
    fallback_source = fallback_source.replace(
        "refunds may be off by a cent.",
        "refunds may be off by a cent.\\n\\n![Comment screenshot](./ready.png)",
    )
    fallback_source = fallback_source.replace(
        "Looking into it before changing anything.",
        'Looking into it before changing anything.\\n\\n<img alt=\\"Reply screenshot\\" src=\\"./ready.png\\" />',
    )
    fallback.write_text(fallback_source)
    shutil.copyfile(workspace / "scripts/dev/fixtures/screenshot-gh.py", gh)
    env["AMF_SCREENSHOT_GH_FALLBACK"] = str(fallback)
    gh.chmod(0o755)
    gh_state = scratch / "gh-state"
    gh_state.mkdir()
    for name in ["calls.jsonl", "writes.jsonl"]:
        (gh_state / name).write_text("")
    (gh_state / "delay.txt").write_text("0")
    (gh_state / "head.txt").write_text("c0ffee" + "0" * 34 + "\n")
    env["AMF_GUI_GH_STATE"] = str(gh_state)
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
    command = [
        "/bin/sh",
        "-c",
        'echo "$$" > "$1"; exec "$2"',
        "gui-check",
        str(gui_pid_path),
        str(workspace / "target/debug/amf-gui"),
    ]
    versions = pathlib.Path.home() / ".local/share/claude/versions"
    if versions.exists():
        if not shutil.which("bwrap"):
            raise RuntimeError(
                "Masking installed Claude versions requires bubblewrap (bwrap)"
            )
        # Preserve native display device access (ordinary --bind mounts are nodev).
        command = [
            "bwrap",
            "--die-with-parent",
            "--dev-bind",
            "/",
            "/",
            "--bind",
            str(empty_versions),
            str(versions),
            "--",
            *command,
        ]
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
    (repo / ".git/amf-gui-pr-triage-fixture").write_text("offline fixture")
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
                        db.execute("SELECT scope_id FROM screenshot_scopes")
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
        owners = []
        for session_id, label, color in [
            ("claude-session", "Claude 1", (220, 30, 30)),
            ("codex-session", "Codex 1", (30, 50, 220)),
        ]:
            owner = dict(
                version=1,
                scope_id=session_id + "-scope",
                project_id="shot-project",
                feature_id="shot-feature",
                session_id=session_id,
                project_name="Invoice API",
                feature_name="Round invoice totals",
                session_label=label,
                workdir=str(repo),
                is_worktree=False,
                created_at=stamp,
            )
            directory = (
                repo
                / ".amf/screenshots"
                / owner["feature_id"]
                / session_id
                / owner["scope_id"]
            )
            directory.mkdir(parents=True)
            (directory / "owner.json").write_text(json.dumps(owner))
            image = png(color)
            (directory / "ready.png").write_bytes(image)
            completion = dict(
                version=1,
                scope_id=owner["scope_id"],
                image_id="ready",
                file="ready.png",
                sha256=hashlib.sha256(image).hexdigest(),
                caption=label + " ready",
                captured_at=stamp,
            )
            (directory / "ready.json").write_text(json.dumps(completion))
            with sqlite3.connect(dbpath) as db:
                db.execute(
                    "INSERT INTO screenshot_scopes(scope_id,project_id,feature_id,session_id,workdir,owner_json) VALUES(?,?,?,?,?,?)",
                    (
                        owner["scope_id"],
                        owner["project_id"],
                        owner["feature_id"],
                        session_id,
                        str(repo),
                        json.dumps(owner),
                    ),
                )
            owners.append((owner, directory, completion))
        (gh_state / "ready.png").write_bytes(png((30, 180, 90)))
        for _ in range(100):
            try:
                with urllib.request.urlopen(
                    f"http://{inspector_address}/", timeout=1
                ) as response:
                    page = response.read().decode()
                inspector_path = re.search(r"/socket/[^\']+/WebPage", page).group(0)
                break
            except (OSError, AttributeError):
                time.sleep(0.1)
        else:
            raise RuntimeError("WebKit inspector did not become ready")
        ws = websocket.create_connection(
            f"ws://{inspector_address}{inspector_path}",
            timeout=15,
            suppress_origin=True,
        )
        target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]

        def evaluate(expression):
            evaluate.sequence += 1
            seq = evaluate.sequence
            request = dict(
                id=seq,
                method="Runtime.evaluate",
                params=dict(expression=expression, returnByValue=True),
            )
            ws.send(
                json.dumps(
                    dict(
                        id=seq + 10000,
                        method="Target.sendMessageToTarget",
                        params=dict(targetId=target, message=json.dumps(request)),
                    )
                )
            )
            while True:
                data = json.loads(ws.recv())
                if data.get("method") == "Target.dispatchMessageFromTarget":
                    message = json.loads(data["params"]["message"])
                    if message.get("id") == seq:
                        assert not message.get("error"), message
                        assert not message["result"].get("wasThrown"), message
                        return message["result"]["result"].get("value")

        evaluate.sequence = 0

        def wait(expression):
            for _ in range(150):
                if evaluate(expression):
                    return
                time.sleep(0.1)
            (out / "failed-state.txt").write_text(evaluate("document.body.innerText"))
            raise AssertionError(expression)

        def click(label):
            evaluate(
                "Array.from(document.querySelectorAll('button')).find(b=>b.textContent.trim()==="
                + json.dumps(label)
                + ").click()"
            )

        def invoke(command, args=None):
            evaluate(
                "window.checkResult=null; window.__TAURI_INTERNALS__.invoke("
                + json.dumps(command)
                + ","
                + json.dumps(args or {})
                + ").then(value=>window.checkResult={ok:true,value}).catch(error=>window.checkResult={ok:false,error});"
            )
            wait("window.checkResult!==null")
            result = evaluate("window.checkResult")
            assert result["ok"], result
            return result.get("value")

        def escape():
            evaluate(
                "window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))"
            )

        checks = []
        wait("document.body.innerText.includes('Validation screenshots')")
        click("Validation screenshots")
        wait("document.querySelectorAll('.screenshot-card').length===2")
        listing = invoke(
            "screenshots_list", dict(selection=dict(feature=None, session_id=None))
        )
        assert {i["owner"]["session_id"] for i in listing["items"]} == {
            "claude-session",
            "codex-session",
        }
        checks.append(
            "Actual IPC discovers historical Claude and Codex producing scopes"
        )
        click("Claude 1 ready")
        wait("!!document.querySelector('.screenshot-canvas img')")
        click("Original size")
        click("Zoom in")
        assert evaluate(
            "document.querySelector('.screenshot-canvas').classList.contains('screenshot-original')"
        )
        click("Next")
        wait(
            "document.querySelector('section[aria-label=\"Screenshot viewer\"]').innerText.includes('Codex 1 ready')"
        )
        escape()
        wait("document.querySelectorAll('.screenshot-card').length===2")
        assert evaluate("document.activeElement.classList.contains('screenshot-open')")
        escape()
        wait("!document.querySelector('.screenshot-grid')")
        click("Validation screenshots")
        wait("document.querySelectorAll('.screenshot-card').length===2")
        owner, directory, completion = owners[0]
        previous = completion["sha256"]
        image = png((20, 170, 40))
        (directory / "ready.png").write_bytes(image)
        completion["sha256"] = hashlib.sha256(image).hexdigest()
        (directory / "ready.json").write_text(json.dumps(completion))
        wait("document.querySelector('.screenshot-card img')!==null")
        click("Refresh screenshots")
        changed = invoke(
            "screenshots_list", dict(selection=dict(feature=None, session_id=None))
        )
        assert (
            next(i for i in changed["items"] if i["scope_id"] == owner["scope_id"])[
                "sha256"
            ]
            != previous
        )
        checks.append(
            "Viewer fit/original/zoom/navigation, focus restoration, reopening and replacement use native bridge"
        )
        escape()
        wait("!document.querySelector('.screenshot-grid')")
        click("Round invoice totals")
        wait("document.body.innerText.includes('PR Triage')")
        click("Screenshots")
        wait("document.querySelectorAll('.screenshot-card').length===2")
        evaluate(
            "window.originalInvoke=window.__TAURI_INTERNALS__.invoke; window.__TAURI_INTERNALS__.invoke=(command,args,...rest)=>{const value=window.originalInvoke(command,args,...rest);return command==='screenshots_list' && args.selection.session_id==='claude-session' ? value.then(result=>new Promise(resolve=>setTimeout(()=>resolve(result),1200))) : value;};"
        )
        evaluate(
            "(()=>{const el=document.querySelector('.screenshot-toolbar select');el.value='claude-session';el.dispatchEvent(new Event('change',{bubbles:true}));})()"
        )
        evaluate(
            "(()=>{const el=document.querySelector('.screenshot-toolbar select');el.value='';el.dispatchEvent(new Event('change',{bubbles:true}));})()"
        )
        wait("document.querySelectorAll('.screenshot-card').length===2")
        evaluate(
            "(()=>{const el=document.querySelector('.screenshot-toolbar select');el.value='codex-session';el.dispatchEvent(new Event('change',{bubbles:true}));})()"
        )
        time.sleep(1.3)
        assert evaluate(
            "document.querySelectorAll('.screenshot-card').length===1 && document.querySelector('.screenshot-card').innerText.includes('Codex 1')"
        )
        evaluate("window.__TAURI_INTERNALS__.invoke=window.originalInvoke")
        checks.append(
            "Delayed native session IPC results cannot replace a newer producing-session selection"
        )
        wait(
            "document.querySelectorAll('.screenshot-card').length===1 && document.querySelector('.screenshot-card').innerText.includes('Codex 1')"
        )
        escape()
        wait("!document.querySelector('.screenshot-grid')")
        click("PR Triage")
        wait("!!document.querySelector('.pr-entry')")
        evaluate("document.querySelector('.pr-entry').click()")
        wait("!!document.querySelector('.pr-detail')")
        click("Reply: not needed")
        wait("!!document.querySelector('.pr-detail textarea')")
        evaluate(
            "(()=>{const el=document.querySelector('.pr-detail textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,'Keep this unsent draft');el.dispatchEvent(new Event('input',{bubbles:true}));})()"
        )
        selected = evaluate(
            "document.querySelector('.pr-comment-selected')?.innerText || ''"
        )
        # Inline images load near the viewport; scroll the reply into view.
        evaluate("document.querySelector('.pr-reply .pr-inline-image').scrollIntoView({block:'center'})")
        wait("!!document.querySelector('.pr-description .pr-image-open img') && document.querySelectorAll('.pr-detail .pr-image-open img').length===2")
        assert not evaluate("!!document.querySelector('.screenshot-grid')")
        evaluate("document.querySelector('.pr-detail .pr-image-open').click()")
        wait("!!document.querySelector('.screenshot-canvas img')")
        escape()
        wait("!document.querySelector('.screenshot-canvas') && !!document.querySelector('.pr-detail')")
        assert evaluate("document.querySelector('.pr-detail textarea').value") == "Keep this unsent draft"
        assert evaluate("document.querySelector('.pr-comment-selected')?.innerText || ''") == selected
        checks.append("PR description/comment/reply images render inline through real IPC; enlarging preserves comment selection and unsent draft")
        click("Discard reply")
        wait("!document.querySelector('.pr-detail textarea')")
        escape()
        wait("!document.querySelector('.pr-detail')")
        view = invoke(
            "pr_triage_begin",
            dict(target=dict(project_id="shot-project", feature_id="shot-feature")),
        )
        view = invoke(
            "pr_triage_act",
            dict(
                workflowId=view["workflow_id"],
                revision=view["revision"],
                action=dict(kind="open", number=12),
            ),
        )
        for _ in range(100):
            view = invoke("pr_triage_snapshot", dict(workflowId=view["workflow_id"]))
            if view["stage"] == "review":
                break
            time.sleep(0.1)
        assert view["stage"] == "review"
        access = invoke(
            "screenshots_check_access",
            dict(workflowId=view["workflow_id"], source="./ready.png"),
        )
        assert [check["name"] for check in access] == [
            "GitHub account", "PR access", "Image access"
        ]
        assert all(check["passed"] for check in access)
        checks.append("Account, PR and image access diagnostics succeed through actual IPC with isolated credentials")
        (gh_state / "delay.txt").write_text("1")
        evaluate(
            "window.previousPrResult=null;window.__TAURI_INTERNALS__.invoke('screenshots_pr_document',"
            + json.dumps(
                dict(
                    workflowId=view["workflow_id"],
                )
            )
            + ").then(value=>window.previousPrResult={ok:true,value}).catch(error=>window.previousPrResult={ok:false,error});"
        )
        view = invoke(
            "pr_triage_act",
            dict(
                workflowId=view["workflow_id"],
                revision=view["revision"],
                action=dict(kind="back_to_list"),
            ),
        )
        view = invoke(
            "pr_triage_act",
            dict(
                workflowId=view["workflow_id"],
                revision=view["revision"],
                action=dict(kind="open", number=11),
            ),
        )
        for _ in range(100):
            view = invoke("pr_triage_snapshot", dict(workflowId=view["workflow_id"]))
            if view["stage"] == "review":
                break
            time.sleep(0.1)
        assert view["review"]["number"] == 11
        wait("window.previousPrResult!==null")
        assert not evaluate("window.previousPrResult.ok")
        invoke(
            "pr_triage_act",
            dict(
                workflowId=view["workflow_id"],
                revision=view["revision"],
                action=dict(kind="close"),
            ),
        )
        checks.append(
            "Delayed real source retrieval cannot apply after switching PR #12 to PR #11"
        )
        if "--public-attachment" in sys.argv[2:]:
            attachment = re.search(
                r'src="(https://github.com/user-attachments/assets/[^"]+)"',
                (workspace / "README.md").read_text(),
            ).group(1)
            (gh_state / "public-attachment.txt").write_text(attachment)
            view = invoke(
                "pr_triage_begin",
                dict(target=dict(project_id="shot-project", feature_id="shot-feature")),
            )
            view = invoke(
                "pr_triage_act",
                dict(
                    workflowId=view["workflow_id"],
                    revision=view["revision"],
                    action=dict(kind="open", number=12),
                ),
            )
            for _ in range(100):
                view = invoke(
                    "pr_triage_snapshot", dict(workflowId=view["workflow_id"])
                )
                if view["stage"] == "review":
                    break
                time.sleep(0.1)
            decoded = invoke(
                "screenshots_inline_image",
                dict(workflowId=view["workflow_id"], source=attachment),
            )
            assert decoded["data_url"].startswith("data:image/png;base64,") and decoded["width"] > 0
            invoke(
                "pr_triage_act",
                dict(
                    workflowId=view["workflow_id"],
                    revision=view["revision"],
                    action=dict(kind="close"),
                ),
            )
            (gh_state / "public-attachment.txt").unlink()
            checks.append(
                "Live public GitHub user-attachment redirects and decoding succeed through production HTTP adapter"
            )

        if "--public-artifact" in sys.argv[2:]:
            artifact_id = sys.argv[sys.argv.index("--public-artifact") + 1]
            assert artifact_id.isdecimal()
            # Live metadata and download are reads of this public repository only.
            metadata = json.loads(
                subprocess.check_output(
                    [
                        env["AMF_SCREENSHOT_AUTH_GH"],
                        "api",
                        f"repos/eldridgerdev/agent-mainframe/actions/artifacts/{artifact_id}",
                    ],
                    env=env,
                    text=True,
                )
            )
            (gh_state / "public-artifact.json").write_text(json.dumps(metadata))
            (gh_state / "head.txt").write_text(
                metadata["workflow_run"]["head_sha"] + "\n"
            )
            view = invoke(
                "pr_triage_begin",
                dict(target=dict(project_id="shot-project", feature_id="shot-feature")),
            )
            view = invoke(
                "pr_triage_act",
                dict(
                    workflowId=view["workflow_id"],
                    revision=view["revision"],
                    action=dict(kind="open", number=12),
                ),
            )
            for _ in range(100):
                view = invoke(
                    "pr_triage_snapshot", dict(workflowId=view["workflow_id"])
                )
                if view["stage"] == "review":
                    break
                time.sleep(0.1)
            sources = invoke(
                "screenshots_remote_list",
                dict(
                    workflowId=view["workflow_id"],
                    selectedRun=metadata["workflow_run"]["id"],
                    runPage=1,
                    requestId="public-artifact-probe",
                ),
            )
            artifacts = [
                item
                for item in sources["items"]
                if any("Actions ·" in label for label in item["provenance"])
            ]
            assert artifacts, sources["issues"]
            decoded = invoke(
                "screenshots_remote_image",
                dict(
                    requestId=sources["request_id"],
                    key=artifacts[0]["key"],
                    thumbnail=True,
                ),
            )
            assert decoded["data_url"].startswith("data:image/png;base64,")
            invoke("screenshots_remote_close", dict(requestId=sources["request_id"]))
            invoke(
                "pr_triage_act",
                dict(
                    workflowId=view["workflow_id"],
                    revision=view["revision"],
                    action=dict(kind="close"),
                ),
            )
            (gh_state / "public-artifact.json").unlink()
            checks.append(
                f"Live public Actions artifact {artifact_id} downloads, validates ZIP members and decodes through production adapter"
            )
        click("Validation screenshots")
        wait("document.querySelectorAll('.screenshot-card').length===2")
        click("Screenshot cleanup…")
        wait("document.body.innerText.includes('Clean up this scope…')")
        invoke("screenshots_cleanup", dict(scopeId=owners[0][0]["scope_id"]))
        click("Refresh screenshots")
        wait("document.querySelectorAll('.screenshot-card').length===1")
        assert not owners[0][1].exists() and owners[1][1].exists()
        checks.append(
            "Explicit cleanup retires/removes only selected scope, retaining its neighbor"
        )
        invoke(
            "screenshots_open_browser",
            dict(url="https://example.com/protected-gallery"),
        )
        for _ in range(100):
            if (scratch / "browser-url.txt").exists():
                break
            time.sleep(0.02)
        assert (
            scratch / "browser-url.txt"
        ).read_text() == "https://example.com/protected-gallery"
        checks.append(
            "Validated browser fallback invokes isolated system-browser fixture with the original gallery URL"
        )
        ws.close()
        (out / "acceptance.json").write_text(
            json.dumps(
                dict(
                    checks=checks,
                    private_sources="Not demonstrated; no private fixture supplied",
                ),
                indent=2,
            )
            + "\n"
        )
        print(json.dumps(dict(checks=checks)))
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
        assert (
            gh_state / "writes.jsonl"
        ).read_text() == "", "The check attempted a GitHub write"
        shutil.copyfile(env["AMF_GUI_AI_CALLS"], out / "fixture-calls.jsonl")
        shutil.copyfile(gh_state / "calls.jsonl", out / "gh-calls.jsonl")
    finally:
        if (gh_state / "calls.jsonl").exists():
            shutil.copyfile(gh_state / "calls.jsonl", out / "gh-calls.jsonl")
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
