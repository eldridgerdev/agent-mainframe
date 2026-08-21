# Feature TODOs

- **Status:** All epics shipped, including Epic 8 (scoped lists —
  worktree / project / global) and Epic 7 (plan mode from a TODO). Epics
  2–6 (session kind, native view, editing, spawn agent from a TODO,
  quick-capture + scratchpad, help-overlay wiring, docs) plus Epic 1's
  final item — the host-feature deletion re-home/delete prompt
  (`AppMode::TodosHostReassign`) — were complete before those.
- **Superseded:** Epic 8 reversed two of the original interview
  decisions — one list per project, and one TODOs session per project.
  Both are marked below where they appear.
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
- ~~**At most one TODOS session per project.**~~ **Superseded by Epic
  8:** one TODOs session per **feature**, gated by
  `Feature::has_todos_session()`. A feature's editor opens on its own
  worktree list, with the project and global lists reachable as side
  panes. The original decision — one session per project, hosted by
  whichever feature created it — is what Epic 8 exists to undo: it put
  work belonging to one checkout in the same list as work belonging to
  every other.
- **Native UI, not tmux-backed.** Opening the TODOS session enters a
  native AMF overlay/mode rather than attaching to a tmux pane.
  `SessionKind::Todos.is_agent_harness()` is `false` and no tmux window
  is created for it.
- **SQLite persistence.** Stored in the existing `amf.db` via a new
  migration and access module — not in the `ProjectStore` JSON blob.
- **Spawn into the host feature.** Launching an agent for a TODO
  creates the new session inside the feature the TODO belongs to (same
  worktree/branch), seeds the composer with a generated prompt, and
  leaves it **editable before sending** (seeded, not auto-submitted).
  **Amended by Epic 8:** still true for a worktree-scoped TODO. A
  project- or global-scoped TODO belongs to no one checkout, so there is
  nothing to infer and the user picks the feature; that feature then
  supplies the agent and mode exactly as a host feature would.
- **Item fields:** done checkbox, priority, notes/detail body, and a
  link to the spawned session.
- **Extras in scope:** reorder items, editable composer prompt before
  launch, a list-level "left off here" carry-over note, and
  quick-capture of a TODO from inside any session view.

## Proposed design

> **Note:** this section records the design as originally proposed. The
> schema and the one-session-per-project rule below were both reshaped
> by **Epic 8** — see its checklist for the shipped shape
> (`MIGRATION_025`, `TodoScope`, `Feature::has_todos_session()`).

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

- [x] Add `MIGRATION_010` creating `todo_lists` + `todos` tables.
- [x] Add `src/db/todos.rs` with row structs and CRUD/query fns.
- [x] Expose `AmfDb` methods (load/upsert list, list/upsert/delete/
      reorder todos).
- [x] Cascade-delete todos & list when a project is deleted
      (explicit cleanup in `app/project_ops.rs::delete_project`, since
      there is no FK cascade from `projects`).
- [x] On host-feature deletion (project survives), prompt **Re-home**
      (reassign `feature_id` to a chosen surviving feature) or
      **Delete** the list; Delete-only when no features remain.
      Implemented as `AppMode::TodosHostReassign`: after
      `complete_deleting_feature` removes the host feature,
      `handle_todos_host_feature_deleted` either silently drops the
      orphaned list (no survivors) or opens the re-home/delete prompt
      (`handlers/todos.rs::handle_todos_host_reassign_key`,
      `ui/dialogs/todos.rs::draw_todos_host_reassign_dialog`). `Esc`
      keeps the list by re-homing onto the first surviving feature.
      Tests: `host_feature_delete_prompts_rehome_onto_surviving_feature`,
      `host_feature_delete_can_delete_the_list`,
      `host_feature_delete_drops_list_when_no_features_remain`,
      `deleting_non_host_feature_leaves_todo_list_untouched`.
- [x] Unit tests for migration + round-trip CRUD + one-list-per-project
      constraint.

### Epic 2 — Session kind & tree integration

- [x] Add `SessionKind::Todos`; `is_agent_harness()` → false (plus
      `is_tmux_backed()` → false); handle in all `match` sites (ui
      labels/icons, sync signature, status, store serialization).
- [x] Offer "TODOs" in the `s` picker only when the project has none
      (`Project::has_todos_session()`); create the `todo_lists` row on
      selection via `load_or_create_todo_list`.
- [x] Skip tmux window creation for `Todos` sessions in
      `session_ops.rs` (`add_todos_session_for_picker`, branched before
      `ensure_feature_running_for_new_session`).
- [x] Render the TODOS session row in the feature session list (tree +
      both pickers + switcher; non-tmux sessions filtered from
      window-cycling and the switcher).
- [x] Block creating a second TODOS session per project (test:
      `add_builtin_session_blocks_second_todos_per_project`, plus picker
      offer/hide tests).
- Opening a TODOs session now routes straight to the native overlay
  (Epic 3); the placeholder "coming soon" message is gone.

### Epic 3 — Native TODO view (read + navigate)

- [x] `AppMode::Todos(TodoViewState)` + open/close from the session
      (`enter_view` routes TODOs sessions to `open_todos_view`;
      `close_todos_view` reselects the session).
- [x] `src/app/todos.rs` load + state; `src/handlers/todos.rs` dispatch.
- [x] `src/ui/dialogs/todos.rs` rendering: full-screen header (open/done
      counts), carry-over banner, list with cursor, priority marker,
      checkbox, strikethrough on done, notes indicator, scrollbar.
- [x] `j/k` navigation and exit (`Esc`/`q`/`Ctrl+Q`).

### Epic 4 — TODO editing

- [x] Add (`a`/`n`) with inline editor (reuses `TextEditor`); persist.
- [x] Edit title (`e`) and notes body (`o`); `Alt+Enter` newline,
      `Enter` commit, `Esc` cancel (mirrors compose).
- [x] Toggle done (`space`/`x`); completed items stay visible
      (strikethrough) and sink below open items, no bulk clear.
- [x] Cycle priority (`p`, High→Med→Low). Note: ordering is `done` then
      manual `sort_order`; priority is a visual marker, not a sort key
      (matches the DB query + the "grouped by done then sort_order" UI
      spec). Revisit if priority-sorting is wanted.
- [x] Reorder (`J/K`) updating `sort_order`; persist via `reorder_todos`.
- [x] Delete (`d`) with y/n confirm overlay; linked session untouched.
- [x] Edit carry-over "left off here" banner (`b`).
- Edits mutate the in-memory state (works without a DB, e.g. tests) and
  persist when a DB is present; the in-memory list is the overlay's
  source of truth.

### Epic 5 — Spawn agent from a TODO

- [x] `g`/`Enter` creates a new agent-harness session in the host
      feature (inheriting its agent/vibe/plan settings) and switches to
      it. If the TODO already links a live session, jump to it and add
      onto it instead of creating a second. (`todos_spawn_agent` resolves
      the host feature from `list.feature_id`; the launch reuses a new
      generic `create_agent_session_labeled` extracted from the PR-review
      session path, running the host feature's `agent` + mode/flags.)
- [x] Seed composer (editable, not submitted) with the generated prompt
      from title + body (`todo_spawn_prompt` → `open_compose_seeded`).
- [x] Record `spawned_session_id`; show "launched" marker (green ▸ /
      nerd-font icon in the row) and allow jump-back — the reuse path
      jumps to the linked live session when `g`/`Enter` is pressed again.

### Epic 6 — Quick-capture & polish

- [x] View-mode keystroke to append a TODO to the project list. Bound to
      leader `N` ("New TODO") in the embedded session view — `t`/`T` were
      already session-cycling. Opens a one-line
      `AppMode::TodoQuickCapture` overlay (`open_todo_quick_capture`,
      `handlers/todos.rs::handle_todo_quick_capture_key`,
      `ui/dialogs/todos.rs::draw_todo_quick_capture_dialog`); Enter appends,
      Esc cancels, empty title is a no-op.
- [x] When no TODOS session exists yet, auto-create the list + session
      under the current feature, then append
      (`quick_capture_todo` reuses `add_todos_session_for_picker`, a no-op
      when the project already has a TODOs session, then
      `load_or_create_todo_list` + `add_todo`). Works DB-less (tests): the
      session is created in memory; persistence is skipped without a DB.
- [x] Generalize the "left off here" carry-over banner into a generic
      list-level **scratchpad** section. Decisions: labeled **"Scratchpad"**
      (per-item body is already "notes", so "Notes" would collide); `b` stays
      the edit key, hint reads `b scratch`; multi-line already supported.
      - **DB column kept as `carry_over`** — no migration. The persistence
        layer (`TodoList.carry_over` field, `set_carry_over` /
        `set_todo_carry_over` methods, SQL column) is unchanged; only the
        app/UI surface is renamed.
      - App/UI renames: `TodoEditTarget::CarryOver` → `Scratchpad`,
        `todos_begin_edit_carry_over` → `todos_begin_edit_scratchpad`,
        `todos_set_carry_over` → `todos_set_scratchpad`,
        `draw_carry_over` → `draw_scratchpad`; banner/editor titles
        "Left off here" → "Scratchpad". A bridging comment notes the legacy
        column name. Test renamed to `todos_edit_scratchpad_banner`.
- [x] Wire keys into help overlay (`ui/dialogs/help.rs`): added an
      "In the TODOs view" section documenting nav/add/edit/notes/
      scratchpad/toggle/priority/reorder/delete/spawn/exit keys.
      Config-wizard remap is intentionally **not** wired: the wizard's
      `DASHBOARD_KEYBINDING_ACTIONS` only covers dashboard normal-mode
      actions, and the TODOs overlay keys are modal (like the embedded
      view and PR-review keysets), which follow the same
      documented-but-not-remappable pattern. Quick-capture `N` is
      already listed under the embedded-view keybinds.
- [x] Update `CLAUDE.md` architecture notes and `CHANGELOG.md`.
      Added a "Feature TODOs" subsection to `CLAUDE.md` (session
      kind, SQLite `db/todos.rs` + `MIGRATION_010`, `AppMode::Todos`
      native view, spawn-from-TODO, quick-capture) plus `todos.rs`
      entries in the app/handlers/ui-dialogs module listings. The
      `CHANGELOG.md` "Per-project TODO lists" entry under v0.29.0
      already covers the user-facing surface, so it was left as-is.

### Epic 7 — Plan mode from a TODO

Shipped. `g`/`Enter` no longer spawns straight away: it resolves what the
key should mean, then offers the choice it cannot resolve. The decisions
below come from a feature-discovery interview; the working plan it produced
lived in the branch's (gitignored) `AMF_PLAN.md`, so its conclusions are
recorded here rather than linked.

- [x] `g`/`Enter` resolves before it asks: a linked feature wins, then a
      live linked session (today's behavior), then the chooser
      (`todos_launch_selected`). A link whose target is gone is dropped
      and announced rather than failing again.
- [x] Chooser + destination step as a **layer over the list**
      (`TodoLaunchStep` in `TodoViewState.launch`), not new `AppMode`
      variants — the same reason `pending_delete` and `editor` are
      fields: a separate mode replaces `TodoViewState` wholesale, forcing
      a reload and discarding the in-memory list that is the overlay's
      source of truth. `Esc` unwinds one step at a time.
- [x] Brief composed from every field the row carries — title, notes,
      list scratchpad, plus a bounded **provenance** paragraph
      (`compose_plan_brief` + `todo_provenance`). Provenance states *that*
      work was started, not a transcript of it: AMF keeps no per-TODO
      history (transcripts are workdir-scoped and effectively Claude-only,
      a tmux capture holds only what is still on screen), so quoting
      either would put text in the brief that does not describe this TODO.
      The brief opens in the interview's existing editable `Brief` phase.
- [x] Host-feature destination: interview keyed
      `todo_interview_key(todo_id)` → `todo:<id>`, **not** the host
      feature's id, which is where that feature's own `P` draft and
      accepted transcript live and would otherwise be overwritten and
      pre-filled from. Draft/resume comes free from `plan_interviews`
      (`MIGRATION_016`), so no new draft storage was needed.
- [x] Host-feature accept writes `AMF_PLAN.todo-<slug>.md` **beside** the
      feature's own `AMF_PLAN.md` rather than over it, spawns a session in
      the host feature, records `spawned_session_id`, and seeds the
      composer with a kickoff prompt that names the file explicitly — the
      harness's injected instruction block still points at `AMF_PLAN.md`,
      so an unqualified "read the plan" would send the agent to the
      feature's plan instead of this one.
- [x] New-feature destination reuses the `n` create-feature wizard
      pre-seeded (branch = slugified title, agent/mode from the host
      feature, plan mode forced on). No new creation path: the wizard
      already checks out the worktree *before* the interview when plan
      mode is on, so "created up front" needed no new code. The AMF
      `Feature` row is still deferred to accept, which is the first moment
      `linked_feature_id` can be written.
- [x] `TodoPlanOrigin` threaded through `PreparedFeatureLaunch`,
      `HookNext::WorktreeCreated`, `RunningHookState`, and
      `BackgroundHook`: the `on_worktree_created` detour rebuilds the
      launch from scratch and would otherwise drop the link silently on
      any project with that hook.
- [x] `MIGRATION_023` adds `todos.linked_feature_id`, a separate link
      from `spawned_session_id` (a session inside the host feature) since
      a TODO can carry both. Cleared explicitly on feature deletion
      (`clear_todo_links_to_deleted_feature`) — no FK, same as every other
      id in these tables.
- [x] Row indicators for a linked feature, `README.md` / help-overlay /
      `CHANGELOG.md` updates, and unit coverage for the resolver, the step
      machine, brief composition, plan-file naming, and link clearing.

**Verified by running the app** (`scripts/dev/screenshot/amf-capture.sh`,
throwaway repo + scratch instance): both destinations reach the interview
with the brief pre-filled; the new-feature path checks out a real worktree
before the interview opens; and an accept into a host feature that already
had a plan left that plan untouched, wrote the per-TODO file beside it,
spawned the labelled session, and seeded the composer unsent. Fixed while
walking it: wrapped option details lost their hanging indent, since
`Paragraph` wrapping restarts continuation lines at column zero
(`detail_lines`).

### Epic 8 — Scoped lists (worktree / project / global)

Shipped. Undoes this doc's founding assumption that a project has one
list. Work belonging to one checkout no longer sits in the same list as
work belonging to every other, and the first feature to add a TODOs
session no longer claims it for the whole repo. The decisions below come
from a feature-discovery interview; the working plan lived in the
branch's (gitignored) `AMF_PLAN.md`, so its conclusions are recorded
here rather than linked.

- [x] `TodoScope::{Worktree, Project, Global}` (`src/db/todos.rs`) is
      the whole key to a list. A worktree list is keyed by **workdir
      path**, not feature id — the list belongs to the checkout, not to
      whichever row points at it. The variants are declared
      narrowest-first and `rank()` makes that order explicit, because it
      is also the order ties between scopes resolve.
- [x] `MIGRATION_025` reshapes `todo_lists`: adds `scope` and `workdir`,
      relaxes `project_id`/`feature_id` to nullable, and replaces the
      `project_id UNIQUE` constraint with three partial unique indexes
      (one project list per project, one worktree list per
      (project, workdir), one global list per machine). A **table
      rebuild**, not an `ALTER` — the last two changes cannot be
      expressed any other way in SQLite — run with `foreign_keys` off in
      both directions: with them on, `DROP TABLE todo_lists` would fire
      `todos`' `ON DELETE CASCADE` and take every TODO in the database
      with it, and `ALTER TABLE ... RENAME` would rewrite `todos`' own
      `REFERENCES` clause to the temporary name. Existing rows are
      backfilled to `scope='project'` with id, host feature, scratchpad,
      and links intact.
- [x] `Project::has_todos_session()` replaced by
      `Feature::has_todos_session()`; both `s`-picker gates and the
      create guard updated. A repo-root feature still gets a session —
      its editor opens on the project + global panes, and under the
      side-pane-only entry point that is its only route to the global
      list.
- [x] `TodoViewState` holds `panes: Vec<TodoPane>` ordered worktree →
      project → global, each owning its list, items, cursor, scroll, and
      scratchpad. Lists are *loaded* on open and created lazily on first
      write, so an untouched scope leaves no row behind.
- [x] One rule for "visible", used by both the draw
      (`visible_pane_count`) and the scan (`visible_todo_scopes`): the
      worktree pane alone until the side panes are revealed, and *all*
      panes for a feature that has none — closing them there would leave
      nothing. The reveal is `AppConfig::todo_side_panes`, app-level
      rather than per-overlay **because the dashboard's `I` runs with no
      overlay open** and still needs a defined answer.
- [x] New overlay keys, verified free against the live dispatch before
      committing to them: `Tab`/`BackTab` cycle focus (and say to press
      `\` when there is only one pane rather than swallowing the press),
      `\` toggles the side panes, `M`/`C` move/copy across scopes.
      `pane_slots` handles narrow terminals — 3 panes at ≥120 cols, 2 at
      ≥72, 1 below, with the focused pane always drawn and the worktree
      pane keeping its slot whenever there is room for a second.
- [x] Move vs copy is a semantic difference: `move_todo` carries
      `spawned_session_id`, `linked_feature_id`, and `in_progress` — the
      same work, re-filed — while `copy_todo` clears all three, so two
      panes never claim one session and "implement next" does not hold
      both in reserve for work only one of them describes. Both append
      at the destination's `sort_order` end.
- [x] `next_todo_across(&[&[Todo]], skipped)` generalises
      `next_todo_index` (kept as its `#[cfg(test)]` one-list form). It
      concatenates the lists in scope order and **stable**-sorts by
      priority rank, which gives the intended rule exactly: priority
      first, scope as the between-list tie-break, manual `sort_order` as
      the within-list one. The reserve-not-skip rule for started items is
      unchanged, now across scopes.
- [x] `AppMode::TodoSpawnTarget` — a feature picker for a project- or
      global-scoped spawn, stashing its origin mode as `Box<AppMode>` for
      the same reason `TodoImplementChoice` does: one of its two callers
      has no overlay open. A project TODO lists that project's features,
      a global one lists every project's.
- [x] Session lookup is now **store-wide** (`session_indices_by_id`). A
      TODO's agent used to be guaranteed to live in the list's host
      feature; with project/global scopes and cross-scope moves it is
      not, so "is this session still alive?" became a question about the
      session rather than about which list holds the row.
      `todos_reconcile_dead_sessions` uses the same lookup.
- [x] `AppMode::TodoDeleteDisposition` gates `delete_feature` when the
      feature's worktree list still holds unfinished items — move to the
      project list, move to the global list, delete, or cancel. Blocking
      by design: deleting a worktree is hard to reverse, and nothing is
      killed or removed until it is answered. `apply_todo_disposition` is
      split out from the confirm handler so the re-filing is testable
      without driving a real tmux kill and worktree removal.
- [x] Quick-capture (`Ctrl+Space` `N`) and Learning Mode's keep-as-TODO
      both target `default_todo_scope(pi, fi)` — the feature's worktree
      list, or the project's at the repo root — and the capture overlay
      names the list it will write to.
- [x] Project deletion drops the project's list **and** every worktree
      list under it (`delete_todo_lists_for_project`); the global list
      belongs to no project and survives.
- [x] `README.md`, help overlay, `CLAUDE.md`'s Feature TODOs section, and
      `CHANGELOG.md` updated; 27 new unit tests covering scope-aware
      selection, move/copy link semantics, quick-capture target
      resolution, pane visibility and focus, and every disposition
      outcome.

**Verified against a real database:** `MIGRATION_025` was run over a copy
of a live `~/.config/amf/amf.db` — same list id, same host feature, all 23
items intact, `carry_over` preserved, and `PRAGMA foreign_key_check`
clean afterwards, with `todos` still referencing the rebuilt table.

**Verified by running the app** (`scripts/dev/screenshot/amf-capture.sh`
with `scenarios/todo-scopes.txt`, throwaway repo + scratch instance): the
editor opens on the worktree pane alone, `\` reveals all three, `M` moves
an item to the global list, `I` takes the worktree item without asking and
then asks which feature should work the global one, and deleting the
feature raises the disposition prompt — with cancel leaving the feature,
its sessions, and its worktree on disk untouched. Also checked at 200 /
100 / 60 columns for the narrow-terminal fallback.

## Open (not built)

- **Cancelling after the worktree exists** leaves an orphan checkout with
  no `Feature` row. Existing behavior keeps it and says so
  (`cancel_plan_interview_feature`); unresolved whether a TODO-originated
  cancel should offer to remove it.
- **Re-planning a linked TODO.** Once linked, `g` jumps to the feature.
  Whether the interview can be re-run from the row (the way `P` re-runs on
  a feature) was never decided.
- **New-feature accept is unproven by a real run** — the
  `linked_feature_id` write and the harness kickoff need a live harness
  rather than a seeded draft. Pi's seeding path specifically is untested.
- **Plan mode from a project- or global-scoped TODO** still offers
  "here, in the host feature", resolving to whichever feature the editor
  was opened under. Epic 8's pick-a-feature decision was scoped to
  spawning an agent (`g`/`Enter`/`I`), not to the plan interview.
- **Worktree keys are only separator-normalised** (`todo_workdir_key`).
  Symlinks and case are not, so one checkout reachable by two different
  real paths would get two lists. Left as-is because AMF stores the
  workdir it created, so the paths it compares come from one source.
- **The feature picker does not remember a target per TODO.** The cursor
  defaults to the feature the editor was opened under, which was one
  keypress in practice; revisit only if that default proves wrong often.
- **The global list has no standalone entry point** — no dashboard key,
  no `s`-picker entry, no leader command. It is reachable only as a side
  pane of a TODO editor, which is also why a repo-root feature has to
  keep getting a TODOs session.

## Resolved decisions

- **Quick-capture with no list yet → auto-create.** If the feature has
  no TODOS session, quick-capture silently creates the `todo_lists` row
  (and the `Todos` session under the current feature) before appending
  the item, so capture never fails. **Amended by Epic 8:** the target is
  the feature's own worktree list, or the project's when the feature sits
  on the repo root, and the overlay names it.
- **Host-feature deletion → prompt to re-home _or_ delete.** (Epic 8:
  this is now about the **project**-scoped list only. A worktree list
  belongs to a checkout rather than to a host feature, and it is settled
  by the disposition prompt before the deletion runs.) "Re-home"
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
  configured agent / vibe / plan settings; no prompt. **Amended by Epic
  8:** unchanged for a worktree TODO. For a project or global TODO the
  settings are inherited from the feature the user picks — the prompt is
  about *where*, never about the settings.
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
