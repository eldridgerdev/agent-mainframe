# Driving the TUI with an AI agent

`scripts/dev/tui-driver/amf-tui-driver.py` keeps a real AMF dashboard running
on a private tmux server and accepts compact JSON commands. It shares the
[GUI driver's](gui-ai-driver.md) protocol, output budgets, process cleanup and
fixture isolation. It needs only Python 3, tmux and an AMF binary, with no
browser, display server or extra Python packages.

AMF also embeds these helpers in the portable
[`amf-interact` skill](../../skills/amf-interact/SKILL.md), injected into managed
projects under `.claude/skills`, `.agents/skills`, `.opencode/skills` or
`.pi/skills`. Its bundled scripts/references work from any working directory;
an AMF checkout is unnecessary. New/restarted sessions receive the skill, and
upgrades refresh existing feature workdirs. Active agents may need a skill reload
or restart. The skill routes workspace configuration, live TUI automation and
isolated GUI/TUI interaction without loading both operating guides up front.

## Quick start

Build once per source change:

```sh
cargo build --locked --bin amf
/usr/bin/python3 scripts/dev/tui-driver/amf-tui-driver.py start
# {"session":"/tmp/amf-tui-...","ready":true}
```

`start --binary /path/to/amf` accepts another build output; the default honors
`CARGO_TARGET_DIR`, then falls back to a usable `AMF_BIN` or `amf` on PATH when
the development binary is absent. `--geometry 120x40` sets the initial columns and rows;
`--config /path/to/config.json` supplies a private AMF configuration. Geometry
accepts 60–400 columns and 20–200 rows. A session directory supplied with
`--session` must be new or empty, and short enough for Unix sockets. Without
`--session`, the directory is created under the platform's temporary directory,
or under `/tmp` where that path is itself too long (macOS).

Use the returned directory for subsequent calls. Global options go before the
command:

```sh
tui_session=/tmp/amf-tui-... # replace with start's returned directory
driver() { /usr/bin/python3 scripts/dev/tui-driver/amf-tui-driver.py --session "$tui_session" "$@"; }
driver snapshot
driver keys N
driver wait --text 'New Project'
driver paste 'Fixture project'
driver snapshot --first-row 10 --last-row 27
driver keys Escape
driver wait --text 'New Project' --absent
driver snapshot --changes
driver stop
```

`start` waits for a dashboard and verifies that the input loop responds with a
read-only help round trip. It seeds an empty store with Claude selected to
bypass first-run onboarding. All paid harness executables still resolve to
failing session-local stubs. Replace files in the session's `bin/` directory
with deterministic fixtures when a test needs a harness. The real `gh` and
`code` commands are also masked.

The session owns its HOME, XDG directories, database and tmux socket. Empty
Codex transcript directories prevent AMF's discovery fallback from reading
`/home/$USER/.codex/sessions`. The private tmux server ignores user tmux config;
ambient tmux identifiers and custom terminfo paths are removed. Launching one
driver cannot redirect another driver or the real AMF IPC endpoint. Use
temporary repositories: this is fixture isolation, not a filesystem or network
sandbox. Repository actions can still modify the repository you supply.

`stop` waits for cleanup of the private server, including its AMF-created feature
sessions and observers, and the driver. Logs and fixture data remain in the
session directory; remove it after stopping when no longer needed. Use `stop`
in a `finally` block or shell trap. The AMF debug log is under
`<session>/state/amf/debug.log`; startup errors report the session directory.

## Small observations and batched workflows

`snapshot` reads tmux's current screen, without ANSI escapes or scrollback. It
returns dimensions, cursor coordinates/visibility, terminal modes, a revision
hash and lines with **zero-based terminal row numbers**. Empty rows and border-only
rows are omitted, and empty right borders are trimmed. Leading padding is
preserved. Unicode character indices are not terminal columns: wide characters
occupy multiple cells.

`snapshot --changes` returns only rows that differ from the last successful
snapshot of the same row range and dimensions. An empty text value clears an
old row. Changing the scope or resizing returns a full observation. Waits do not
advance this baseline. Snapshots hidden inside a batch, failed requests and
disconnected clients do not advance it either. This compares text; color-only
changes are not reported.

`--text-limit 2400` bounds line text. Truncated snapshots are marked and do not
advance the baseline. Global `--max-output 4000` bounds the serialized result;
oversized results become a marked preview and also preserve the previous
baseline. The next complete snapshot still compares against the last delivered
observation. Character budgets are not measured model-token budgets.

- `keys Up C-a Enter` uses tmux key names and its current terminal modes.
- `text 'literal text'` sends printable Unicode characters without interpreting
  them as key names. Control characters are rejected; use `keys` or `paste`.
- `paste 'text'` uses a uniquely named private tmux buffer and `paste-buffer -p`.
  It supports multiline input and tabs, then deletes the buffer. Escape sequences
  and other control characters are rejected.
- `wait --text 'Ready'` polls locally and emits one result. `--absent` waits for
  text to disappear. Matches are literal, case-sensitive and within the visible
  screen; use unique markers. Wait timeouts do not dump the screen.
- `resize 100x30` changes the tmux window and invalidates the delta baseline.
- `automation create-project --file request.json` calls `amf automation` using
  this session's environment and IPC socket. `create-feature` and
  `create-batch-features` are also supported; default harness stubs prevent paid
  starts. No environment reconstruction or hand-written socket messages needed.
  To create a feature without launching sessions, put `"no_start": true` in
  its request JSON (including a batch automation action's `request` object).
  It remains stopped and returns `started: false`; omit initial prompt fields
  when using this option. See the [automation guide](../automation/README.md).

Batch a known workflow into one tool call, with its final observation as the
only result. Each action uses the CLI operation name:

```sh
driver batch <<'JSON'
[
  {"op":"keys","keys":["N"]},
  {"op":"wait","text":"New Project"},
  {"op":"paste","text":"Fixture café 世界"},
  {"op":"wait","text":"Fixture café 世界"},
  {"op":"snapshot","first_row":10,"last_row":27,"text_limit":1000}
]
JSON
```

`batch --file actions.json` accepts the same array. Global `--timeout 30` sets
one deadline for the entire request or batch (default 15 seconds, maximum 120).
A failure reports its one-based action index and stops immediately. Earlier
mutations remain; nothing is rolled back or retried. Input is refused if the
owned pane is dead or in tmux copy-mode.

If AMF exits, the session ends. Its final screen and scrollback (a panic
message, for instance) are saved to `last-pane.txt` in the session directory
before the private tmux server is shut down, and every later command reports
why the session ended instead of a socket error.

## xterm compatibility

AMF's live TUI uses ratatui/crossterm; xterm.js is used by the desktop/remote
terminal viewers and the newer screenshot renderer. The driver reads the
already-decoded tmux grid, the same source the existing capture/view transports
use. It does not replay an ANSI stream through another terminal emulator, so
alternate screens, cursor movements and scroll regions stay tmux's responsibility.

The private AMF pane uses system `xterm-256color` terminfo. tmux translates named
arrow keys according to application-cursor mode. Paste normalization matches
xterm.js: CRLF and LF become CR, and tmux adds bracketed-paste delimiters when
the program enables that mode. CRLF is collapsed before tmux's LF conversion to
avoid sending two CR characters. tmux 3.2 does not expose a bracketed-paste
metadata flag; the driver uses AMF's existing `paste-buffer -p` path instead of
depending on a newer metadata field.

This validates terminal behavior and text, not xterm fonts, colors, WebGL or
browser event routing. For requested visual proof, continue to use the existing
`amf-screenshot` skill and xterm screenshot renderer. Routine driver calls never
capture images or install browser dependencies.

## Validation

```sh
/usr/bin/python3 scripts/dev/test_ai_driver.py
/usr/bin/python3 scripts/dev/tui-driver/test_driver.py
/usr/bin/python3 scripts/dev/gui-driver/test_driver.py
/usr/bin/python3 scripts/dev/tui-driver/smoke.py
```

The native smoke check accepts `--binary`, creates a project through the real
form, verifies SQLite persistence and isolated IPC, tests resize/delta/failure
handling, and stops its private server. A disposable raw terminal program checks
the exact Unicode, application-cursor and bracketed-paste bytes, including LF
and CRLF normalization, with no paid agent or browser. One JSON response reports
the results, timings and observation sizes. Linux CI runs this smoke check; the
Python regression checks also run in CI.
