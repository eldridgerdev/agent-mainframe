"""Shared JSON protocol, isolation and lifecycle for AMF development drivers."""

import contextlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
MAX_REQUEST = 1024 * 1024


def encode(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def bounded(value, limit):
    rendered = encode(value)
    if len(rendered) <= limit:
        return value
    result = {"truncated": True, "total_chars": len(rendered), "preview": ""}
    low, high = 0, len(rendered)
    while low < high:
        middle = (low + high + 1) // 2
        result["preview"] = rendered[:middle]
        if len(encode(result)) <= limit:
            low = middle
        else:
            high = middle - 1
    result["preview"] = rendered[:low]
    return result


class RequestDriver:
    def run(self, request):
        if not isinstance(request, dict):
            raise ValueError("Request must be a JSON object")
        timeout = float(request.get("timeout", 15))
        if not 0 < timeout <= 120:
            raise ValueError("timeout must be between 0 and 120 seconds")
        deadline = time.monotonic() + timeout
        if request.get("op") != "batch":
            return self.action(request, deadline)
        actions = request.get("actions")
        if not isinstance(actions, list) or not 1 <= len(actions) <= 100:
            raise ValueError("batch needs 1–100 actions")
        if not all(isinstance(action, dict) and isinstance(action.get("op"), str) for action in actions):
            raise ValueError("Each batch action must be an object with an op string")
        result = None
        for index, action in enumerate(actions):
            try:
                if time.monotonic() >= deadline:
                    raise TimeoutError("Batch deadline exceeded")
                checkpoint = self.checkpoint()
                result = self.action(action, deadline)
                if action["op"] == "snapshot" and index < len(actions) - 1:
                    # Only the final result is delivered. A hidden snapshot
                    # cannot become the basis for the next changed-row read.
                    self.restore(checkpoint)
            except Exception as error:
                raise RuntimeError(f"Batch action {index + 1} ({action.get('op')}): {error}") from error
        # Return only the last result; completed actions are never retried.
        return {"completed": len(actions), "result": result}

    def close(self):
        pass

    def checkpoint(self):
        return None

    def restore(self, checkpoint):
        pass


def dispatch(driver, request):
    checkpoint = driver.checkpoint()
    try:
        if not isinstance(request, dict):
            raise ValueError("Request must be a JSON object")
        limit = int(request.get("max_output", 12000))
        if not 256 <= limit <= 100000:
            raise ValueError("max_output must be between 256 and 100000 characters")
        result = driver.run(request)
        limited = bounded(result, limit)
        if limited is not result:
            driver.restore(checkpoint)
        return {"ok": True, "result": limited}
    except Exception as error:
        driver.restore(checkpoint)
        return {"ok": False, "error": str(error)[:2000]}


class ProcessRuntime:
    def __init__(self, session):
        self.session = session
        self.children = []
        self.logs = []
        self.env = os.environ.copy()

    def launch(self, name, args, cwd, env):
        log = (self.session / f"{name}.log").open("w")
        self.logs.append(log)
        process = subprocess.Popen(args, cwd=cwd, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        self.children.append(process)
        return process

    def prepare(self):
        home = self.session / "home"
        for directory in [home, self.session / "config", self.session / "state", self.session / "bin"]:
            directory.mkdir()
        # Usage/session discovery falls back to /home/$USER if this directory
        # is absent, even when HOME is private.
        (home / ".codex/sessions").mkdir(parents=True)
        self.env.update(
            HOME=str(home), XDG_CONFIG_HOME=str(self.session / "config"),
            XDG_STATE_HOME=str(self.session / "state"), XDG_DATA_HOME=str(home / ".local/share"),
            XDG_CACHE_HOME=str(home / ".cache"), AMF_TMUX_SOCKET=str(self.session / "tmux.sock"),
            GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
            CODEX_HOME=str(home / ".codex"),
        )
        # Default exercises cost no agent tokens. Tests can replace these
        # session-local executables with deterministic fixtures.
        for executable in ["claude", "codex", "opencode", "pi", "gh", "code"]:
            stub = self.session / "bin" / executable
            stub.write_text("#!/bin/sh\necho 'Disabled in the isolated AMF driver; install a local fixture' >&2\nexit 127\n")
            stub.chmod(0o700)
        self.env["PATH"] = str(self.session / "bin") + os.pathsep + self.env["PATH"]
        for key in ("TMUX", "TMUX_PANE", "TERMINFO", "TERMINFO_DIRS", "ENV", "BASH_ENV"):
            self.env.pop(key, None)

    def alive(self):
        return all(child.poll() is None for child in self.children)

    def postmortem(self):
        """Save evidence close() would destroy; return its file name, if any."""
        return None

    def close(self):
        for child in reversed(self.children):
            # npm's parent can exit before Vite; its process group is still
            # ours and must be stopped even if the direct child has exited.
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                pass
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        if self.env.get("AMF_TMUX_SOCKET") == str(self.session / "tmux.sock"):
            with contextlib.suppress(OSError, subprocess.TimeoutExpired):
                subprocess.run(["tmux", "-S", self.env["AMF_TMUX_SOCKET"], "kill-server"],
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
        for log in self.logs:
            log.close()


def serve(session, binary, runtime_factory):
    runtime = None
    driver = None
    stop = False

    def shutdown(_signum, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    try:
        runtime = runtime_factory(session)
        driver = runtime.start(binary, time.monotonic() + 45)
        with socket.socket(socket.AF_UNIX) as server:
            server.bind(str(session / "driver.sock"))
            server.listen(4)
            server.settimeout(1)
            (session / "ready").write_text(encode({"session": str(session), "ready": True}))
            while not stop:
                if not runtime.alive():
                    raise RuntimeError("An owned application process exited; inspect session logs")
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                with connection:
                    connection.settimeout(5)
                    checkpoint = driver.checkpoint()
                    try:
                        with connection.makefile("rb") as reader:
                            line = reader.readline(MAX_REQUEST + 1)
                        if len(line) > MAX_REQUEST:
                            raise ValueError("Request exceeds 1 MiB")
                        request = json.loads(line)
                        if isinstance(request, dict) and request.get("op") == "stop":
                            stop = True
                            response = {"ok": True, "result": {"stopped": True}}
                        else:
                            response = dispatch(driver, request)
                    except Exception as error:
                        response = {"ok": False, "error": str(error)[:2000]}
                    try:
                        connection.sendall((encode(response) + "\n").encode())
                    except OSError:
                        # A disconnected caller must not stop the app or
                        # consume an observation that it did not receive.
                        driver.restore(checkpoint)
    except KeyboardInterrupt:
        pass
    except Exception as error:
        message = str(error)
        saved = None
        if runtime is not None:
            with contextlib.suppress(Exception):
                saved = runtime.postmortem()
        if isinstance(saved, str):
            message += f"; final screen saved to {saved}"
        (session / "error").write_text(message)
    finally:
        cleanup_errors = []
        for resource in (driver, runtime):
            if resource is not None:
                try:
                    resource.close()
                except Exception as error:
                    cleanup_errors.append(str(error))
        if cleanup_errors:
            (session / "cleanup-error").write_text("; ".join(cleanup_errors))
        (session / "driver.sock").unlink(missing_ok=True)
        (session / "ready").unlink(missing_ok=True)
        (session / "stopped").touch()


def ended(session):
    """Explain a session whose daemon is gone, from the files serve() leaves."""
    if (session / "error").exists():
        return f"Session ended: {(session / 'error').read_text().strip()}; logs: {session}"
    if (session / "stopped").exists():
        return f"Session is already stopped; start a new one (logs: {session})"
    if not session.is_dir():
        return f"No session directory at {session}"
    return f"Session is not running; logs: {session}"


def call(session, request):
    payload = (encode(request) + "\n").encode()
    if len(payload) > MAX_REQUEST:
        raise ValueError("Request exceeds 1 MiB")
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(float(request.get("timeout", 15)) + 5)
        try:
            connection.connect(str(session / "driver.sock"))
        except (FileNotFoundError, ConnectionRefusedError) as error:
            raise RuntimeError(ended(session)) from error
        connection.sendall(payload)
        with connection.makefile("rb") as reader:
            line = reader.readline()
    if not line:
        # The daemon went away mid-request.
        raise RuntimeError(ended(session))
    return json.loads(line)


def session_path_fits(session):
    # AMF's reply sockets live beneath the session; keep them inside the
    # platform's Unix socket path limit.
    return len(os.fsencode(str(session / "state/amf/reply/"))) <= 75


def new_session_dir(prefix):
    # macOS's per-user temporary directory (/var/folders/…/T) alone exhausts
    # the socket path budget, so a bare `start` falls back to /tmp there.
    candidate = Path(tempfile.gettempdir()) / (prefix + "x" * 8)
    return Path(tempfile.mkdtemp(prefix=prefix, dir=None if session_path_fits(candidate) else "/tmp"))


def start_session(script, binary, session=None, prefix="amf-gui-", extra_args=()):
    session = session.resolve() if session else new_session_dir(prefix)
    if not session.exists():
        session.mkdir(mode=0o700, parents=True)
    elif any(session.iterdir()):
        raise ValueError("Session directory must be new or empty")
    session.chmod(0o700)
    if not session_path_fits(session):
        raise ValueError("Session path is too long; use a short path under /tmp")
    with (session / "driver.log").open("w") as log:
        process = subprocess.Popen([sys.executable, str(script), "_serve", str(session), str(binary), *extra_args],
                                   stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        deadline = time.monotonic() + 50
        while time.monotonic() < deadline:
            if (session / "ready").exists():
                return json.loads((session / "ready").read_text())
            if process.poll() is not None:
                error = (session / "error").read_text() if (session / "error").exists() else "Driver exited"
                raise RuntimeError(f"{error}; logs: {session}")
            time.sleep(0.1)
        raise TimeoutError(f"Application startup timed out; logs: {session}")
    except BaseException:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=15)
        raise


def stop_session(session):
    response = call(session, {"op": "stop"})
    if response["ok"]:
        deadline = time.monotonic() + 15
        while not (session / "stopped").exists():
            if time.monotonic() >= deadline:
                raise TimeoutError("Shutdown is still running; inspect driver.log")
            time.sleep(0.1)
        if (session / "cleanup-error").exists():
            raise RuntimeError("Cleanup failed: " + (session / "cleanup-error").read_text())
    return response
