"""Shared harness for native GUI frame scripts.

Drives one isolated GUI through the WebKit inspector and captures its X11
window. Only the window belonging to the supplied GUI PID is inspected.
"""

import hashlib
import json
import pathlib
import re
import subprocess
import sys
import time
import urllib.request

import gi
import websocket
from Xlib import display

# The capture itself runs in a subprocess; importing here fails before the
# first frame when the bindings it needs are missing.
gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gdk, GdkX11  # noqa: E402,F401

CAPTURE_WINDOW = """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])"""


class GuiFrames:
    """One inspector connection and X11 window for the GUI process `pid`."""

    def __init__(self, out, pid, inspector_address):
        self.out = pathlib.Path(out).resolve()
        self.notes = []
        self._captured = set()
        self._seq = 0
        for _ in range(100):
            try:
                with urllib.request.urlopen(
                    f"http://{inspector_address}/", timeout=2
                ) as response:
                    inspector_page = response.read().decode()
                inspector_path = re.search(
                    r"/socket/[^\']+/WebPage", inspector_page
                ).group(0)
                break
            except (OSError, AttributeError):
                time.sleep(0.1)
        else:
            raise RuntimeError("The native WebKit inspector did not become ready")
        self._ws = websocket.create_connection(
            f"ws://{inspector_address}{inspector_path}",
            timeout=15,
            suppress_origin=True,
        )
        self._x = None
        try:
            self._target = json.loads(self._ws.recv())["params"]["targetInfo"][
                "targetId"
            ]
            self._x = display.Display()
            self.window = self._find_window(pid)
        except BaseException:
            self.close()
            raise

    @classmethod
    def from_argv(cls):
        """Build from the runner's arguments: output dir, GUI PID, inspector."""
        return cls(sys.argv[1], int(sys.argv[2]), sys.argv[3])

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self._ws.close()
        if self._x is not None:
            self._x.close()

    def _find_window(self, pid):
        atom = self._x.intern_atom("_NET_WM_PID")
        windows = []

        def visit(w):
            try:
                p = w.get_full_property(atom, 0)
                g = w.get_geometry()
                if p and int(p.value[0]) == pid and g.width > 500:
                    windows.append(w)
                for child in w.query_tree().children:
                    visit(child)
            except Exception:
                pass

        for _ in range(100):
            visit(self._x.screen().root)
            if windows:
                break
            time.sleep(0.1)
        assert windows, "The isolated GUI has no X11 window"
        return max(
            windows, key=lambda w: w.get_geometry().width * w.get_geometry().height
        )

    def resize(self, width, height):
        self.window.configure(x=0, y=0, width=width, height=height)
        self._x.sync()

    def evaluate(self, expression):
        self._seq += 1
        seq = self._seq
        request = {
            "id": seq,
            "method": "Runtime.evaluate",
            "params": {"expression": expression, "returnByValue": True},
        }
        self._ws.send(
            json.dumps(
                {
                    "id": seq + 10000,
                    "method": "Target.sendMessageToTarget",
                    "params": {
                        "targetId": self._target,
                        "message": json.dumps(request),
                    },
                }
            )
        )
        while True:
            data = json.loads(self._ws.recv())
            if data.get("method") == "Target.dispatchMessageFromTarget":
                message = json.loads(data["params"]["message"])
                if message.get("id") == seq:
                    assert not message.get("error"), message
                    assert not message["result"].get("wasThrown"), message
                    return message["result"]["result"].get("value")

    def wait(self, expression):
        for _ in range(100):
            if self.evaluate(expression):
                return
            time.sleep(0.1)
        raise AssertionError(
            f"Timed out: {expression}; body={self.evaluate('document.body.innerText')}"
        )

    def click(self, text):
        # A button can be present but disabled while its panel's Rust state
        # loads, which takes longer on slower runners.
        self.wait(
            f'Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==={json.dumps(text)} && !b.disabled)'
        )
        self.evaluate(
            f'Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)}).click()'
        )

    def capture(self, name, note, expects, expression=None, allow_alert=False):
        """Assert the page state, then save the window as `name` with its text."""
        self.wait('document.fonts.status==="loaded"')
        self.wait('!document.body.innerText.includes("Loading changes…")')
        body = self.evaluate("document.body.innerText")
        for text in expects:
            assert text in body, (text, body)
        if not allow_alert:
            assert not self.evaluate('!!document.querySelector("[role=alert]")'), body
        if expression:
            assert self.evaluate(expression), expression
        time.sleep(0.25)
        g = self.window.get_geometry()
        subprocess.run(
            [
                "/usr/bin/python3",
                "-c",
                CAPTURE_WINDOW,
                str(self.window.id),
                str(g.width),
                str(g.height),
                str(self.out / name),
            ],
            check=True,
        )
        digest = hashlib.sha256((self.out / name).read_bytes()).digest()
        assert digest not in self._captured, "The native window saved a stale frame"
        self._captured.add(digest)
        (self.out / name.replace(".png", ".txt")).write_text(body)
        self.notes.append({"file": name, "note": note, "expects": expects})
        print("PASS:", name, note, flush=True)

    def write_notes(self):
        (self.out / "capture-notes.jsonl").write_text(
            "".join(
                json.dumps({"file": n["file"], "note": n["note"]}) + "\n"
                for n in self.notes
            )
        )
