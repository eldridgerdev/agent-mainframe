#!/usr/bin/env python3
"""Native TUI and xterm-compatible input smoke check; emits one JSON result."""

import argparse
import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import time

spec = importlib.util.spec_from_file_location("tui_driver", Path(__file__).with_name("amf-tui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def require(condition, detail):
    if not condition:
        raise RuntimeError(driver.encode(detail))


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--binary", type=Path)
    cli.add_argument("--driver", type=Path, default=Path(__file__).with_name("amf-tui-driver.py"), help="Also supports an injected copy in another project")
    args = cli.parse_args()
    script = args.driver
    command = [sys.executable, str(script), "start"]
    if args.binary:
        command += ["--binary", str(args.binary)]
    began = time.monotonic()
    launch = subprocess.run(command, capture_output=True, text=True)
    require(launch.returncode == 0, launch.stdout or launch.stderr)
    session = Path(json.loads(launch.stdout)["session"])
    metrics = {"session": str(session), "startup_seconds": round(time.monotonic() - began, 2)}

    def call(request):
        response = driver.call(session, request)
        require(response["ok"], response)
        return response["result"]

    try:
        full = call({"op": "snapshot"})
        metrics["snapshot_chars"] = len(driver.encode(full))
        unchanged = call({"op": "snapshot", "changes": True})
        require(unchanged["delta"] and unchanged["lines"] == [], unchanged)
        metrics["unchanged_snapshot_chars"] = len(driver.encode(unchanged))
        name = "TUI café 世界"
        began = time.monotonic()
        batch = call({"op": "batch", "actions": [
            {"op": "keys", "keys": ["N"]},
            {"op": "wait", "text": "New Project"},
            {"op": "paste", "text": name},
            {"op": "wait", "text": name},
            {"op": "snapshot", "first_row": 10, "last_row": 27},
        ]})
        metrics["five_action_batch_seconds"] = round(time.monotonic() - began, 2)
        metrics["dialog_snapshot_chars"] = len(driver.encode(batch["result"]))
        # The path starts blank: enter this session's private workdir, then
        # confirm through the harness field.
        call({"op": "batch", "actions": [
            {"op": "keys", "keys": ["Tab"]},
            {"op": "paste", "text": str(session)},
            {"op": "keys", "keys": ["Tab", "Enter"]},
            {"op": "wait", "text": "New Project", "absent": True},
            {"op": "wait", "text": name},
        ]})
        with sqlite3.connect(session / "config/amf/amf.db") as database:
            require(database.execute("SELECT name,repo FROM projects").fetchone() == (name, str(session)), "Project was not persisted")
        # Structured setup uses only this session's IPC socket.
        repo = session / "automation-fixture"
        repo.mkdir()
        call({"op": "automation", "action": "create-project", "request": {
            "path": str(repo), "project_name": "IPC fixture", "dry_run": False,
        }})
        call({"op": "wait", "text": "IPC fixture"})
        call({"op": "resize", "geometry": "100x30"})
        resized = call({"op": "snapshot", "changes": True})
        require((resized["cols"], resized["rows"], resized["delta"]) == (100, 30, False), resized)
        call({"op": "batch", "actions": [
            {"op": "keys", "keys": ["N"]},
            {"op": "wait", "text": "New Project"},
        ]})
        limited = call({"op": "snapshot", "max_output": 256})
        require(limited["truncated"] and len(driver.encode(limited)) <= 256, limited)
        # Neither a truncated result nor a hidden intermediate snapshot may
        # consume rows that the caller has not seen.
        recovered = call({"op": "batch", "actions": [
            {"op": "snapshot"}, {"op": "snapshot", "changes": True},
        ]})["result"]
        require(recovered["delta"] and any("New Project" in line["text"] for line in recovered["lines"]), recovered)
        call({"op": "batch", "actions": [
            {"op": "keys", "keys": ["Escape"]},
            {"op": "wait", "text": "New Project", "absent": True},
        ]})
        metrics["observation_recovery"] = "passed"
        failed = driver.call(session, {"op": "batch", "timeout": 0.2, "actions": [
            {"op": "wait", "text": "impossible fixture marker"}, {"op": "keys", "keys": ["q"]},
        ]})
        require(not failed["ok"] and "Batch action 1" in failed["error"], failed)
        call({"op": "wait", "text": "IPC fixture"})

        # Test terminal modes directly without launching an agent or browser.
        # xterm emits these same application-cursor and bracketed-paste bytes.
        probe = session / "input-probe.py"
        received = session / "input-bytes.json"
        probe.write_text("""import json,os,sys,tty
tty.setraw(sys.stdin.fileno())
sys.stdout.write('\\x1b[?1049h\\x1b[?1h\\x1b[?2004h\\x1b[2J\\x1b[H\\x1b[31mXTERM café 世界\\x1b[0m\\r\\nREADY')
sys.stdout.flush()
data=b''
while b'DONE!' not in data:
    data+=os.read(sys.stdin.fileno(),4096)
with open(sys.argv[1],'w') as output: json.dump(data.hex(),output)
sys.stdout.write('\\r\\nRECEIVED');sys.stdout.flush()
import time;time.sleep(60)
""")
        runtime = driver.Runtime(session)
        runtime.tmux("new-window", "-d", "-t", "driver", "-n", "input-probe", sys.executable, str(probe), str(received), deadline=time.monotonic() + 5)
        runtime.target = "driver:input-probe.0"
        terminal = driver.Driver(runtime, Path("/unused"))
        terminal.run({"op": "wait", "text": "READY"})
        frame = terminal.run({"op": "snapshot"})
        require(frame["alternate_screen"] and frame["application_cursor"], frame)
        require(any("XTERM café 世界" in line["text"] for line in frame["lines"]), frame)
        require(all("\x1b" not in line["text"] for line in frame["lines"]), "ANSI leaked into observation")
        terminal.run({"op": "batch", "actions": [
            {"op": "keys", "keys": ["Up"]},
            {"op": "paste", "text": "one\n世界\r\nthree"},
            {"op": "text", "text": "DONE!"},
            {"op": "wait", "text": "RECEIVED"},
        ]})
        data = bytes.fromhex(json.loads(received.read_text()))
        expected = b"\x1bOA\x1b[200~" + "one\r世界\rthree".encode() + b"\x1b[201~DONE!"
        require(data == expected, {"input_hex": data.hex(), "expected_hex": expected.hex()})
        metrics["xterm_input_modes"] = "passed"
    finally:
        response = driver.stop_session(session)
        require(response["ok"], response)
    require(not (session / "driver.sock").exists(), "Driver socket survived shutdown")
    tmux = subprocess.run(["tmux", "-S", str(session / "tmux.sock"), "list-sessions"], capture_output=True)
    require(tmux.returncode != 0, "Private tmux survived shutdown")
    print(driver.encode({"ok": True, "native_workflow": "passed", "cleanup": "passed", **metrics}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(driver.encode({"ok": False, "error": str(error)}))
        sys.exit(1)
