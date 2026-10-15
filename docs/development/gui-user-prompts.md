# Desktop User prompt authoring

Open **Prompt library** from workspace navigation or an agent composer. Use
**New User template** to save a name, multiline body and comma/space-separated
tags. User entries offer **Edit User template** and a separate deletion
confirmation. Descriptions and configured placeholder definitions are kept,
including definitions whose keys are absent from the current body.

**Duplicate to User** copies a visible User, global, project or worktree template
with a fresh identity and dates and the TUI's ` (copy)` name suffix. The source
stays unchanged. A declarative template with unsupported metadata is browseable
but cannot be copied when a User row could lose those fields. Malformed User
metadata must be repaired externally before authoring; browsing remains tolerant.
Editing declarative sources and exporting templates remain TUI actions.

An agent composer's **Save as template** opens a new User editor with its current
unsent text. Saving, cancellation and write failures leave the mounted composer
unchanged. This action never sends a prompt. Required name/body validation and tag
normalization use the same domain rules as the TUI.

Delayed User edits and deletions read and compare the exact persisted row under
SQLite's writer lock. Changes to its raw metadata, timestamps or content refuse
the write. An unrelated row's edit or library reordering does not conflict.
Reloading an edited template or closing an unsaved editor requires an explicit
discard choice. Pending writes cannot be dismissed or submitted twice.

Run the ordinary workspace and frontend suites described in [checks.md](checks.md).
Focused coverage lives in `gui_prompts/authoring_tests.rs`, `db/prompt_templates.rs`
and the User editor/library/App component tests. The asserted native Linux
scenario is
[`gui-prompt-authoring.txt`](../../scripts/dev/screenshot/scenarios/gui-prompt-authoring.txt).
It uses private Git/worktree, SQLite, tmux and runtime-home fixtures with offline
Claude/OpenCode receivers, preserves `HOME`, checks user harness configurations by
digest only and captures only its own GUI PID. It neither starts a paid harness
nor writes declarative sources. macOS authoring interactions remain unvalidated.
