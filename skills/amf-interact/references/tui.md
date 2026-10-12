# AMF TUI interaction

The driver reads tmux's rendered grid and sends terminal-mode-aware input. It
works with xterm-based terminals without launching a browser or replaying ANSI.
It covers application cursor keys, Unicode and bracketed paste; it does not
validate xterm's browser event routing or rendering.

Set `skill_dir` to the absolute directory containing the SKILL.md you loaded.
All paths below then work from any project's working directory:

```sh
driver_root="$skill_dir/scripts"
if [ ! -f "$driver_root/ai_driver.py" ]; then
  driver_root="$skill_dir/../../scripts/dev" # AMF source checkout
fi
tui_driver="$driver_root/tui-driver/amf-tui-driver.py"
python3 "$tui_driver" start
# {"session":"/tmp/amf-tui-...","ready":true}
```

The binary defaults to a local AMF development build when available, then a
usable `$AMF_BIN`, then `amf` on PATH. Override with `start --binary /path/to/amf`.
Python 3 and tmux are required. `start --geometry 100x30` controls initial size.

Use the returned directory as `tui_session`. Global options precede the command:

```sh
tui_session=/tmp/amf-tui-... # replace with the returned directory
tui() { python3 "$tui_driver" --session "$tui_session" "$@"; }
trap 'tui stop' EXIT
tui snapshot
tui keys N
tui wait --text 'New Project'
tui paste 'Fixture project'
tui snapshot --first-row 10 --last-row 27 --text-limit 1200
tui keys Escape
tui wait --text 'New Project' --absent
tui snapshot --changes
```

Snapshots return row/text pairs, dimensions, cursor, terminal modes and revision.
Rows are zero-based. Blank/border-only rows are omitted; leading padding is
preserved. `--changes` compares the last complete delivered snapshot of the same
row range and geometry; an empty text value clears a previous row. Waits, hidden
batch snapshots, truncation and failed delivery preserve that baseline. A scope
change or resize yields a full snapshot. Text deltas do not report color changes.

`--max-output 4000` bounds serialized result characters; overflow is a marked
preview. `--text-limit` bounds snapshot line text. These are character budgets,
not measured model-token counts. `--timeout 30` is one deadline for the whole
request/batch (default 15, maximum 120 seconds).

Batch actions from stdin or `batch --file actions.json`; only the last result is
returned. An error names the failing action and leaves earlier mutations applied.

```sh
tui batch <<'JSON'
[
  {"op":"keys","keys":["N"]},
  {"op":"wait","text":"New Project"},
  {"op":"paste","text":"Fixture project"},
  {"op":"snapshot","first_row":10,"last_row":27,"text_limit":1200}
]
JSON
```

| Operation | Arguments |
| --- | --- |
| `keys` | `keys`: array of tmux key names, e.g. `Up`, `C-a`, `Enter` |
| `text` | `text`: literal printable Unicode, without control characters |
| `paste` | `text`: Unicode with tabs/newlines; bracketed when enabled by the app |
| `wait` | `text`: literal screen text; optional `absent: true` |
| `snapshot` | Optional `changes`, `first_row`, `last_row`, `text_limit` |
| `resize` | `geometry`: e.g. `100x30` |
| `automation` | `action`: `create-project`, `create-feature`, or `create-batch-features`; `request`: JSON object |

Driver `automation` targets the private TUI, unlike a bare `amf automation`
command. Use the latter only for requested changes to the running dashboard.
For its current request fields, run `amf automation <action> --help`.

`stop` shuts down only the driver's private tmux server. Logs and its database
remain under the returned session directory for diagnosis. If AMF exits on its
own, later commands say why and its final screen is in `last-pane.txt` there. Default harness
executables are failing stubs; put deterministic fixtures in that session's
`bin/` directory to exercise agent-related workflows without paid calls.
