# Project preferred harness in the desktop

Open a project and choose **Preferred harness**. This edits the project's saved
default separately from **Agent availability**, which edits its allowed-harness
configuration. Choosing a value changes nothing until **Save settings**. Cancel
and reload protect unsaved edits. Existing features, session bindings and running
panes retain their harness; no hook setup or process launch runs while saving.

The TUI and desktop share preference validation and persistence. A desktop
confirmation binds the stable project, repository directory identity, current
preference, both project configuration files, global configuration and current
allowed/installed choices. Changed inputs require reload. SQLite's normal store
CAS refuses a concurrent save and adopts the sibling's state without overwriting
it. JSON configuration and the SQLite preference are separate operations.

Ordinary and TODO new-feature forms receive their initial default from the
shared preferred-or-first normalization. A saved preference outside the current
allowed, configured and installed catalog falls back for a new form without
rewriting that preference. Empty choices block creation. Loading and config
errors also block creation with an explicit retry. Existing dialogs retain their
selected harness after refresh; manual and preset choices remain authoritative
across delayed responses and destination changes. An unavailable selection stays
visible and blocks submission until the user explicitly chooses another harness.

## Native proof

Build matching source binaries and frontend dependencies, then run:

```sh
AMF_GUI_CAPTURE_BINARY="$PWD/target/debug/amf-gui" \
  /usr/bin/python3 scripts/dev/screenshot/capture-gui-project-preference.py /tmp/amf-project-preference-proof
```

The runner preserves HOME and uses private XDG, SQLite, Git and tmux fixtures,
with a Bubblewrap mount for the private `~/.amf` runtime. It masks installed
Claude versions, allows only offline Claude/OpenCode receiver launches and
asserts that real harness configuration/runtime inventories remain unchanged.
Its native controls use the maintained `amf-interact` GUI driver and real IPC.

The nine-frame scenario covers explicit save/cancel, a real external SQLite
stale confirmation, unchanged original feature/session bindings and pane PIDs,
the ordinary default, a preset surviving refresh, actual ordinary and TODO
OpenCode launches, the TODO planning default and its unsent editable draft.
Core regressions additionally exercise config/install/catalog changes,
replacement/deleted repositories, sibling feature edits, a CAS conflict after
validation, failed persistence and preferred/default normalization. Frontend
coverage includes delayed responses, cached destination transitions, unavailable
intent, lookup failure/retry and unsaved preference protection.

Publish the pushed scenario
`scripts/dev/screenshot/scenarios/gui-project-preference.txt` with
`scripts/dev/screenshot/publish-pages.sh --strict`. Its mandatory namespace probe
may temporarily relax Ubuntu's user-namespace policy only on an ephemeral
GitHub-hosted Linux runner, restoring it on exit. Local/self-hosted policy is
never changed and there is no unisolated fallback.

This proof uses offline receivers. Paid harness behavior and macOS native runtime
remain unvalidated; hosted macOS unit tests do not establish native GUI behavior.
