# Desktop feature harness changes

A worktree feature's **More feature actions → Change harness…** dialog uses the
same replacement operation as the TUI session configuration picker. Choosing a
harness or canceling does not apply it. Confirmation switches every agent row,
clears saved conversations and token usage bindings, and preserves session IDs,
TODO references, individual stopped choices and nonagent definitions.

A running feature stops before checked hook setup and binding persistence, then
restarts its previously runnable sessions. If every row is individually stopped,
it remains stopped. A stopped feature changes bindings and hooks without starting
processes. Replacement availability and malformed settings are checked before
shutdown. Hook, save and launch errors report their phase and the desktop
refreshes process state; generated files or processes may have partially changed.

Delayed confirmation binds the stable project/feature IDs, full persisted session
rows, checkout and Git directory identity, HEAD/branch, pane PIDs and current
configuration/availability. A changed target requires reload and confirmation.
The desktop blocks changes while this window has an affected unsent agent draft
or send in progress. Project preferred-harness editing is a separate operation.

## Reproduce native proof

After installing the development prerequisites and building matching binaries:

```sh
npm --prefix gui ci
npm --prefix gui run build
cargo build --workspace --locked
AMF_GUI_CAPTURE_BINARY="$PWD/target/debug/amf-gui" \
  /usr/bin/python3 scripts/dev/screenshot/capture-gui-harness-change.py /tmp/amf-harness-proof
```

The runner uses the maintained `amf-interact` GUI driver's controls against a real
native webview/IPC backend. It preserves `HOME`, supplies private XDG/Git/SQLite
and tmux fixtures, and mounts a private `~/.amf` runtime through bubblewrap. It
masks installed Claude versions and launches only offline Claude/OpenCode
receivers. It verifies the real harness configuration and AMF runtime inventory
are unchanged. No Codex conversion or paid harness runs in this native proof.

Seven asserted native frames cover a running mixed-session feature, unsent draft
refusal and preservation, cancel without mutation, a real external SQLite stale
confirmation, confirmed restart with new PIDs and preserved stopped/nonagent
rows, and a stopped conversion with no launch. Core tests additionally cover
unavailable replacements, malformed JSON/TOML without clobbering, hook/write and
shutdown failures, unsaved-binding refusal, partial restart, all-stopped rows and
CAS conflicts before binding persistence and after launch.

For the isolated hosted capture, use
`scripts/dev/screenshot/scenarios/gui-harness-change.txt`. Publish only after its
branch is pushed with `scripts/dev/screenshot/publish-pages.sh --strict`.
