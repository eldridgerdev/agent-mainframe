#!/usr/bin/env python3
"""Persistent, isolated Linux Tauri driver. JSON in/out; no image capture."""

import argparse
import json
import os
from pathlib import Path
import re
import socket
import signal
import subprocess
import sys
import time
import urllib.request


ROOT = Path(__file__).resolve().parents[3]
DOM = Path(__file__).with_name("dom.js")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from ai_driver import ProcessRuntime, RequestDriver, bounded, call, encode, serve, start_session, stop_session


def port():
    with socket.socket() as handle:
        handle.bind(("127.0.0.1", 0))
        return handle.getsockname()[1]


class Inspector:
    def __init__(self, address, deadline):
        import websocket

        self.sequence = 0
        self.ws = None
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(f"http://{address}/", timeout=1) as response:
                    page = response.read().decode()
                match = re.search(r"/socket/[^'\"<>\s]+/WebPage", page)
                if match:
                    self.ws = websocket.create_connection(
                        f"ws://{address}{match.group(0)}", timeout=2, suppress_origin=True,
                    )
                    break
            except OSError:
                pass
            time.sleep(0.1)
        if self.ws is None:
            raise RuntimeError("WebKit inspector not ready (Linux debug GUI required); see gui.log")
        try:
            while time.monotonic() < deadline:
                self.ws.settimeout(max(0.01, deadline - time.monotonic()))
                event = json.loads(self.ws.recv())
                if event.get("method") == "Target.targetCreated":
                    self.target = event["params"]["targetInfo"]["targetId"]
                    return
            raise TimeoutError("WebKit did not announce its page target")
        except BaseException:
            self.close()
            raise

    def close(self):
        if self.ws:
            self.ws.close()

    def evaluate(self, expression, deadline):
        self.sequence += 1
        request = {"id": self.sequence, "method": "Runtime.evaluate", "params": {
            "expression": expression, "returnByValue": True,
        }}
        outer_id = self.sequence + 1000000
        self.ws.settimeout(max(0.01, deadline - time.monotonic()))
        self.ws.send(encode({"id": outer_id, "method": "Target.sendMessageToTarget", "params": {
            "targetId": self.target, "message": encode(request),
        }}))
        while time.monotonic() < deadline:
            self.ws.settimeout(max(0.01, deadline - time.monotonic()))
            event = json.loads(self.ws.recv())
            if event.get("id") == outer_id and event.get("error"):
                raise RuntimeError(encode(event["error"]))
            if event.get("method") != "Target.dispatchMessageFromTarget":
                continue
            message = json.loads(event["params"]["message"])
            if message.get("id") != self.sequence:
                continue
            if message.get("error"):
                raise RuntimeError(encode(message["error"]))
            result = message["result"]
            if result.get("wasThrown"):
                raise RuntimeError(result["result"].get("description", "JavaScript threw"))
            return result["result"].get("value")
        raise TimeoutError("WebKit evaluation timed out")


class Driver(RequestDriver):
    def __init__(self, inspector):
        self.inspector = inspector

    def action(self, action, deadline):
        op = action.get("op")
        if op == "eval":
            return self.inspector.evaluate(action["expression"], deadline)
        if op == "wait":
            expression = action.get("expression")
            if expression is None:
                if not isinstance(action.get("text"), str):
                    raise ValueError("wait needs text or expression")
                expression = f"window.__amfGuiDriver({encode(action | {'op': 'has_text'})})"
            while time.monotonic() < deadline:
                if self.inspector.evaluate(expression, deadline):
                    return {"ok": True}
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            raise TimeoutError("Wait condition did not become true")
        if op not in {"snapshot", "click", "fill", "select"}:
            raise ValueError(f"Unknown operation: {op}")
        return self.inspector.evaluate(f"window.__amfGuiDriver({encode(action)})", deadline)

    def close(self):
        self.inspector.close()


class Runtime(ProcessRuntime):
    def __init__(self, session, source=ROOT):
        super().__init__(session)
        self.source = Path(source)

    def start(self, binary, deadline):
        self.prepare()
        self.env.update(GDK_BACKEND="x11", WEBKIT_INSPECTOR_HTTP_SERVER=f"127.0.0.1:{port()}")
        if not self.env.get("DISPLAY"):
            raise RuntimeError("An X11 display is required; run under xvfb-run -a on headless Linux")
        # The debug binary embeds this fixed dev URL. Refuse to inspect another
        # checkout's frontend, rather than silently connecting to its server.
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", 1420)) == 0:
                raise RuntimeError("Port 1420 is occupied; stop the existing Vite server first")
        vite = self.launch("vite", ["npm", "run", "dev", "--", "--host", "127.0.0.1"], self.source / "gui", os.environ.copy())
        while time.monotonic() < deadline:
            if vite.poll() is not None:
                raise RuntimeError("Vite exited; see vite.log (run npm ci in gui first)")
            try:
                with urllib.request.urlopen("http://127.0.0.1:1420/", timeout=1):
                    break
            except OSError:
                time.sleep(0.1)
        else:
            raise TimeoutError("Vite did not become ready")
        self.launch("gui", [str(binary)], self.session, self.env)
        inspector = Inspector(self.env["WEBKIT_INSPECTOR_HTTP_SERVER"], deadline)
        try:
            driver = Driver(inspector)
            driver.run({"op": "wait", "expression": "typeof window.__TAURI_INTERNALS__?.invoke === 'function' && !!document.querySelector('nav[aria-label=Workspace]')", "timeout": max(0.01, deadline - time.monotonic())})
            inspector.evaluate(DOM.read_text(), deadline)
            return driver
        except BaseException:
            inspector.close()
            raise

def parser():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--session", type=Path, help="Session directory returned by start")
    cli.add_argument("--timeout", type=float, default=15, help="Total request/batch deadline in seconds (max 120)")
    cli.add_argument("--max-output", type=int, default=12000, help="Result character budget")
    commands = cli.add_subparsers(dest="op", required=True)
    start = commands.add_parser("start", help="Start a private GUI, Vite, HOME, database and tmux socket")
    start.add_argument("--source", type=Path, default=ROOT, help="AMF source checkout providing gui/ and its matching debug binary")
    start.add_argument("--binary", type=Path, help="Debug amf-gui binary; defaults to the source checkout's Cargo target directory")
    commands.add_parser("stop", help="Stop only this driver's owned processes; retain logs and fixtures")
    snapshot = commands.add_parser("snapshot", help="Visible text and controls; excludes xterm content")
    snapshot.add_argument("--scope", help="Unique CSS selector to narrow inspection")
    snapshot.add_argument("--text-limit", type=int, default=2400)
    snapshot.add_argument("--control-limit", type=int, default=40)
    for operation in ["click", "fill", "select"]:
        action = commands.add_parser(operation)
        targets = action.add_mutually_exclusive_group(required=True)
        targets.add_argument("--ref")
        targets.add_argument("--name", help="Exact control label; ambiguity fails")
        targets.add_argument("--selector")
        action.add_argument("--scope", help="Unique CSS selector")
        if operation != "click":
            action.add_argument("value")
    wait = commands.add_parser("wait", help="Poll in the driver, returning only success or timeout")
    condition = wait.add_mutually_exclusive_group(required=True)
    condition.add_argument("--text")
    condition.add_argument("--expression")
    wait.add_argument("--scope")
    evaluate = commands.add_parser("eval", help="Evaluate synchronous JavaScript in the native webview")
    evaluate.add_argument("expression")
    batch = commands.add_parser("batch", help="JSON array of actions; stops at first failure; returns last result")
    batch.add_argument("--file", type=argparse.FileType("r"), default=sys.stdin)
    return cli


def main():
    # Internal daemon entrypoint; session directory has already been created.
    if len(sys.argv) == 5 and sys.argv[1] == "_serve":
        source = Path(sys.argv[4])
        serve(Path(sys.argv[2]), Path(sys.argv[3]), lambda session: Runtime(session, source))
        return
    args = parser().parse_args()
    try:
        if args.op == "start":
            if sys.platform != "linux":
                raise RuntimeError("The native WebKit driver currently supports Linux/WSLg")
            import websocket  # noqa: F401 — fail before spawning anything

            source = args.source.resolve()
            if not (source / "gui/package.json").is_file():
                raise RuntimeError("GUI interaction requires an AMF source checkout; supply start --source /path/to/agent-mainframe")
            target = Path(os.environ.get("CARGO_TARGET_DIR", source / "target"))
            binary = (args.binary or target / "debug/amf-gui").resolve()
            if not binary.is_file():
                raise RuntimeError(f"Missing {binary}; run npm ci in gui and cargo build -p amf-gui --locked")
            print(encode(start_session(Path(__file__).resolve(), binary, args.session, extra_args=[str(source)])))

        else:
            if not args.session:
                raise ValueError("Supply --session with the directory returned by start")
            request = {key: value for key, value in vars(args).items() if key not in {"session", "file"} and value is not None}
            if args.op == "batch":
                request["actions"] = json.load(args.file)
            response = stop_session(args.session.resolve()) if args.op == "stop" else call(args.session.resolve(), request)
            print(encode(response))
            if not response["ok"]:
                sys.exit(1)
    except Exception as error:
        print(encode({"ok": False, "error": str(error)[:2000]}))
        sys.exit(1)


if __name__ == "__main__":
    main()
