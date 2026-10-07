#!/usr/bin/env python3
"""Assert native GUI syntax highlighting through WebKit IPC and capture its X11 window.

Covers unified/split diffs, Final Review with a line comment and the Learning
reader in light and dark. Parsers are installed through the GUI's own
`syntax_install` command (the TUI picker's installer: `git clone` from GitHub,
`cc`) into the private XDG config. No agent or AI harness runs.
Only the window belonging to the supplied isolated GUI PID is inspected.
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

gi.require_version("Gdk", "3.0")
gi.require_version("GdkX11", "3.0")
from gi.repository import Gdk, GdkX11  # noqa: E402,F401

out = pathlib.Path(sys.argv[1]).resolve()
pid = int(sys.argv[2])
repo = pathlib.Path(sys.argv[4])

for attempt in range(100):
    try:
        with urllib.request.urlopen(f"http://{sys.argv[3]}/", timeout=2) as response:
            inspector_page = response.read().decode()
        inspector_path = re.search(r"/socket/[^\']+/WebPage", inspector_page).group(0)
        break
    except (OSError, AttributeError):
        time.sleep(0.1)
else:
    raise RuntimeError("The native WebKit inspector did not become ready")
ws = websocket.create_connection(
    f"ws://{sys.argv[3]}{inspector_path}", timeout=15, suppress_origin=True
)
target = json.loads(ws.recv())["params"]["targetInfo"]["targetId"]
seq = 0


def send(method, params):
    global seq
    seq += 1
    request = {"id": seq, "method": method, "params": params}
    ws.send(
        json.dumps(
            {
                "id": seq + 10000,
                "method": "Target.sendMessageToTarget",
                "params": {"targetId": target, "message": json.dumps(request)},
            }
        )
    )
    while True:
        data = json.loads(ws.recv())
        if data.get("method") == "Target.dispatchMessageFromTarget":
            message = json.loads(data["params"]["message"])
            if message.get("id") == seq:
                assert not message.get("error"), message
                return message["result"]


def evaluate(expression):
    result = send("Runtime.evaluate", {"expression": expression, "returnByValue": True})
    assert not result.get("wasThrown"), result
    return result["result"].get("value")


def wait(expression, timeout=10):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if evaluate(expression):
            return
        time.sleep(0.1)
    # Say what the page showed instead, so a CI failure is diagnosable.
    state = evaluate(
        'JSON.stringify({header:document.querySelector(".diff-file-header,.learning-code-header")?.innerText,'
        'code:(document.querySelector(".diff-code,.learning-code")?.innerHTML||"").slice(0,1500),'
        'alerts:Array.from(document.querySelectorAll("[role=alert],[role=status]")).map(e=>e.innerText)})'
    )
    raise AssertionError(f"{expression}\nPage: {state}")


def click(text):
    found = evaluate(
        f'(()=>{{const b=Array.from(document.querySelectorAll("button")).find(b=>b.textContent.trim()==={json.dumps(text)});if(b)b.click();return !!b;}})()'
    )
    assert found, f"No button {text!r}"


def select(label, value):
    evaluate(
        f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('select');el.value={json.dumps(value)};el.dispatchEvent(new Event('change',{{bubbles:true}}));}})()"""
    )


def fill(label, value):
    evaluate(f"""(()=>{{const label=Array.from(document.querySelectorAll('label')).find(l=>l.querySelector('span')?.textContent==={json.dumps(label)});const el=label.querySelector('textarea');Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,{json.dumps(value)});el.dispatchEvent(new Event('input',{{bubbles:true}}));}})()""")


def tauri(command, args=None):
    """Runs a Tauri command from the page and returns its JSON result."""
    evaluate(
        f"window.__amfShot=undefined;window.__TAURI_INTERNALS__.invoke({json.dumps(command)},{json.dumps(args or {})}).then(v=>window.__amfShot={{ok:v}},e=>window.__amfShot={{err:e}})"
    )
    wait("window.__amfShot!==undefined")
    result = evaluate("window.__amfShot")
    assert "err" not in result, result
    return result["ok"]


def install_parser(key):
    """Installs a parser through the GUI's command, as the badge's button does."""
    status = tauri("syntax_install_status")
    if status["running"]:
        raise AssertionError(f"An install is already running: {status}")
    tauri("syntax_install", {"language": key})
    deadline = time.time() + 600
    while time.time() < deadline:
        status = tauri("syntax_install_status")
        if not status["running"]:
            assert status["error"] is None and status["message"], status
            print("INSTALLED:", status["message"], flush=True)
            return
        time.sleep(1)
    raise AssertionError(f"{key} parser install timed out")


x = display.Display()
atom = x.intern_atom("_NET_WM_PID")
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


for attempt in range(100):
    visit(x.screen().root)
    if windows:
        break
    time.sleep(0.1)
assert windows, "The isolated GUI has no X11 window"
window = max(windows, key=lambda w: w.get_geometry().width * w.get_geometry().height)
notes = []
captured_frames = set()


def capture(name, note, expects, expression=None):
    wait('document.fonts.status==="loaded"')
    wait('!document.body.innerText.includes("Loading changes…") && !document.body.innerText.includes("Updating review…")')
    body = evaluate("document.body.innerText")
    for text in expects:
        assert text in body, (text, body)
    assert not evaluate('!!document.querySelector("[role=alert]")'), body
    if expression:
        assert evaluate(expression), expression
    time.sleep(0.4)
    g = window.get_geometry()
    subprocess.run(
        [
            "/usr/bin/python3",
            "-c",
            """import gi,sys;gi.require_version('Gdk','3.0');gi.require_version('GdkX11','3.0');from gi.repository import Gdk,GdkX11;w=GdkX11.X11Window.foreign_new_for_display(Gdk.Display.get_default(),int(sys.argv[1]));p=Gdk.pixbuf_get_from_window(w,0,0,int(sys.argv[2]),int(sys.argv[3]));assert p;p.savev(sys.argv[4],'png',[],[])""",
            str(window.id),
            str(g.width),
            str(g.height),
            str(out / name),
        ],
        check=True,
    )
    digest = hashlib.sha256((out / name).read_bytes()).digest()
    assert digest not in captured_frames, "The native window saved a stale frame"
    captured_frames.add(digest)
    (out / name.replace(".png", ".txt")).write_text(body)
    notes.append({"file": name, "note": note, "expects": expects})
    print("PASS:", name, note, flush=True)


def choose_file(path):
    evaluate(f'Array.from(document.querySelectorAll(".diff-file")).find(b=>b.querySelector("span").textContent==={json.dumps(path)}).click()')
    wait(f'document.querySelector(".diff-file[aria-pressed=true] span")?.textContent==={json.dumps(path)}')
    wait('!document.body.innerText.includes("Updating review…")')


def row_has(text, token, scope=".diff-code"):
    """True when the code element showing `text` colours part of it as `token`."""
    return (
        f'Array.from(document.querySelectorAll({json.dumps(scope + " code")}))'
        f'.some(c=>c.textContent.includes({json.dumps(text)})&&!!c.querySelector({json.dumps(".syn-" + token)}))'
    )


def no_tokens(scope=".diff-code"):
    return f'!document.querySelector({json.dumps(scope + " [class^=syn-]")})'


def line_of(path, text):
    for number, line in enumerate((repo / path).read_text().splitlines(), start=1):
        if text in line:
            return number
    raise AssertionError((path, text))


def set_dark(dark):
    send(
        "Page.overrideUserPreference",
        {"name": "PrefersColorScheme", "value": "Dark" if dark else "Light"},
    )
    wait(f'matchMedia("(prefers-color-scheme: dark)").matches==={str(dark).lower()}')


def open_changes():
    click("Changes")
    wait('!!document.querySelector(".diff-file")')
    wait('!document.body.innerText.includes("Loading changes…")')


claude = repo / ".claude"
claude.mkdir()
with (repo / ".git/info/exclude").open("a") as exclusions:
    exclusions.write("\n.claude/\n")
progress_path = claude / "final-review-progress.json"
source_before = {p: p.read_bytes() for p in repo.rglob("*") if p.is_file() and ".git" not in p.parts and ".claude" not in p.parts}

try:
    set_dark(False)
    window.configure(x=0, y=0, width=1500, height=980)
    x.sync()
    wait('Array.from(document.querySelectorAll("button")).some(b=>b.textContent.trim()==="Credit notes")')
    click("Credit notes")
    wait('document.body.innerText.includes("Stopped") && document.body.innerText.includes("Changes")')
    open_changes()

    # The private config starts with no parsers: a supported language says so
    # and offers the TUI picker's install behind explicit confirmation.
    choose_file("tools/report.py")
    wait('document.body.innerText.includes("Plain text · Python parser not installed")')
    assert evaluate(no_tokens())
    click("Install Python parser…")
    wait('!!document.querySelector("[role=group][aria-label=\'Install Python parser\']")')
    capture(
        "001-install-confirmation.png",
        "With no Python parser installed the diff stays plain text, and installing one needs explicit confirmation of the GitHub clone and local compile.",
        ["Plain text · Python parser not installed", "clones its tree-sitter grammar from GitHub", "Install parser", "Credit notes subtract from the customer total."],
        no_tokens(),
    )
    click("Install parser")
    wait('document.body.innerText.includes("Installing the Python parser")', timeout=30)
    wait(row_has("Credit notes subtract", "string"), timeout=300)
    capture(
        "002-python-docstring-unified.png",
        "After the install the diff reloads with colours: the changed docstring line is a string although its opening quotes are far above the hunk.",
        ["Python", "Installed Python tree-sitter parser", "Credit notes subtract from the customer total.", "sorted(totals.items()"],
        row_has("Credit notes subtract", "string") + "&&" + row_has("return dict(sorted", "keyword") + '&&!document.body.innerText.includes("Plain text · Python")',
    )

    # Install the other fixture languages through the same GUI command.
    for key in ["rust", "typescript", "markdown"]:
        install_parser(key)
    click("Refresh")
    wait('!document.body.innerText.includes("Loading changes…")')

    choose_file("src/lib.rs")
    wait(row_has("credit notes carry a negative total", "comment"), timeout=20)
    capture(
        "003-rust-unified.png",
        "Rust rows inside a block comment and a raw string are coloured from whole-file context; added/removed backgrounds and both line-number columns stay visible.",
        ["Rust", "credit notes carry a negative total", "- apac", "cents.max(0)"],
        row_has("credit notes carry a negative total", "comment")
        + "&&" + row_has("- apac", "string")
        + "&&" + row_has("let cents", "keyword")
        + '&&!!document.querySelector("tr.diff-added .syn-comment")&&!!document.querySelector("tr.diff-removed .syn-comment")',
    )

    select("Layout", "split")
    choose_file("web/receipt.ts")
    wait('!!document.querySelector(".diff-split")')
    wait(row_has("Credit notes appear as negative totals.", "string"), timeout=20)
    capture(
        "004-typescript-split.png",
        "Side by side, the TypeScript template-literal line is a string on the current side, and both sides of the changed return are highlighted.",
        ["TypeScript", "Credit notes appear as negative totals.", "Credit note"],
        row_has("Credit notes appear as negative totals.", "string")
        + "&&" + row_has('receipt.cents < 0', "keyword"),
    )

    select("Layout", "unified")
    choose_file("README.md")
    wait('!!document.querySelector(".diff-code [class^=syn-]")', timeout=20)
    capture(
        "005-markdown.png",
        "Markdown uses the same shared parser set: list markers and inline code are coloured.",
        ["Markdown", "Credit notes are receipts with a negative total."],
        '!!document.querySelector(".diff-code [class^=syn-]")',
    )

    choose_file("deploy/settings.weird")
    capture(
        "006-unknown-extension-plain.png",
        "A file with no supported language falls back to plain text and says why.",
        ["Plain text · no supported language", "replicas = 3"],
        no_tokens(),
    )
    choose_file("logo.bin")
    wait('document.body.innerText.includes("Binary file changed")')
    assert evaluate('!document.querySelector(".diff-file-header .syntax-badge")')
    click("Close")
    wait('!document.querySelector(".diff-reader")')

    # Final Review: a line comment on a highlighted line.
    click("Final Review")
    wait('!!document.querySelector(".diff-file")', timeout=20)
    choose_file("src/lib.rs")
    line = line_of("src/lib.rs", "credit notes carry a negative total")
    label = f"Select line {line}"
    evaluate(f"document.querySelector('button[aria-label={json.dumps(label)}]').click()")
    wait(f"document.querySelector('button[aria-label={json.dumps(label)}]')?.getAttribute('aria-pressed')==='true'")
    click("Comment on selection")
    fill("Line comment", "total_cents clamps at zero, so this rule is not enforced yet.")
    click("Save comment")
    wait('!document.querySelector(".review-editor")')
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')
    capture(
        "007-review-line-comment.png",
        "Final Review keeps highlighting with a saved line comment anchored to the selected line, which stays a comment although the block comment opened above the hunk.",
        ["Rust", f"line {line} [", "total_cents clamps at zero"],
        row_has("credit notes carry a negative total", "comment", ".diff-lines .review-line-selected"),
    )
    saved = json.loads(progress_path.read_text())["line_comments"]["src/lib.rs"]
    assert saved[0]["location"] == {"old_line": None, "new_line": line}, saved
    click("Pause review")
    wait('!document.querySelector(".diff-reader")')

    # The Learning reader shows the whole file with the same colours.
    click("Learning")
    wait('!!document.querySelector(".learning-file")', timeout=20)
    if not evaluate('Array.from(document.querySelectorAll(".learning-file")).some(b=>b.textContent.trim().endsWith("lib.rs"))'):
        evaluate('Array.from(document.querySelectorAll(".learning-file")).find(b=>b.textContent.trim()==="src").click()')
        wait('Array.from(document.querySelectorAll(".learning-file")).some(b=>b.textContent.trim().endsWith("lib.rs"))')
    evaluate('Array.from(document.querySelectorAll(".learning-file")).find(b=>b.textContent.trim().endsWith("lib.rs")).click()')
    wait('document.querySelector(".learning-code-header strong")?.textContent==="src/lib.rs"')
    wait('!!document.querySelector(".learning-code .syn-comment")', timeout=20)
    evaluate('document.querySelector(\'button[aria-label="Select line 9"]\').click()')
    wait('document.querySelector(\'button[aria-label="Select line 9"]\')?.classList.contains("learning-line-selected")')
    capture(
        "008-learning-reader.png",
        "The Learning reader colours the whole Rust file, and a selected line inside the block comment keeps its comment colour.",
        ["src/lib.rs", "Rust", "credit notes carry a negative total", "pub fn total_cents"],
        '!!document.querySelector(".learning-line-selected .syn-comment")&&!!document.querySelector(".learning-code .syn-keyword")',
    )

    # Dark mode: the same views through the dark syntax tokens.
    set_dark(True)
    capture(
        "009-learning-reader-dark.png",
        "In dark mode the Learning reader switches to the dark syntax tokens.",
        ["src/lib.rs", "Rust", "pub fn total_cents"],
        'getComputedStyle(document.querySelector(".learning-code .syn-keyword")).color!=="rgb(124, 58, 237)"',
    )
    evaluate('document.querySelector("button[aria-label=Close]").click()')
    wait('!document.querySelector(".learning-reader")')
    open_changes()
    choose_file("src/lib.rs")
    wait(row_has("credit notes carry a negative total", "comment"))
    capture(
        "010-rust-unified-dark.png",
        "Dark mode keeps the coloured comment and raw-string rows readable on added and removed backgrounds.",
        ["Rust", "credit notes carry a negative total", "- apac"],
        row_has("- apac", "string"),
    )
    select("Layout", "split")
    choose_file("web/receipt.ts")
    wait('!!document.querySelector(".diff-split")')
    wait(row_has("Credit notes appear as negative totals.", "string"))
    capture(
        "011-typescript-split-dark.png",
        "The side-by-side TypeScript diff in dark mode.",
        ["TypeScript", "Credit notes appear as negative totals."],
        row_has("Credit notes appear as negative totals.", "string"),
    )
    click("Close")
    wait('!document.querySelector(".diff-reader")')
    click("Final Review")
    wait('!!document.querySelector(".diff-file")', timeout=20)
    choose_file("src/lib.rs")
    evaluate('Array.from(document.querySelectorAll("details")).forEach(d=>d.open=true)')
    capture(
        "012-review-line-comment-dark.png",
        "The resumed review restores the saved line comment above the dark-mode highlighted diff.",
        ["Rust", f"line {line} [", "total_cents clamps at zero"],
        row_has("credit notes carry a negative total", "comment"),
    )
    click("Pause review")
    wait('!document.querySelector(".diff-reader")')

    source_after = {p: p.read_bytes() for p in repo.rglob("*") if p.is_file() and ".git" not in p.parts and ".claude" not in p.parts}
    assert source_after == source_before, "Highlighting or review changed source files"
    (out / "capture-notes.jsonl").write_text(
        "".join(
            json.dumps({"file": n["file"].replace(".png", ".ansi"), "note": n["note"]})
            + "\n"
            for n in notes
        )
    )
finally:
    ws.close()
    x.close()
