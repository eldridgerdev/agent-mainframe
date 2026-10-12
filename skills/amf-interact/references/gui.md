# AMF native GUI interaction

This is a Linux/WSLg native Tauri/WebKit driver, using compact DOM observations.
It requires an AMF source checkout with frontend dependencies, a matching debug
`amf-gui` binary, an X11 display, Python 3 and `websocket-client`. A release GUI
alone does not expose the required debug inspector. Do not infer GUI results
from the TUI driver when those prerequisites are absent.

Set `skill_dir` to the absolute directory containing the SKILL.md you loaded and
`amf_source` to the AMF checkout, which can be outside the managed project:

```sh
driver_root="$skill_dir/scripts"
if [ ! -f "$driver_root/ai_driver.py" ]; then
  driver_root="$skill_dir/../../scripts/dev" # AMF source checkout
fi
gui_driver="$driver_root/gui-driver/amf-gui-driver.py"
amf_source=/path/to/agent-mainframe
# Build from that checkout once per AMF source change:
(cd "$amf_source/gui" && npm ci)
(cd "$amf_source" && cargo build -p amf-gui --locked)
/usr/bin/python3 "$gui_driver" start --source "$amf_source"
# {"session":"/tmp/amf-gui-...","ready":true}
```

Use the Python interpreter that has `websocket-client` installed (often
`/usr/bin/python3`). `--binary /path/to/debug/amf-gui` supports a separate Cargo
target directory; the default also honors `CARGO_TARGET_DIR`. Keep source and
binary matched. The driver needs port 1420 and refuses an occupied Vite port.
On a headless host, wrap the **entire interaction script** with `xvfb-run -a` so
the display remains alive until cleanup.

Use the returned directory as `gui_session`. Global options precede commands:

```sh
gui_session=/tmp/amf-gui-... # replace with the returned directory
gui() { /usr/bin/python3 "$gui_driver" --session "$gui_session" "$@"; }
trap 'gui stop' EXIT
gui snapshot
gui click --name 'New project'
gui wait --text 'Repository path'
gui snapshot --scope '[role=dialog]'
gui fill --name 'Repository path' /tmp/my-fixture-repo
gui fill --name 'Project name' 'Fixture project'
gui snapshot --scope '[role=dialog]' --text-limit 1200 --control-limit 20
```

Targets use `--ref e1` from a snapshot, `--name 'Exact label'`, or `--selector
'CSS'`, with optional `--scope 'CSS'`. Ambiguous, disabled and detached targets
fail. Refresh references after a dialog is replaced. `fill` supports React
controlled inputs; `select` takes an option's value; use `click` for checkboxes.
Snapshots omit hidden/inert elements and terminal contents. Use a targeted
`eval 'synchronous JS'` only when the normal controls/observations are insufficient.

Batch actions from stdin or `batch --file actions.json`; only the last result is
returned. This example runs after the initial dialog has been closed:

```sh
gui batch <<'JSON'
[
  {"op":"click","name":"New project"},
  {"op":"wait","text":"Repository path"},
  {"op":"fill","name":"Repository path","value":"/tmp/my-fixture-repo"},
  {"op":"fill","name":"Project name","value":"Fixture project"},
  {"op":"snapshot","scope":"[role=dialog]","text_limit":1200}
]
JSON
```

`--timeout 30` gives the whole request/batch one deadline (default 15, maximum
120 seconds). `--max-output 4000` bounds serialized result characters; overflow
is a marked preview. `wait --text` polls visible text locally; `wait --expression`
accepts a precise boolean JavaScript condition. `eval` is synchronous and does
not await promises. On a batch error, inspect before continuing: earlier actions
remain applied and are never retried automatically.

The session has a private HOME/database/tmux server and disabled paid harnesses.
`stop` waits for the owned GUI/Vite processes and private tmux server to exit;
logs and fixture files remain under the session directory. Repositories selected
inside the GUI can still be edited, so use disposable fixtures for tests.
Text assertions do not establish visual correctness. Use the AMF checkout's
screenshot skill only when the user explicitly requests visual proof.
