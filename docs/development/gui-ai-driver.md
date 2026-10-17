# Driving the native GUI with an AI agent

Use `scripts/dev/gui-driver/amf-gui-driver.py` for repeatable interaction with
the real Linux/WSLg Tauri GUI and Rust backend. It keeps one GUI and inspector
connection alive across CLI calls. Responses are compact JSON; mutations return
only success, waits poll locally, and batches return only their final result.
The driver never captures images.

The [TUI driver](tui-ai-driver.md) offers the same session/JSON conventions with
tmux keys, paste and changed-row observations. Both share `scripts/dev/ai_driver.py`
for isolation, budgets, batching and process cleanup.

For agents working in another project, AMF injects the portable
[`amf-interact` skill](../../skills/amf-interact/SKILL.md), these scripts and
operating references into that harness's local skill directory. Use the bundled
script with `start --source /path/to/agent-mainframe` to select the matching AMF
checkout. An installed release GUI alone cannot provide its debug inspector.
The skill explains when to choose configuration, live automation or isolated
interaction. New/restarted sessions receive it; an upgrade also refreshes files
in existing feature workdirs. Active agents may need their harness's skill reload
or a restart to discover a newly added skill.

## Setup and quick start

Install the normal [GUI prerequisites](../../gui/README.md#run-from-source),
Python 3 and `websocket-client` (`python3-websocket` on Debian/Ubuntu). Use the
Python interpreter that has that module, usually `/usr/bin/python3`. No browser
download, Playwright, Xlib or GTK Python bindings are needed. An X11 display is
required; on a headless Linux host run the driver under `xvfb-run -a` and keep
that wrapper alive for the session's lifetime (wrap the whole interaction script,
not just `start`). For example, `xvfb-run -a /usr/bin/python3
scripts/dev/gui-driver/smoke.py` runs the native smoke check and cleanup together.

Build once per source change, then reuse the binary and session:

```sh
(cd gui && npm ci)
cargo build -p amf-gui --locked

/usr/bin/python3 scripts/dev/gui-driver/amf-gui-driver.py start
# {"session":"/tmp/amf-gui-...","ready":true}
```

`start --binary /path/to/debug/amf-gui` supports a separate Cargo target directory.
The default honors `CARGO_TARGET_DIR`. Use a debug build from this checkout:
the driver uses WebKit's debug inspector and starts this checkout's Vite server
on Tauri's fixed port 1420. It refuses a port occupied on any loopback address
(`127.0.0.1`, `::1` or whatever `localhost` resolves to). Only one development
frontend can run at a time. It does not rebuild or install dependencies for you.

Use the returned directory as `--session`, before the command. For example:

```sh
driver() { /usr/bin/python3 scripts/dev/gui-driver/amf-gui-driver.py --session "$gui_session" "$@"; }
gui_session=/tmp/amf-gui-... # replace with start's returned directory
driver snapshot
driver click --name 'New project'
driver wait --text 'Repository path'
driver snapshot --scope '[role=dialog]'
driver fill --name 'Repository path' /tmp/my-fixture-repo
driver fill --name 'Project name' 'Fixture project'
driver click --name 'Create project'
driver wait --text 'Fixture project'
driver stop
```

Each session has a new, private `HOME`, XDG config/state/data/cache, SQLite
database and tmux socket. The real `claude`, `codex`, `opencode`, `pi`, `gh` and
`code` commands are masked with failing stubs, so the default session cannot
spend agent tokens through those commands. For agent workflows, put deterministic
fixtures in the session's `bin/` directory. This is process/config isolation,
not a network or filesystem sandbox: use temporary fixture repositories when
testing actions that edit files. The driver adds no production automation port
or app permissions. Its Unix socket is inside the private session directory;
the WebKit inspector listens on loopback for the lifetime of the debug GUI.

`stop` waits for cleanup of this session's GUI/Vite process groups and private
tmux server. Logs, the database and fixture files remain in the session directory
for diagnosis; remove that directory after stopping when no longer needed.
Use `stop` in a `finally` block or shell trap. A failed startup also cleans up
its processes and reports the log directory.

## Keeping interaction output small

- Start with `snapshot`; scope subsequent snapshots to a dialog or panel.
  Snapshots contain visible text and controls with stable `e1`-style references.
  Hidden/inert elements and xterm terminal contents are excluded. Read a bounded
  terminal excerpt explicitly with `eval` or a private tmux pane capture if needed.
- `click`, `fill` and `select` accept `--ref e1`, `--name 'Exact label'` or
  `--selector 'CSS'`, plus optional `--scope 'CSS'`. Exact names include ARIA
  labels and associated form labels. Ambiguous targets, disabled controls and
  detached references fail. Refresh the snapshot after replacing a dialog.
  This is a compact DOM summary, not a complete accessibility-tree implementation.
- A full page reload (Vite does one for edits it cannot hot-swap) keeps the
  session usable: the driver re-injects its helper on the next command. Every
  earlier reference is stale, and new ones read `e1-1`, `e1-2`, … so an old
  reference cannot name a new element. Take another snapshot.
- `fill` updates React controlled inputs/textareas through their native value
  setter and bubbling events. `select` accepts an option's **value**. Click a
  checkbox or switch to change it.
- `wait --text 'Ready'` tests visible text, ignoring differences in whitespace,
  so a phrase may span inline elements or the line breaks a snapshot shows. With
  `--scope`, a scope that has not rendered yet counts as "not yet"; one that
  matches several elements is an error. `wait --expression 'boolean JS'`
  tests more precise conditions. `eval 'synchronous JS'` is an escape hatch for
  targeted assertions and reads; it does not await returned promises.
- `--timeout 30` sets a shared deadline for an entire request or batch (default
  15 seconds, maximum 120). Polling happens inside the driver and produces one
  response. Choose an explicit wait for the result of asynchronous UI mutations.
- `snapshot --text-limit 1200 --control-limit 20` reduces observation size.
  `--max-output 4000` bounds the serialized result in characters (default 12,000),
  returning a marked preview if necessary. The response envelope is additional.
  These are character budgets, not measured model-token budgets.

Batch a known workflow into one tool call. Supply a JSON array on stdin or
with `batch --file path.json`. The final snapshot is the only observation returned:

```sh
driver batch <<'JSON'
[
  {"op":"click","name":"New project"},
  {"op":"wait","text":"Repository path"},
  {"op":"fill","name":"Repository path","value":"/tmp/my-fixture-repo"},
  {"op":"fill","name":"Project name","value":"Fixture project"},
  {"op":"snapshot","scope":"[role=dialog]","text_limit":800}
]
JSON
```

A batch stops at the first error and reports its one-based action index. Earlier
mutations remain applied; nothing is rolled back or retried. Inspect current
state before continuing after an error. CLI errors return JSON and a nonzero exit.

## Contrast audit

`driver audit-contrast [--scope CSS] [--themes id,id] [--include-disabled] [--json]`
measures the current page against WCAG 1.4.3 (text: 4.5:1, or 3:1 when large) and
1.4.11 (inputs, selects, `.btn` and `.switch` boundaries or fills: 3:1). Colours are
composited through ancestor backgrounds, `opacity` and alpha. Text over a
gradient is judged against each stop of the gradient's top layer (the one drawn
behind content). A borderless, unfilled field is credited with the boundary of
the box that draws it. Disabled elements, which WCAG exempts, are omitted unless
`--include-disabled`, which tags them.

Themes are applied in place, as `applyTheme` does, so one page state is measured
under the built-in palettes and every catalog theme; nothing is navigated, and the
original theme is restored afterwards. Transitions and animations are frozen while
measuring, because `getComputedStyle` otherwise reports colours part-way through a
theme change. The theme catalog loads from the backend; if it cannot, the command
fails with the reason. The xterm canvas and SVG are excluded, as are native
checkboxes and radios: WebKitGTK paints those itself, so their CSS colours do not
describe what is drawn.

Each finding is one line, `<themes failing>/<themes audited> <kind> <element>
"<sample>" worst=<ratio> :: theme=ratio ...`, worst theme first and capped at six.
It obeys `--max-output`; scope the audit to shrink it. Open the view to audit first.

```sh
driver audit-contrast --scope '[role=dialog]'
# {"ok":true,"result":{"themes":14,"findings":1,"lines":["3/14 text div.card > p.muted \"Hint\" worst=2.1 :: nord=2.1 dark=2.5 light=3.0"]}}
```

## Existing tools and when to use them

| Tool | Useful for | Limit |
| --- | --- | --- |
| GUI driver | Native GUI interaction and compact behavioral assertions | Linux/WSLg debug build; no visual validation |
| `gui/tests/` / `npm test` | Fast component/state checks | Mocked Tauri IPC; no native renderer/backend round trip |
| `cargo test --workspace --locked` | Shared core and Rust GUI command behavior | Does not drive the native webview |
| [`amf automation`](../automation/README.md) | Structured project/feature setup in a running TUI | TUI IPC entrypoint; not a GUI controller |
| [`scripts/dev/screenshot/`](../../scripts/dev/screenshot/README.md) | Existing deterministic native GUI fixtures and visual proof | Scenario-specific setup and capture; use only when visual proof is explicitly requested |

Existing GUI proof scripts already use the same native WebKit inspector protocol,
but embed separate connection helpers, fixtures, assertions and image capture.
The driver makes interactive exploration reusable without requiring a new proof
scenario. Those specialized scenarios continue to own their fixtures and visual
assertions. A future extraction can let them share the transport once their
different lifecycle and capture requirements are covered.

Driver checks:

```sh
/usr/bin/python3 scripts/dev/test_ai_driver.py
/usr/bin/python3 scripts/dev/gui-driver/test_driver.py
(cd gui && npm test -- GuiDriver)
/usr/bin/python3 scripts/dev/gui-driver/smoke.py
```

The Python tests cover protocol routing/errors, bounded output, local waits,
batch failure and process cleanup. DOM tests cover exact/scoped targeting,
visibility, reference lifetimes and actual React controlled form updates.
The native smoke command starts its own session, creates a fixture project via
the real form, checks SQLite persistence and failure handling, and stops the GUI
and a private tmux fixture. It reports timings and observation character counts
in one JSON response; `--binary` supports a separate build output.
