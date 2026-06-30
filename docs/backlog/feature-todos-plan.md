# Feature TODOs

- **Status:** Ready
- **Owner:** unassigned
- **Relates to:** `SessionKind` (`src/project.rs`), session picker
  (`src/app/session_ops.rs`, `src/handlers/picker.rs`), composer seed
  (`src/app/compose.rs` — `open_compose_seeded`), SQLite store
  (`src/db/`), view modes (`src/app/state.rs` — `AppMode`).

## Why / problem

When you stop work on a feature for the day, the context of "what's
left and where I left off" lives only in your head. AMF should let you
jot down TODOs against the work you're doing, see them next to the rest
of the feature tree, and pick a TODO back up tomorrow by launching an
agent that already knows what to do.

The list doubles as a launcher: highlight a TODO, press a key, and AMF
spins up a fresh agent-harness session in the right worktree with a
composer prompt pre-filled to address that item — editable before it's
sent.

## Decisions (from interview)

- **It's a session kind, not auto-created.** A new
  `SessionKind::Todos` is added through the `S` session picker, "just
  like any other" session. If no TODOS session has been started for a
  project, nothing shows — there is no always-on row.
- **At most one TODOS session per project.** Enforced across all of the
  project's features. The picker hides/greys the option (or jumps to
  the existing one) when the project already has a TODOS session. The
  session lives under whichever feature it was created in; that feature
  is the **host feature**, and all of the project's TODOs belong to it.
- **Native UI, not tmux-backed.** Opening the TODOS session enters a
  native AMF overlay/mode rather than attaching to a tmux pane.
  `SessionKind::Todos.is_agent_harness()` is `false` and no tmux window
  is created for it.
- **SQLite persistence.** Stored in the existing `amf.db` via a new
  migration and access module — not in the `ProjectStore` JSON blob.
- **Spawn into the host feature.** Launching an agent for a TODO always
  creates the new session inside the feature the TODO belongs to (same
  worktree/branch), seeds the composer with a generated prompt, and
  leaves it **editable before sending** (seeded, not auto-submitted).
- **Item fields:** done checkbox, priority, notes/detail body, and a
  link to the spawned session.
- **Extras in scope:** reorder items, editable composer prompt before
  launch, a list-level "left off here" carry-over note, and
  quick-capture of a TODO from inside any session view.

## Proposed design

### Data model

New SQLite tables (migration `MIGRATION_010`), accessed via a new
`src/db/todos.rs` module exposing methods on `AmfDb`. Kept out of the
`ProjectStore` JSON so the list isn't rewritten on every store save.

```text
todo_lists
  id            TEXT PRIMARY KEY      -- uuid
  project_id    TEXT NOT NULL UNIQUE  -- enforces one list per project
  feature_id    TEXT NOT NULL         -- host feature
  carry_over    TEXT                  -- "left off here" banner note
  created_at    TEXT NOT NULL
  updated_at    TEXT NOT NULL

todos
  id                  TEXT PRIMARY KEY  -- uuid
  list_id             TEXT NOT NULL     -- FK -> todo_lists.id
  title               TEXT NOT NULL
  body                TEXT              -- notes / detail
  priority            TEXT NOT NULL     -- 'high' | 'med' | 'low'
  done                INTEGER NOT NULL  -- 0/1
  sort_order          INTEGER NOT NULL  -- manual reorder
  spawned_session_id  TEXT              -- FeatureSession.id of launched agent
  created_at          TEXT NOT NULL
  updated_at          TEXT NOT NULL
```

`AmfDb` gains: `load_todo_list(project_id)`,
`upsert_todo_list(...)`, `list_todos(list_id)`, `upsert_todo(...)`,
`delete_todo(id)`, `reorder_todos(...)`, and cascade cleanup when a
project/feature is deleted (extend the existing delete paths).

### Session kind & tree

- Add `Todos` to `SessionKind` (`src/project.rs`).
  `is_agent_harness()` returns `false`; it is **not** an
  `AgentKind`.
- The session picker (`S`) offers "TODOs" only when the project has no
  existing TODOS session. Selecting it calls a new
  `feature.add_session_named(SessionKind::Todos, "TODOs")` and creates
  the `todo_lists` row, but skips tmux window creation
  (`session_ops.rs` branch on kind).
- Renders in the feature's session list with its own icon/label like
  other sessions. Opening it (Enter / view) routes to the native TODO
  mode instead of `AppMode::Viewing`.

### Native TODO UI

- New `AppMode::Todos(TodoViewState)` in `src/app/state.rs`, with state
  holding the loaded list, todos, selection index, and an inline editor
  (reuse `src/editor.rs`) for title/body/carry-over edits.
- New `src/app/todos.rs` (app methods: open/close, load/save, add,
  edit, delete, toggle done, cycle priority, reorder, edit carry-over,
  spawn agent) and `src/handlers/todos.rs` (key dispatch). Rendering in
  `src/ui/dialogs/todos.rs` (or a full-screen `ui/` view).
- Layout: a carry-over "left off here" banner at top, then the list of
  TODOs grouped/sorted by `done` then `sort_order`, each showing
  priority marker, checkbox, title, and a notes indicator.
- Keys (draft, reconcile with `keybindings.json` / config wizard):
  `j/k` move, `a`/`n` add, `e` edit title, `o` edit notes,
  `space`/`x` toggle done, `p` cycle priority, `J/K` reorder,
  `d` delete (confirm), `g`/`Enter` spawn agent, `b` edit carry-over
  banner, `Esc`/`Ctrl+Q` exit.

### Spawn agent from a TODO

1. Resolve the host feature from the TODO's `list.feature_id`.
2. Create a new agent-harness session in that feature (reuse the
   existing add-session + `launch_claude` path in `session_ops.rs`),
   using the project/feature's configured agent and vibe/plan settings.
3. Switch into the new session view and call `open_compose_seeded(...)`
   with a generated prompt — **seeded, not submitted** — so it can be
   edited first. Draft template:

   ```text
   Please address this TODO item for this feature:

   <title>

   <body, if any>
   ```

4. Store the new session's id in `todos.spawned_session_id` so the list
   can show "launched" and jump back to it.

### Quick-capture from a session view

- A leader/view keystroke (e.g. leader → `t`) opens a one-line input
  that appends a TODO to the current project's list.
- If the project has no TODOS session yet, either create one on the
  fly under the current feature or show a toast prompting the user to
  add it via `S` (see open questions).

## Progress

### Epic 1 — Persistence layer

- [ ] Add `MIGRATION_010` creating `todo_lists` + `todos` tables.
- [ ] Add `src/db/todos.rs` with row structs and CRUD/query fns.
- [ ] Expose `AmfDb` methods (load/upsert list, list/upsert/delete/
      reorder todos).
- [ ] Cascade-delete todos & list when a project is deleted.
- [ ] On host-feature deletion (project survives), prompt **Re-home**
      (reassign `feature_id` to a chosen surviving feature) or
      **Delete** the list; Delete-only when no features remain.
- [ ] Unit tests for migration + round-trip CRUD + one-list-per-project
      constraint.

### Epic 2 — Session kind & tree integration

- [ ] Add `SessionKind::Todos`; `is_agent_harness()` → false; handle in
      all `match` sites (ui labels/icons, sync, status).
- [ ] Offer "TODOs" in the `S` picker only when the project has none;
      create the `todo_lists` row on selection.
- [ ] Skip tmux window creation for `Todos` sessions in
      `session_ops.rs`.
- [ ] Render the TODOS session row in the feature session list.
- [ ] Block creating a second TODOS session per project (test).

### Epic 3 — Native TODO view (read + navigate)

- [ ] `AppMode::Todos(TodoViewState)` + open/close from the session.
- [ ] `src/app/todos.rs` load + state; `src/handlers/todos.rs` dispatch.
- [ ] `src/ui/dialogs/todos.rs` rendering: carry-over banner + list with
      checkbox, priority marker, notes indicator, selection highlight.
- [ ] `j/k` navigation and exit (`Esc`/`Ctrl+Q`).

### Epic 4 — TODO editing

- [ ] Add (`a`/`n`) with inline editor; persist.
- [ ] Edit title (`e`) and notes body (`o`).
- [ ] Toggle done (`space`/`x`); completed items stay visible
      (strikethrough/grouped), no bulk clear.
- [ ] Cycle priority (`p`); sort respects priority + manual order.
- [ ] Reorder (`J/K`) updating `sort_order`; persist.
- [ ] Delete (`d`) with confirm.
- [ ] Edit carry-over "left off here" banner (`b`).

### Epic 5 — Spawn agent from a TODO

- [ ] `g`/`Enter` creates a new agent-harness session in the host
      feature (inheriting its agent/vibe/plan settings) and switches to
      it. If the TODO already links a live session, jump to it and add
      onto it instead of creating a second.
- [ ] Seed composer (editable, not submitted) with the generated prompt
      from title + body.
- [ ] Record `spawned_session_id`; show "launched" marker and allow
      jump-back to that session.

### Epic 6 — Quick-capture & polish

- [ ] View-mode keystroke to append a TODO to the project list.
- [ ] When no TODOS session exists yet, auto-create the list + session
      under the current feature, then append.
- [ ] Wire keys into `keybindings.json` / config wizard and help
      overlay (`ui/dialogs/help.rs`).
- [ ] Update `CLAUDE.md` architecture notes and `CHANGELOG.md`.

## Resolved decisions

- **Quick-capture with no list yet → auto-create.** If the project has
  no TODOS session, quick-capture silently creates the `todo_lists` row
  (and the `Todos` session under the current feature) before appending
  the item, so capture never fails.
- **Host-feature deletion → prompt to re-home _or_ delete.** "Re-home"
  means: the list itself is project-scoped, so rather than deleting all
  the TODOs along with the feature that happened to host them, AMF
  reassigns the list's `feature_id` to a different surviving feature of
  the project (so the TODOs live on). When the host feature is deleted
  but the project remains, show a small prompt: **Re-home** (pick which
  surviving feature now hosts the list) or **Delete** (drop the list and
  its TODOs). If no other features remain, only Delete applies.
- **Spawn when a session is already linked → reuse and add on.** If the
  TODO already has a `spawned_session_id` pointing at a live session,
  jump to that session and inject the prompt onto it (seeded/editable)
  rather than creating a second session. Create a new session only when
  there is no live linked session.
- **Spawned-session agent settings → inherit.** Use the host feature's
  configured agent / vibe / plan settings; no prompt.
- **Done items → keep visible, delete per-item.** Completed TODOs stay
  shown (strikethrough / grouped) indefinitely. There is no bulk "clear
  completed"; the per-item delete (`d`) fully removes an item. Deleting
  a TODO whose session is linked leaves the session untouched.

## Reasoning / when to build

This is a self-contained vertical slice — new tables, one session kind,
one native view — that reuses the existing session-spawn and
composer-seed machinery rather than inventing new agent plumbing. Build
it when there's appetite for a planning/triage surface inside AMF; the
spawn-from-TODO launcher is the differentiator over keeping a plain
markdown to-do file, and it leans entirely on paths that already exist.
