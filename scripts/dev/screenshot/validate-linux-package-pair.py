#!/usr/bin/env python3
"""Exercise two released Linux packages in private roots, never system dpkg.

The rootless dpkg database deliberately omits host dependencies. This checks
payload installation/upgrades against the host GTK/WebKit runtime, not apt's
dependency resolution or a real desktop installation. Screenshots are opt-in.
"""

import argparse
import copy
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import socket
import sqlite3
import subprocess
import tarfile
import tempfile
import time
import urllib.request

import websocket

from owned_process import stop_owned_process_group


ASSETS = {
    "0.48.0": {
        "deb": "e7b852bc0811989cceee9cbdee46c32277972e8d8721a04259b7694e1413b378",
        "gnu": "1de0b900120ad223e996d9e465e37bfa2943aa0a8a945ffb7a2dc1a82b6e22b7",
        "musl": "ca67e7fc52595ca2350cc00972a6a281866522fb02baa131393c8eba0823604f",
        "appimage": "ac30fedfa5b4a0991140de04ef2c40c03f009c71c8d9ee079823e9f82639d3dc",
    },
    "0.49.0": {
        "deb": "6eb78f71372bfd53ec6d50806f0bc4dca5ab8d480a7626818e8f3302f5d629eb",
        "gnu": "4dac7af8913d09dcbed357906503b99426168e84eeb650fef610326e71fcb6c9",
        "musl": "9dc64d8c636f9383438f12e79f93e837b4e5f24e3c8402ba7202f39347029981",
        "appimage": "aa01a96e53a7e9f67802a370142144a35561f907f11afa7ef4d60af168ae48db",
    },
}
SCRIPT_DIR = pathlib.Path(__file__).resolve().parent


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def bundle_filter(member, destination):
    # Some Python 3.10 security backports resolve symlinks from the archive
    # root instead of their parent. Check the original filesystem destinations
    # independently, then use a harmless link target for data_filter's other
    # checks. Keep the archive's original relative symlink spelling on return.
    root = os.path.realpath(destination)
    path = os.path.realpath(os.path.join(root, member.name))
    if os.path.commonpath([root, path]) != root:
        raise tarfile.OutsideDestinationError(member, path)
    if member.issym():
        if os.path.isabs(member.linkname):
            raise tarfile.AbsoluteLinkError(member)
        target = os.path.realpath(os.path.join(root, os.path.dirname(member.name), member.linkname))
        if os.path.commonpath([root, target]) != root:
            raise tarfile.LinkOutsideDestinationError(member, target)
        checked = copy.copy(member)
        checked.linkname = "."
        checked = tarfile.data_filter(checked, destination)
        checked.linkname = member.linkname
        return checked
    return tarfile.data_filter(member, destination)


def wait(check, label):
    for _ in range(150):
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {label}")


class Inspector:
    def __init__(self, address):
        def target():
            try:
                page = urllib.request.urlopen(f"http://{address}/", timeout=1).read().decode()
                return re.search(r"/socket/[^']+/WebPage", page)
            except OSError:
                return None

        path = wait(target, "packaged WebKit target").group(0)
        self.socket = websocket.create_connection(f"ws://{address}{path}", timeout=15, suppress_origin=True)
        self.target = json.loads(self.socket.recv())["params"]["targetInfo"]["targetId"]
        self.seq = 0

    def evaluate(self, expression):
        self.seq += 1
        request = {"id": self.seq, "method": "Runtime.evaluate", "params": {"expression": expression, "returnByValue": True}}
        self.socket.send(json.dumps({"id": self.seq + 10000, "method": "Target.sendMessageToTarget", "params": {"targetId": self.target, "message": json.dumps(request)}}))
        while True:
            data = json.loads(self.socket.recv())
            if data.get("method") == "Target.dispatchMessageFromTarget":
                message = json.loads(data["params"]["message"])
                if message.get("id") == self.seq:
                    assert not message.get("error"), message
                    assert not message["result"].get("wasThrown"), message
                    return message["result"]["result"].get("value")

    def invoke(self, name, args=None):
        self.evaluate(f"window.__packageResult=null;window.__TAURI_INTERNALS__.invoke({json.dumps(name)},{json.dumps(args or {})}).then(value=>window.__packageResult={{ok:true,value}},error=>window.__packageResult={{ok:false,error}})")
        result = wait(lambda: self.evaluate("window.__packageResult"), name)
        assert result["ok"], result
        return result.get("value")

    def close(self):
        self.socket.close()


class PairProof:
    def __init__(self, scratch, out, assets, capture, tui_target, gui_package):
        self.scratch, self.out, self.assets, self.capture = scratch, out, assets, capture
        self.tui_target = tui_target
        self.gui_package = gui_package
        self.root = scratch / "install"
        self.gui_binary = self.root / ("opt/amf-gui.AppImage" if gui_package == "appimage" else "usr/bin/amf-gui")
        self.config = scratch / "config/amf"
        self.config.mkdir(parents=True)
        (self.config / "config.json").write_text('{"config_version":1,"context_warning_percent":70,"context_critical_percent":85}\n')
        if gui_package == "deb":
            admin = self.root / "var/lib/dpkg"
            admin.mkdir(parents=True)
            (admin / "status").touch()
        (scratch / "tmp").mkdir()
        self.env = os.environ.copy()
        self.env.update(XDG_CONFIG_HOME=str(scratch / "config"), XDG_STATE_HOME=str(scratch / "state"), XDG_DATA_HOME=str(scratch / "data"), XDG_CACHE_HOME=str(scratch / "cache"), AMF_TMUX_SOCKET=str(scratch / "tmux.sock"), AMF_TMUX_BIN=str(self.root / "opt/amf/tmux"), GDK_BACKEND="x11")
        self.env["TMPDIR"] = str(scratch / "tmp")
        self.tmux = [str(self.root / "opt/amf/tmux"), "-S", self.env["AMF_TMUX_SOCKET"]]
        self.gui = None
        self.gui_log = None
        self.inspector = None
        self.report = []
        self.notes = []
        self.versions = {}
        self.pane_identity = None
        self.server_started = False

    def asset(self, version, kind):
        if kind in ["deb", "appimage"]:
            extension = "deb" if kind == "deb" else "AppImage"
            name = "amf-gui-x86_64-unknown-linux-gnu." + extension
        else:
            name = f"amf-x86_64-unknown-linux-{kind}.tar.gz"
        directory = self.assets / ("v" + version)
        path = directory / name
        if not path.exists():
            directory.mkdir(parents=True, exist_ok=True)
            subprocess.run(["gh", "release", "download", "v" + version, "--repo", "eldridgerdev/agent-mainframe", "--pattern", name, "--dir", str(directory)], check=True)
        assert sha256(path) == ASSETS[version][kind], f"Release asset digest mismatch: {path}"
        return path

    def install_gui(self, version):
        self.close_gui()
        before_tui = sha256(self.root / "opt/amf/amf") if "tui" in self.versions else None
        path = self.asset(version, self.gui_package)
        if self.gui_package == "appimage":
            self.gui_binary.parent.mkdir(parents=True, exist_ok=True)
            temporary = self.gui_binary.with_suffix(".new")
            shutil.copyfile(path, temporary)
            temporary.chmod(0o755)
            temporary.replace(self.gui_binary)
            self.versions["gui"] = version
            if before_tui:
                assert sha256(self.root / "opt/amf/amf") == before_tui, "GUI update changed TUI"
            return
        metadata = subprocess.check_output(["dpkg-deb", "--field", str(path)], text=True)
        assert f"Package: amf-gui\n" in metadata and f"Version: {version}\n" in metadata and "Architecture: amd64\n" in metadata
        assert all(name in metadata for name in ["tmux", "libwebkit2gtk-4.1-0", "libgtk-3-0"])
        with open(self.out / "dpkg-install.log", "a") as log:
            subprocess.run(["fakeroot", "dpkg", f"--root={self.root}", f"--log={self.out}/dpkg-actions.log", "--force-not-root", "--force-depends", "--install", str(path)], check=True, stdout=log, stderr=subprocess.STDOUT)
        installed = subprocess.check_output(["dpkg-query", f"--admindir={self.root}/var/lib/dpkg", "--show", "--showformat=${Status} ${Version}", "amf-gui"], text=True)
        assert installed == "install ok installed " + version, installed
        if before_tui:
            assert sha256(self.root / "opt/amf/amf") == before_tui, "GUI install changed TUI"
        self.versions["gui"] = version

    def install_tui(self, version):
        before_gui = sha256(self.gui_binary) if "gui" in self.versions else None
        stage = self.scratch / ("unpack-" + version)
        stage.mkdir()
        with tarfile.open(self.asset(version, self.tui_target)) as archive:
            # Official archives contain symlinks for bundled libraries. Python's
            # data filter rejects escaping paths/links and special device files.
            assert hasattr(tarfile, "data_filter"), "Python with safe tar data filter required"
            archive.extractall(stage, filter=bundle_filter)
        (self.root / "opt").mkdir(parents=True, exist_ok=True)
        current = self.root / "opt/amf"
        if current.exists():
            current.rename(self.root / ("opt/previous-amf-" + self.versions["tui"]))
        (stage / f"amf-x86_64-unknown-linux-{self.tui_target}").rename(current)
        version_output = subprocess.check_output([str(current / "amf"), "--version"], text=True)
        assert version in version_output, version_output
        assert (current / "tmux").is_file() and (current / "tmux-real").is_file()
        if before_gui:
            assert sha256(self.gui_binary) == before_gui, "TUI install changed GUI"
        self.versions["tui"] = version

    def open_gui(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            address = f"127.0.0.1:{listener.getsockname()[1]}"
        self.gui_log = open(self.out / "packaged-gui.log", "a")
        command = [str(self.gui_binary)]
        if self.gui_package == "appimage":
            command.append("--appimage-extract-and-run")
        self.gui = subprocess.Popen(command, cwd=self.scratch, env=self.env | {"WEBKIT_INSPECTOR_HTTP_SERVER": address}, stdout=self.gui_log, stderr=subprocess.STDOUT, start_new_session=True)
        self.inspector = Inspector(address)
        wait(lambda: self.inspector.evaluate("!!window.__TAURI_INTERNALS__ && !document.body.innerText.includes('Loading workspace')"), "packaged app loaded")
        if self.capture:
            self.size_window()

    def size_window(self):
        # Xvfb's 1600x1000 screen always fits. A smaller local display or its
        # window manager may clamp the request: capture at the granted size.
        x, window = self.owned_window()
        screen = x.screen()
        width, height = min(1420, screen.width_in_pixels), min(900, screen.height_in_pixels)
        window.configure(x=0, y=0, width=width, height=height)
        x.sync()
        try:
            wait(lambda: self.inspector.evaluate(f"window.innerWidth >= {width - 20}"), "native window size")
        except AssertionError:
            print(f"WARNING: window manager kept the GUI narrower than {width}px; capturing at its granted size", flush=True)
        x.close()

    def close_gui(self):
        if self.inspector:
            self.inspector.close()
            self.inspector = None
        stop_owned_process_group(self.gui)
        self.gui = None
        if self.gui_log:
            self.gui_log.close()
            self.gui_log = None
        if self.pane_identity:
            assert self.identity() == self.pane_identity, "Closing GUI stopped/replaced fixture session"

    def seed(self):
        # Create the pre-existing session before the GUI can start a tmux server.
        # AppImage's runtime environment must not become the fixture's server
        # environment: its PYTHONHOME is intended for its own bundled programs.
        self.repo = self.scratch / "repo"
        self.repo.mkdir()
        self.received = self.scratch / "received.json"
        command = shlex.join(["/usr/bin/python3", str(SCRIPT_DIR / "fixtures/gui-composer-harness.py"), str(self.received), "Released Linux packages — unpaid continuity fixture"])
        subprocess.run(self.tmux + ["new-session", "-d", "-s", "amf-package-proof", "-n", "codex", "-x", "120", "-y", "30", command], check=True)
        self.server_started = True
        wait(self.received.exists, "pre-existing unpaid receiver ready")
        self.pane_identity = self.identity()
        self.open_gui()
        self.inspector.invoke("get_snapshot")
        self.close_gui()
        with sqlite3.connect(self.config / "amf.db") as db:
            stamp = "2026-10-11T17:00:00Z"
            db.execute("INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)", ("package-project", "Linux package continuity", str(self.repo), 0, stamp))
            db.execute("INSERT INTO features(id,project_id,name,branch,workdir,tmux_session,status,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?)", ("package-feature", "package-project", "Preserve this live session", "", str(self.repo), "amf-package-proof", "idle", stamp, stamp))
            db.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)", ("package-session", "package-feature", "codex", "Unpaid continuity fixture", "codex", stamp, 0))
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('available_harnesses','[\"codex\"]')")
            self.initial_schema = db.execute("SELECT MAX(version) FROM schema_version").fetchone()[0]

    def identity(self):
        value = subprocess.check_output(self.tmux + ["display-message", "-p", "-t", "amf-package-proof:codex", "#{session_id}:#{pane_id}:#{pane_pid}:#{pid}"], text=True).strip()
        # display-message can succeed with empty target fields for a missing
        # session. Such a result never proves a live pane survived an upgrade.
        assert re.fullmatch(r"\$\d+:%\d+:[1-9]\d*:[1-9]\d*", value), f"Missing live fixture identity: {value!r}"
        return value

    def tui_smoke(self):
        self.close_gui()
        command = shlex.join(["env", *[f"{name}={value}" for name, value in self.env.items() if name.startswith("XDG_") or name.startswith("AMF_TMUX_")], str(self.root / "opt/amf/amf")])
        subprocess.run(self.tmux + ["new-session", "-d", "-s", "package-tui", "-x", "140", "-y", "40", command], cwd=self.scratch, check=True)
        try:
            def loaded():
                pane = subprocess.check_output(self.tmux + ["capture-pane", "-p", "-t", "package-tui"], text=True)
                return pane if "Linux package continuity" in pane and "Preserve this live session" in pane else None
            pane = wait(loaded, "released TUI loaded shared database")
            assert "v" + self.versions["tui"] in pane, pane
        finally:
            subprocess.run(self.tmux + ["kill-session", "-t", "package-tui"], check=False, stderr=subprocess.DEVNULL)
        assert self.identity() == self.pane_identity

    def checkpoint(self, name):
        self.tui_smoke()
        self.open_gui()
        snapshot = self.inspector.invoke("get_snapshot")
        project = next(p for p in snapshot["projects"] if p["id"] == "package-project")
        feature = next(f for f in project["features"] if f["id"] == "package-feature")
        assert [s["id"] for s in feature["sessions"]] == ["package-session"]
        wait(lambda: self.inspector.evaluate("Array.from(document.querySelectorAll('button')).some(b=>b.textContent.includes('Preserve this live session'))"), "existing packaged feature navigation")
        self.inspector.evaluate("Array.from(document.querySelectorAll('button')).find(b=>b.textContent.includes('Preserve this live session')).click()")
        try:
            wait(lambda: self.inspector.evaluate("!!document.querySelector('.xterm')"), "reattached live terminal")
        except AssertionError:
            (self.out / (name + "-failure.txt")).write_text(self.inspector.evaluate("document.body.innerText"))
            raise
        marker = f"{name}: GUI {self.versions['gui']} ({self.gui_package}) / TUI {self.versions['tui']} ({self.tui_target}) — same live pane"
        # The app's real terminal_input command writes to its attached tmux pane.
        for _ in range(50):
            try:
                self.inspector.invoke("terminal_input", {"key": "package-feature:package-session", "text": marker + "\r"})
                break
            except AssertionError:
                time.sleep(.1)
        wait(lambda: self.received.exists() and marker in json.loads(self.received.read_text()), "input reached unchanged fixture process")
        assert self.identity() == self.pane_identity
        with sqlite3.connect(self.config / "amf.db") as db:
            assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
            schema = db.execute("SELECT MAX(version) FROM schema_version").fetchone()[0]
            assert db.execute("SELECT id FROM projects").fetchall() == [("package-project",)]
            assert db.execute("SELECT id FROM features").fetchall() == [("package-feature",)]
            assert db.execute("SELECT id FROM feature_sessions").fetchall() == [("package-session",)]
        record = {"phase": name, "versions": dict(self.versions), "pane_identity": self.pane_identity, "schema": schema, "initial_schema": self.initial_schema, "gui_package": self.gui_package, "gui_sha256": sha256(self.gui_binary), "tui_sha256": sha256(self.root / "opt/amf/amf"), "tmux_version": subprocess.check_output(self.tmux[:1] + ["-V"], text=True).strip(), "terminal_input": marker}
        self.report.append(record)
        if self.capture:
            self.screenshot(name, marker)
        self.close_gui()
        print("PASS:", marker, flush=True)

    def owned_window(self):
        from Xlib import display
        x = display.Display()
        windows = []
        atom = x.intern_atom("_NET_WM_PID")
        def visit(window):
            try:
                pid = window.get_full_property(atom, 0)
                if pid and self.owns_gui_pid(int(pid.value[0])) and window.get_geometry().width > 500:
                    windows.append(window)
                for child in window.query_tree().children:
                    visit(child)
            except Exception:
                pass
        visit(x.screen().root)
        assert windows, "No window belonging to owned packaged GUI"
        return x, max(windows, key=lambda w: w.get_geometry().width * w.get_geometry().height)

    def screenshot(self, name, marker):
        import gi
        gi.require_version("Gdk", "3.0")
        gi.require_version("GdkX11", "3.0")
        from gi.repository import Gdk, GdkX11
        x, window = self.owned_window()
        wait(lambda: self.inspector.evaluate("document.fonts.status==='loaded'"), "packaged fonts")
        time.sleep(1)
        geometry = window.get_geometry()
        native = GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(), window.id)
        picture = Gdk.pixbuf_get_from_window(native, 0, 0, geometry.width, geometry.height)
        assert picture
        filename = name + ".png"
        picture.savev(str(self.out / filename), "png", [], [])
        method = "AppImage extract-and-run without FUSE" if self.gui_package == "appimage" else "rootless Debian install"
        self.notes.append({"file": filename, "note": marker + f". Official released payloads; {method} and shared database, not a system installer test."})
        x.close()

    def owns_gui_pid(self, pid):
        # AppImage's runtime owns a child GUI. The live unreaped launcher pins
        # its private process group; never accept a window by title or a stale
        # descendant PID after that launcher has exited.
        if self.gui is None or self.gui.poll() is not None:
            return False
        try:
            return os.getpgid(pid) == self.gui.pid
        except ProcessLookupError:
            return False

    def cleanup(self):
        try:
            self.close_gui()
        finally:
            if self.server_started:
                subprocess.run(self.tmux + ["kill-server"], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", type=pathlib.Path)
    parser.add_argument("--assets-dir", type=pathlib.Path)
    parser.add_argument("--tui-target", choices=["gnu", "musl"], default="musl", help="Released TUI bundle; GNU requires the release's glibc version")
    parser.add_argument("--gui-package", choices=["deb", "appimage"], default="deb", help="Debian private install or AppImage extract-and-run without FUSE")
    parser.add_argument("--capture", action="store_true", help="Also save native PNG proof, when explicitly requested")
    args = parser.parse_args()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    reports, notes = [], []
    with tempfile.TemporaryDirectory(prefix="amf-package-assets-") as cached:
        assets = args.assets_dir.resolve() if args.assets_dir else pathlib.Path(cached)
        for ordering in ["gui-first", "tui-first"]:
            with tempfile.TemporaryDirectory(prefix="amf-linux-packages-") as temporary:
                scratch = pathlib.Path(temporary)
                proof = PairProof(scratch, out, assets, args.capture, args.tui_target, args.gui_package)
                try:
                    proof.install_tui("0.48.0")
                    proof.install_gui("0.48.0")
                    proof.seed()
                    if ordering == "gui-first":
                        proof.checkpoint("001-installed-pair")
                        proof.install_gui("0.49.0")
                        proof.checkpoint("002-gui-only-upgrade")
                        proof.install_tui("0.49.0")
                        proof.checkpoint("003-both-upgraded")
                    else:
                        proof.install_tui("0.49.0")
                        proof.checkpoint("004-tui-only-upgrade")
                        proof.install_gui("0.49.0")
                        proof.checkpoint("005-reverse-order-complete")
                    assert proof.report[-1]["schema"] == 45 and proof.initial_schema == 43
                    reports.append({"ordering": ordering, "checkpoints": proof.report})
                    notes.extend(proof.notes)
                finally:
                    proof.cleanup()
    (out / "package-validation.json").write_text(json.dumps({"release_digest_pins": ASSETS, "tui_target": args.tui_target, "gui_package": args.gui_package, "host": os.uname().sysname + " " + os.uname().machine, "host_glibc": os.confstr("CS_GNU_LIBC_VERSION"), "rootless_dependencies": "host GTK/WebKit; TUI bundled tmux; no apt resolution or system installation", "sequences": reports}, indent=2) + "\n")
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(note) + "\n" for note in notes))


if __name__ == "__main__":
    main()
