#!/usr/bin/env python3
"""Exercise the native GUI driver and report compact timings; captures no images."""

import argparse
import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import time


spec = importlib.util.spec_from_file_location("gui_driver", Path(__file__).with_name("amf-gui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def require(condition, detail):
    if not condition:
        raise RuntimeError(driver.encode(detail))


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--binary", type=Path, help="Debug GUI from this checkout")
    cli.add_argument("--driver", type=Path, default=Path(__file__).with_name("amf-gui-driver.py"), help="Also supports an injected copy in another project")
    cli.add_argument("--source", type=Path, help="Matching AMF source checkout")
    args = cli.parse_args()
    command = [sys.executable, str(args.driver), "start"]
    if args.binary:
        command += ["--binary", str(args.binary)]
    if args.source:
        command += ["--source", str(args.source)]
    started = time.monotonic()
    launch = subprocess.run(command, capture_output=True, text=True)
    require(launch.returncode == 0, launch.stdout or launch.stderr)
    session = Path(json.loads(launch.stdout)["session"])
    metrics = {"session": str(session), "startup_seconds": round(time.monotonic() - started, 2)}

    def call(request):
        response = driver.call(session, request)
        require(response["ok"], response)
        return response["result"]

    try:
        repo = session / "fixture-repo"
        repo.mkdir()
        subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], check=True)
        initial = call({"op": "snapshot"})
        metrics["snapshot_chars"] = len(driver.encode(initial))
        metrics["dom_chars"] = call({"op": "eval", "expression": "document.documentElement.outerHTML.length"})
        batch_start = time.monotonic()
        batch = call({"op": "batch", "actions": [
            {"op": "click", "name": "New project"},
            {"op": "wait", "text": "Repository path"},
            {"op": "fill", "name": "Repository path", "value": str(repo)},
            {"op": "fill", "name": "Project name", "value": "Native driver fixture"},
            {"op": "select", "name": "Preferred harness", "value": ""},
            {"op": "snapshot", "scope": "[role=dialog]", "text_limit": 800},
        ]})
        metrics["six_action_batch_seconds"] = round(time.monotonic() - batch_start, 2)
        snapshot = batch["result"]
        metrics["dialog_snapshot_chars"] = len(driver.encode(snapshot))
        require(next(control for control in snapshot["controls"] if control["name"] == "Project name")["value"] == "Native driver fixture", snapshot)
        path_ref = next(control for control in snapshot["controls"] if control["name"] == "Repository path")["ref"]
        call({"op": "batch", "actions": [
            {"op": "click", "name": "Create project"},
            {"op": "wait", "expression": '!document.querySelector("[role=dialog]")'},
            {"op": "wait", "text": "Native driver fixture"},
        ]})
        stale = driver.call(session, {"op": "fill", "ref": path_ref, "value": "must fail"})
        require(not stale["ok"] and "Stale reference" in stale["error"], stale)
        ambiguous = driver.call(session, {"op": "click", "selector": "button"})
        require(not ambiguous["ok"] and "visible elements" in ambiguous["error"], ambiguous)
        limited = call({"op": "eval", "expression": "JSON.stringify('quote\\\"'.repeat(10000))", "max_output": 256})
        require(limited["truncated"] and len(driver.encode(limited)) <= 256, limited)
        timeout = driver.call(session, {"op": "batch", "timeout": 0.2, "actions": [
            {"op": "wait", "expression": "false"}, {"op": "click", "name": "New project"},
        ]})
        require(not timeout["ok"] and "Batch action 1" in timeout["error"], timeout)
        with sqlite3.connect(session / "config/amf/amf.db") as database:
            row = database.execute("SELECT name,repo FROM projects").fetchone()
            require(row == ("Native driver fixture", str(repo)), row)
        subprocess.run(["tmux", "-S", str(session / "tmux.sock"), "new-session", "-d", "-s", "cleanup-fixture", "sleep 10000"], check=True)
    finally:
        shutdown = subprocess.run([sys.executable, str(Path(__file__).with_name("amf-gui-driver.py")), "--session", str(session), "stop"], capture_output=True, text=True)
        require(shutdown.returncode == 0, shutdown.stdout or shutdown.stderr)
    require(not (session / "driver.sock").exists(), "Driver socket survived shutdown")
    # tmux can leave a socket file after exit; test server liveness instead.
    tmux = subprocess.run(["tmux", "-S", str(session / "tmux.sock"), "list-sessions"], capture_output=True)
    require(tmux.returncode != 0, "Private tmux server survived shutdown")
    print(driver.encode({"ok": True, "native_workflow": "passed", "cleanup": "passed", **metrics}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(driver.encode({"ok": False, "error": str(error)}))
        sys.exit(1)
