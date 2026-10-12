#!/usr/bin/env python3
"""Persistent isolated AMF TUI interaction with compact JSON; no image capture."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from ai_driver import ProcessRuntime, RequestDriver, ROOT, call, encode, serve, start_session, stop_session


def geometry(value):
    match = re.fullmatch(r"(\d+)x(\d+)", value)
    if not match or not 60 <= int(match[1]) <= 400 or not 20 <= int(match[2]) <= 200:
        raise ValueError("geometry must be COLSxROWS, with 60–400 columns and 20–200 rows")
    return int(match[1]), int(match[2])


def default_binary():
    local = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug/amf"
    if local.is_file():
        return local
    configured = os.environ.get("AMF_BIN")
    if configured and os.access(configured, os.X_OK) and Path(configured).is_file():
        return Path(configured)
    return Path(shutil.which("amf") or local)


def compact_line(value):
    # Preserve left padding (terminal columns), omit an empty right border.
    value = value.rstrip(" │┃║")
    if not value.strip(" \t│┃║┌┐└┘─━═╭╮╰╯├┤┬┴┼╞╡╪╫┝┥╟╢╤╧╔╗╚╝╠╣╦╩╬"):
        return ""
    return value


class Driver(RequestDriver):
    def __init__(self, runtime, binary):
        self.runtime = runtime
        self.binary = binary
        self.previous = None

    def checkpoint(self):
        return self.previous

    def restore(self, checkpoint):
        self.previous = checkpoint

    def frame(self, deadline):
        fields = ["pane_width", "pane_height", "cursor_x", "cursor_y", "cursor_flag", "alternate_on", "pane_in_mode", "pane_dead", "keypad_cursor_flag"]
        values = self.runtime.tmux("display-message", "-p", "-t", self.runtime.target,
                                   "\t".join("#{" + key + "}" for key in fields), deadline=deadline).strip().split("\t")
        if len(values) != len(fields):
            raise RuntimeError("Incomplete tmux pane metadata")
        meta = dict(zip(fields, (int(value or 0) for value in values)))
        if meta["pane_dead"]:
            raise RuntimeError("AMF exited; its final screen is saved to last-pane.txt in the session directory")
        if meta["pane_in_mode"]:
            raise RuntimeError("Pane is in tmux copy-mode; exit it before driving AMF")
        raw = self.runtime.tmux("capture-pane", "-p", "-t", self.runtime.target, deadline=deadline)
        lines = raw.splitlines()
        lines = (lines + [""] * meta["pane_height"])[:meta["pane_height"]]
        return meta, lines

    def snapshot(self, action, deadline):
        meta, raw = self.frame(deadline)
        start, end = int(action.get("first_row", 0)), int(action.get("last_row", len(raw) - 1))
        if not 0 <= start <= end < len(raw):
            raise ValueError("Row range is outside the current screen (rows are zero-based)")
        limit = int(action.get("text_limit", 2400))
        if not 0 <= limit <= 50000:
            raise ValueError("text_limit must be between 0 and 50000")
        lines = tuple(compact_line(line) for line in raw)
        scope = (meta["pane_width"], meta["pane_height"], start, end)
        delta = bool(action.get("changes")) and self.previous is not None and self.previous[0] == scope
        output = []
        remaining = limit
        truncated = False
        for index in range(start, end + 1):
            value = lines[index]
            if delta:
                if value == self.previous[1][index]:
                    continue
            elif not value:
                continue
            if len(value) > remaining:
                output.append({"row": index, "text": value[:remaining]})
                truncated = True
                break
            output.append({"row": index, "text": value})
            remaining -= len(value)
        if not truncated:
            self.previous = (scope, lines)
        revision = hashlib.sha256(encode([scope[:2], raw]).encode()).hexdigest()[:12]
        return {"cols": meta["pane_width"], "rows": meta["pane_height"],
                "cursor": {"col": meta["cursor_x"], "row": meta["cursor_y"], "visible": bool(meta["cursor_flag"])},
                "alternate_screen": bool(meta["alternate_on"]), "application_cursor": bool(meta["keypad_cursor_flag"]),
                "revision": revision, "delta": delta, "lines": output, "text_truncated": truncated}

    def action(self, action, deadline):
        operation = action.get("op")
        if operation == "snapshot":
            return self.snapshot(action, deadline)
        if operation == "wait":
            if not isinstance(action.get("text"), str) or not action["text"]:
                raise ValueError("wait requires nonempty literal text")
            while time.monotonic() < deadline:
                try:
                    _, lines = self.frame(deadline)
                except TimeoutError:
                    # tmux used up what was left of the deadline: the
                    # condition did not hold in time, which is what to report.
                    break
                found = action["text"] in "\n".join(lines)
                if found != bool(action.get("absent", False)):
                    return {"ok": True}
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            raise TimeoutError("Wait condition did not become true")
        if operation == "resize":
            cols, rows = geometry(action["geometry"])
            self.runtime.tmux("resize-window", "-t", self.runtime.target, "-x", str(cols), "-y", str(rows), deadline=deadline)
            self.previous = None
            return {"ok": True}
        if operation == "automation":
            command = action.get("action")
            if command not in {"create-project", "create-feature", "create-batch-features"}:
                raise ValueError("Unsupported automation action")
            try:
                response = subprocess.run([str(self.binary), "automation", command], input=encode(action["request"]),
                                          env=self.runtime.env, cwd=self.runtime.session, capture_output=True,
                                          text=True, timeout=max(0.01, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                raise TimeoutError(f"amf automation {command} did not finish before the deadline") from None
            if response.returncode:
                raise RuntimeError((response.stderr or response.stdout)[-1500:])
            value = json.loads(response.stdout)
            if value.get("ok") is False:
                raise RuntimeError(encode(value))
            return value
        # Re-read modes before input: never type into a dead shell or copy-mode.
        self.frame(deadline)
        if operation == "keys":
            keys = action.get("keys")
            if not isinstance(keys, list) or not 1 <= len(keys) <= 100 or not all(isinstance(key, str) and key and not key.startswith("-") for key in keys):
                raise ValueError("keys requires 1–100 tmux key names without option prefixes")
            self.runtime.tmux("send-keys", "-t", self.runtime.target, "--", *keys, deadline=deadline)
        elif operation == "text":
            value = action["text"]
            if not isinstance(value, str) or any(ord(char) < 32 or ord(char) == 127 for char in value):
                raise ValueError("text must be printable text; use keys for control keys or paste for multiline input")
            self.runtime.tmux("send-keys", "-t", self.runtime.target, "-l", "--", value, deadline=deadline)
        elif operation == "paste":
            value = action["text"]
            if not isinstance(value, str) or any(ord(char) < 32 and char not in "\n\r\t" or ord(char) == 127 for char in value):
                raise ValueError("paste allows text, newlines and tabs, but no terminal control sequences")
            # xterm.js normalizes CRLF/LF to CR. tmux paste-buffer converts LF
            # to CR, so first collapse CRLF to avoid sending a doubled CR.
            value = value.replace("\r\n", "\n")
            buffer = "amf-driver-" + uuid.uuid4().hex
            try:
                self.runtime.tmux("load-buffer", "-b", buffer, "-", input=value, deadline=deadline)
                self.runtime.tmux("paste-buffer", "-b", buffer, "-p", "-d", "-t", self.runtime.target, deadline=deadline)
            finally:
                self.runtime.tmux("delete-buffer", "-b", buffer, deadline=time.monotonic() + 1, check=False)
        else:
            raise ValueError(f"Unknown operation: {operation}")
        return {"ok": True}


class Runtime(ProcessRuntime):
    target = "driver:dashboard.0"

    def __init__(self, session, size="120x40", config=None):
        super().__init__(session)
        self.size = size
        self.config = config
        self.tmux_binary = shutil.which("tmux")
        if not self.tmux_binary:
            raise RuntimeError("tmux is required")
        self.started = False

    def tmux(self, *args, deadline, input=None, check=True):
        try:
            result = subprocess.run([self.tmux_binary, "-S", str(self.session / "tmux.sock"), "-f", "/dev/null", *args],
                                    env=self.env, cwd=self.session, input=input, capture_output=True,
                                    text=True, timeout=max(0.01, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            # One deadline covers a whole request, so a tmux call near its end
            # can get a few milliseconds. That is the request timing out, and
            # callers that retry or poll handle exactly TimeoutError.
            raise TimeoutError(f"tmux {args[0]} did not finish before the deadline") from None
        if check and result.returncode:
            raise RuntimeError(result.stderr.strip()[:1500] or "Private tmux command failed")
        return result.stdout

    def start(self, binary, deadline):
        self.prepare()
        self.env.update(TERM="xterm-256color", SHELL="/bin/sh", AMF_TMUX_BIN=self.tmux_binary)
        config = self.session / "config/amf"
        config.mkdir()
        # Import the supported legacy fixture format to bypass onboarding,
        # without declaring any real harness executable available.
        (config / "projects.json").write_text(encode({"version": 5, "projects": [], "available_harnesses": ["claude"]}))
        if self.config:
            shutil.copyfile(self.config, config / "config.json")
        cols, rows = geometry(self.size)
        # Pin the final program's environment even if tmux's shell changes PATH.
        pinned = ["HOME", "PATH", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME",
                  "AMF_TMUX_SOCKET", "AMF_TMUX_BIN", "TERM", "SHELL", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "CODEX_HOME"]
        command = "exec " + shlex.join(["env", *(f"{key}={self.env[key]}" for key in pinned), str(binary)])
        self.tmux("new-session", "-d", "-s", "driver", "-n", "dashboard", "-x", str(cols), "-y", str(rows),
                  "-c", str(self.session), command, deadline=deadline)
        self.started = True
        self.tmux("set-window-option", "-t", self.target, "remain-on-exit", "on", deadline=deadline)
        self.tmux("set-window-option", "-t", self.target, "window-size", "manual", deadline=deadline)
        driver = Driver(self, binary)
        driver.action({"op": "wait", "text": "No projects yet"}, deadline)
        # The first painted dashboard can precede the input loop's startup
        # drain. Prove responsiveness with a read-only help round trip.
        while time.monotonic() < deadline:
            self.tmux("send-keys", "-t", self.target, "?", deadline=deadline)
            try:
                driver.action({"op": "wait", "text": "Keybindings"}, min(deadline, time.monotonic() + 1))
                break
            except TimeoutError:
                continue
        else:
            raise TimeoutError("TUI did not respond to its help key")
        self.tmux("send-keys", "-t", self.target, "Escape", deadline=deadline)
        driver.action({"op": "wait", "text": "No projects yet"}, deadline)
        return driver

    def alive(self):
        if not self.started:
            return False
        try:
            return self.tmux("display-message", "-p", "-t", self.target, "#{pane_dead}",
                             deadline=time.monotonic() + 1).strip() == "0"
        except (RuntimeError, TimeoutError):
            return False

    def postmortem(self):
        # close() kills the private tmux server, and with it the remain-on-exit
        # pane that holds a panic message. Keep that screen and its scrollback.
        if not self.started:
            return None
        try:
            screen = self.tmux("capture-pane", "-p", "-S", "-", "-t", self.target, deadline=time.monotonic() + 2)
        except (RuntimeError, OSError):
            return None
        (self.session / "last-pane.txt").write_text(screen)
        return "last-pane.txt"


def parser():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--session", type=Path)
    cli.add_argument("--timeout", type=float, default=15, help="Whole request/batch deadline, maximum 120 seconds")
    cli.add_argument("--max-output", type=int, default=12000, help="Serialized result character budget")
    commands = cli.add_subparsers(dest="op", required=True)
    start = commands.add_parser("start")
    start.add_argument("--binary", type=Path, default=default_binary())
    start.add_argument("--geometry", default="120x40")
    start.add_argument("--config", type=Path)
    commands.add_parser("stop")
    snapshot = commands.add_parser("snapshot")
    snapshot.add_argument("--changes", action="store_true")
    snapshot.add_argument("--first-row", type=int)
    snapshot.add_argument("--last-row", type=int)
    snapshot.add_argument("--text-limit", type=int, default=2400)
    keys = commands.add_parser("keys")
    keys.add_argument("keys", nargs="+")
    for operation in ["text", "paste"]:
        commands.add_parser(operation).add_argument("text")
    wait = commands.add_parser("wait")
    wait.add_argument("--text", required=True)
    wait.add_argument("--absent", action="store_true")
    commands.add_parser("resize").add_argument("geometry")
    batch = commands.add_parser("batch")
    batch.add_argument("--file", type=argparse.FileType("r"), default=sys.stdin)
    automation = commands.add_parser("automation")
    automation.add_argument("action", choices=["create-project", "create-feature", "create-batch-features"])
    automation.add_argument("--file", type=argparse.FileType("r"), default=sys.stdin)
    return cli


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "_serve":
        options = json.loads(sys.argv[4])
        serve(Path(sys.argv[2]), Path(sys.argv[3]), lambda session: Runtime(session, **options))
        return
    args = parser().parse_args()
    try:
        if args.op == "start":
            binary = args.binary.resolve()
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise ValueError(f"Missing executable {binary}; supply --binary /path/to/amf or install amf on PATH")
            geometry(args.geometry)
            config = str(args.config.resolve()) if args.config else None
            if config and not Path(config).is_file():
                raise ValueError("Config file does not exist")
            print(encode(start_session(Path(__file__).resolve(), binary, args.session, "amf-tui-",
                                       [encode({"size": args.geometry, "config": config})])))
            return
        if not args.session:
            raise ValueError("Supply --session with the directory returned by start")
        request = {key: value for key, value in vars(args).items() if key not in {"session", "file"} and value is not None}
        if args.op in {"batch", "automation"}:
            request["actions" if args.op == "batch" else "request"] = json.load(args.file)
        response = stop_session(args.session.resolve()) if args.op == "stop" else call(args.session.resolve(), request)
        print(encode(response))
        if not response["ok"]:
            sys.exit(1)
    except Exception as error:
        print(encode({"ok": False, "error": str(error)[:2000]}))
        sys.exit(1)


if __name__ == "__main__":
    main()
