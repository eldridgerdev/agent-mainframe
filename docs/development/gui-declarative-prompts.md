# Declarative prompt editing and export

Select a Global, Project or Worktree entry in the desktop **Prompt library** and
choose **Edit declarative template**. The editor names its source and write path.
Edit name, body and tags; descriptions, configured placeholder definitions,
identities, timestamps and opaque extra metadata remain unchanged. Renaming to
another row's name is refused. Closing or reloading unsaved edits asks before
discarding; a failed save or reload keeps all local fields.

**Export template…** copies any visible User or declarative template to an
explicit Global, Project or Worktree destination. Refresh the preview to see the
actual path and source text. A same-name replacement requires a separate checkbox;
a self-export reports that no file will be written. Exports copy all source
metadata and retain destination-only opaque template fields, unrelated templates
and other configuration. A feature sharing its project's checkout must use the
Project destination. Exporting does not send or alter a composer draft.

The TUI uses the same file engine for edits and exports. Its existing `x`, then
`g`/`p`/`w` destination keys remain; `y` confirms a same-name replacement and `Esc`
cancels. In a declarative editor, `F5` reloads with discard protection. `Ctrl+R`
continues to redo body edits for every prompt editor.

Writes patch raw JSON instead of round-tripping typed configuration. Malformed
JSON, unsupported known fields and ambiguous same-name destination rows are
refused. Opaque unknown fields are preserved. When a local scope still reads
`.amf/config.json`, the first save writes `amf.json` with the effective legacy
content and permissions; the legacy bytes remain unchanged. The dialog shows both
paths. Concurrent arrival of the canonical file invalidates the open snapshot.

Freshness binds raw source/destination bytes, row ordinal and content, both local
configuration paths, file/checkout identities and stable project/feature IDs.
User exports also bind the exact persisted SQLite row revision. External edits,
renames, moved source rows, deleted scopes and retargeted checkouts require a new
preview or explicit reload/reselection. Unrelated User rows do not conflict.
Ordered advisory writer locks cooperate with desktop/TUI settings writers and
normalize aliases before locking. Atomic replacement prevents partial files;
editors that do not take AMF's advisory locks still have an unavoidable race
between the final freshness read and rename. This is not a filesystem-wide CAS.

Run the normal checks in [checks.md](checks.md). Focused tests cover the raw engine,
both adapters and owned dialogs. The requested asserted native Linux/WSLg proof
is [`gui-declarative-prompts.txt`](../../scripts/dev/screenshot/scenarios/gui-declarative-prompts.txt):
private SQLite, Git/worktree, config, tmux and runtime-home fixtures; offline
Claude/OpenCode receivers; source/destination preservation assertions; unchanged
composer and pane identities; user harness configuration digest guards. It keeps
`HOME` and isolates `HOME/.amf` with bubblewrap. No paid harness or production
harness setup runs. macOS authoring interactions remain unvalidated.
