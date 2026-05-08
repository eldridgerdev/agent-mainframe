# Phase 4: Session Status — DB-Backed Storage

## Context

Phases 1–3 (on the `database` branch) migrated the project store,
token usage cache, and debug log to SQLite. This branch implements
Phase 4: replacing the per-file `.amf/session-status/<id>.txt` pattern
with a proper `session_status` table in the DB.

## Current Behaviour

Custom sessions (kind = `Custom`) write a plain-text status line to:

```text
<workdir>/.amf/session-status/<session_id>.txt
```

The file is written by the `on_stop` hook script that each custom
session defines. AMF reads the first non-empty line of that file during
every sync cycle (`sync.rs` foreground path and background job).

Problems:

- No atomic guarantee — a crash mid-write produces a partial file.
- Listing `.amf/session-status/` requires `readdir` on every sync.
- Status is silently lost when the `.amf/` directory is cleaned up
  or the worktree is removed and re-created.
- No cross-session query (e.g. "all idle custom sessions").

## Design

### Migration 004

Add a `session_status` table to the existing `amf.db`:

```sql
CREATE TABLE session_status (
    session_id  TEXT PRIMARY KEY,
    feature_id  TEXT NOT NULL,
    status_text TEXT NOT NULL DEFAULT '',
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_session_status_feature
    ON session_status(feature_id);
```

### New CLI subcommand: `amf set-status`

```text
amf set-status <session_id> <status_text>
```

- Opens `amf.db` (the same DB the running TUI uses, WAL mode — safe for
  a concurrent second writer).
- Inserts/replaces the row.
- Exits immediately; no IPC needed.

Hook scripts currently do:

```bash
echo "$STATUS" > "$AMF_WORKDIR/.amf/session-status/$AMF_SESSION_ID.txt"
```

After this change they do:

```bash
amf set-status "$AMF_SESSION_ID" "$STATUS"
```

### Read path (sync.rs)

Replace the file read in `run_jobs` and `sync_session_status_with_tracker`
with a DB lookup:

```rust
// Before
let status_path = job.workdir
    .join(".amf")
    .join("session-status")
    .join(format!("{}.txt", job.session_id));
let status_text = std::fs::read_to_string(&status_path).ok()...;

// After
let status_text = app.db
    .as_ref()
    .and_then(|db| db.load_session_status(&job.session_id).ok())
    .flatten();
```

For the background sync path the DB connection is not available (the
tracker is cloned into a background thread). The background job carries
status text read at job-dispatch time, so the pattern changes to: load
all custom session statuses from DB once before dispatching, pass them
into the job as a `HashMap<session_id, String>`.

### Write path (internal)

When AMF internally sets a custom session's status (e.g. after `on_stop`
completes), write to DB:

```rust
db.upsert_session_status(session_id, feature_id, status_text)?;
```

Also delete the row when a feature or session is deleted
(`feature_ops.rs` `delete_feature`).

### Generated hook script update

`ensure_notify_scripts()` regenerates hook scripts on every startup
from `include_str!("scripts/...")`. Update the custom `on_stop`
template to call `amf set-status` instead of writing the file.

Keep the old file-write as a fallback for installs running a binary
that pre-dates this change (i.e. the script writes both).

### Backward Compatibility

During the transition period (old binary with new DB, or new binary
with old hooks):

- If a `session_status` row exists in DB → use it.
- Else fall back to reading the `.txt` file and immediately write it
  into the DB (one-time migration on first read).

This means the file remains the source of truth until the hook script
is regenerated, then the DB takes over.

## Files to Change

| File | Change |
|---|---|
| `src/db/migrations.rs` | Add `MIGRATION_004` with `session_status` table |
| `src/db/session_status.rs` | New: `load()`, `upsert()`, `delete_session()`, `delete_feature()` |
| `src/db/mod.rs` | Expose `load_session_status`, `upsert_session_status`, `delete_session_status`, `delete_feature_statuses` |
| `src/main.rs` | Add `SetStatus { session_id, status_text }` subcommand |
| `src/app/sync.rs` | Replace file read with DB lookup (both foreground and background paths) |
| `src/app/feature_ops.rs` | Delete status rows on feature/session delete |
| `src/app/session_ops.rs` | Delete status rows on session delete |
| `scripts/on-stop-custom.sh` | Write `amf set-status` + file fallback |
| `src/app/setup.rs` | Regenerate script via `include_str!` |

## Test Plan

- [ ] `amf set-status <id> "some text"` writes to DB, readable by next sync.
- [ ] Custom session status appears in dashboard after `on_stop` fires.
- [ ] Deleting a feature removes its status rows from DB.
- [ ] Old `.txt` file is migrated into DB on first read (backward compat).
- [ ] Background sync job reads status correctly (from pre-dispatch
  snapshot, not from background thread DB access).
- [ ] Existing `sync_session_status_reads_first_line` test updated to
  use DB instead of file.
- [ ] No regression in the 12+ existing session-status tests.

## Out of Scope

- Migrating existing `.txt` files on startup (covered by the lazy
  read-and-migrate on first sync).
- Removing the `.amf/session-status/` directory cleanup code in
  `feature_ops.rs` (leave it for now, it becomes a no-op once hooks
  are regenerated).
