# Feature TODOs

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change.

Scoped to-do lists surfaced as a session kind and a native (non-tmux)
overlay:

- **Three scopes, one type.** `TodoScope::{Worktree, Project, Global}`
  (`src/db/todos.rs`) is the whole key to a list: a worktree list is keyed
  by **workdir path** (not feature id — the list belongs to the checkout,
  not to whichever row points at it), a project list by project id, and the
  global list by nothing at all. The variants are declared narrowest-first
  and `rank()` makes that order explicit, because it is also the order ties
  between scopes resolve.
- **Session kind:** `SessionKind::Todos` (`src/project.rs`).
  `is_agent_harness()` and `is_tmux_backed()` are both `false`, so no tmux
  window is created and it is filtered out of window-cycling / the switcher.
  Offered in the `s` picker once per **feature**
  (`Feature::has_todos_session()`); the project-level gate it replaced is
  gone. Creating one creates the list its editor opens on — the feature's
  worktree list, or the project's at the repo root.
- **Persistence:** SQLite tables `todo_lists` and `todos`, created by
  `MIGRATION_010`/`011` and reshaped by `MIGRATION_025`, accessed via
  `src/db/todos.rs`. 025 is a **table rebuild**, not an `ALTER`: dropping
  `project_id UNIQUE` and relaxing `project_id`/`feature_id` to nullable
  cannot be expressed any other way. It runs with `foreign_keys` off in
  both directions on purpose — `DROP TABLE todo_lists` with them on would
  fire `todos`' `ON DELETE CASCADE` and take every TODO with it, and
  `ALTER TABLE ... RENAME` with them on would rewrite `todos`' `REFERENCES`
  clause to the temporary name. Three partial unique indexes replace the old
  single UNIQUE: one project list per project, one worktree list per
  (project, workdir), one global list per machine. Every pre-existing row is
  backfilled to `scope='project'` with its id, host feature, and
  `carry_over` scratchpad intact. Todos survive without a DB (in-memory),
  which is what the tests exercise; the in-memory panes are the overlay's
  source of truth and persist when a DB is present.
- **Native view:** `AppMode::Todos(TodoViewState)` (opened from the session
  via `enter_view` → `open_todos_view`), holding `panes: Vec<TodoPane>`
  ordered **worktree → project → global** — the same order the tie-break
  uses. The worktree pane is absent for a feature on the repo root, which is
  the only way the vector is shorter than three. Each pane owns its list,
  items, cursor, scroll, and scratchpad banner (whose DB column is still the
  legacy name `carry_over`), so moving focus never disturbs the pane being
  left. Lists are *loaded* on open and created lazily on first write
  (`todos_ensure_list_id_for`), so an untouched scope leaves no row behind.
  An inline `TextEditor` handles title/notes/scratchpad edits; `Ctrl+T` opts
  it into the Vim keymap (`todos_toggle_edit_vim`), remembered on
  `TodoViewState::todo_vim_enabled` for the life of the overlay, and `Ctrl+Q`
  cancels an edit (the escape hatch Vim's `Esc` gives up to Insert→Normal).
- **What "visible" means, in one rule.** `TodoViewState::pane_is_visible`
  (also `visible_pane_indices`, used by both draw and key handling) and
  `App::visible_todo_scopes()` (scan) implement the same thing: the
  worktree pane is always visible, and the project and global panes are
  each gated by their own independent flag, `AppConfig::todo_project_visible`
  and `todo_global_visible` — either, both, or neither can be hidden, with
  `focus: Option<usize>` (not a bare index) covering the case where every
  optional pane is hidden on a repo-root feature. The flags are app-level
  rather than per-overlay **because the dashboard's `I` runs with no
  overlay open** and still needs a defined notion of which scopes count.
- **Layout:** `pane_slots` decides which panes get a column at the current
  width (3 at ≥120 cols, 2 at ≥72, else 1). Two rules in order: the focused
  pane is always drawn, and the worktree pane keeps its slot whenever there
  is room for a second.
- **Keys added to the overlay:** `Tab`/`BackTab` cycle focus among visible
  panes, `p`/`g` independently toggle the project/global panes on or off
  (hiding the focused pane advances focus to the next visible one), `M`/`C`
  move/copy the selected item to another scope. `M`/`C` offer every *other*
  pane, visible or not — the scopes exist regardless of the toggles.
- **Move vs copy is a semantic difference, not a convenience one.** A
  **move** (`move_todo`) leaves `agent_session_id`, `linked_feature_id`, and
  `status` untouched: it is the same work, re-filed. A **copy** (`copy_todo`)
  clears the session and feature links and resets `status` to not-started
  (a completed source copies as completed — that half of the state is worth
  keeping), so two panes never both claim one session and "implement next"
  does not hold both in reserve for work only one of them describes. Both
  append at the destination's `sort_order` end.
- **Spawn from a TODO:** `g`/`Enter` resolves a linked feature, then a live
  linked session, then opens the chooser. A **worktree** TODO spawns in the
  feature that owns the checkout, inheriting its agent + mode. A **project**
  or **global** TODO belongs to no one checkout, so `launch_todo_in_scope`
  opens `AppMode::TodoSpawnTarget` — a feature picker (that project's
  features, or every project's for a global TODO) whose choice supplies the
  agent and mode. It stashes the origin mode as `Box<AppMode>` and restores
  it verbatim on cancel, for the same reason `TodoImplementChoice` does: one
  of its two callers has no overlay open.
- **Sessions are looked up store-wide** (`session_indices_by_id`), not
  inside the list's host feature. A TODO's agent used to be guaranteed to
  live there; with project/global scopes and cross-scope moves it is not, so
  "is this session alive?" is a question about the session rather than about
  which list holds the row. `todos_reconcile_dead_sessions` uses the same
  lookup, so a session is dead only when it exists in **no** feature.
- **Implement next (`I`):** `next_todo_across(&[&[Todo]], skipped)` is pure
  and scope-aware; `next_todo_index` is its one-list form, kept `#[cfg(test)]`
  so the per-list rules can be pinned down alone. It concatenates the lists
  in scope order and **stable**-sorts by `TodoPriority::rank`, which gives
  exactly the intended rule: priority first, scope as the between-list
  tie-break, manual `sort_order` as the within-list one. Completed and
  in-progress items (`Todo::is_eligible_for_automatic_spawn`, i.e.
  `status != NotStarted`) and explicitly-skipped ids are passed over. A TODO
  that links a session or a planned feature is **held in reserve, not
  skipped** — any unstarted item in any visible scope outranks it, and it is
  only returned (as `NextTodo::Started`) when nothing unstarted remains
  anywhere.
- **Status (`TodoStatus`, `MIGRATION_028`):** `NotStarted` / `InProgress` /
  `Completed`, stored as a checked `status` TEXT column that replaced the
  earlier boolean `todos.in_progress` from `MIGRATION_024` — one exhaustive
  value instead of two flags that could disagree. Paired with
  `agent_session_id` (also added by `MIGRATION_028`; a
  [`crate::project::FeatureSession`] id, harness-neutral and stable across
  restarts) inside `TodoWorkState`, which is the only thing allowed to
  change either field: `reserve_launch` claims a not-started TODO before its
  session exists (`InProgress`, no association yet — this is what a spawn
  sets, via `todos_reserve_launch`, before the launch can fail),
  `associate_session` attaches the real session id once created
  (`todos_mark_started`, only while still `InProgress`, so a late result
  can't attach after a manual status change), `rollback_launch` reverts a
  failed creation or prompt-delivery back to `NotStarted` with no
  association (`todos_rollback_launch`, called through a best-effort wrapper
  on failure paths so a rollback write failing can't replace the original,
  actionable launch error), and `clear_missing_session` drops a stale
  session id **without** touching `status`. A session link survives
  abandonment on purpose — it is what lets a repeat spawn attempt find and
  offer the work already started, and what's absent for a TODO marked
  in-progress by hand.
  `status` only changes on completion or the manual `i` cycle
  (`TodoWorkState::cycle_manually`); a dead associated session
  (`todos_reconcile_dead_sessions`, `reconcile_todo_agent_associations`, run
  from ordinary status sync including startup) clears **only** the link, not
  the flag — "a missing agent does not make work unstarted again." Stopping
  the host feature clears nothing either: stopped work is still in
  progress.
- **The already-started prompt** is `AppMode::TodoImplementChoice`, not a
  `TodoLaunchStep`, because only one of its two surfaces has a
  `TodoViewState`. It carries the candidate's `pane_kind` and `list_id` so
  *Start another agent on it* routes through the same scope rule as a fresh
  spawn, and so the item is re-resolved on confirm with no overlay open. Its
  *Go to the work already started* self-heals like `g`/`Enter` does: a
  `linked_feature_id` whose feature is gone is cleared, or the link — the
  only thing holding the item back from `Ready` — would make every later `I`
  re-offer it. It stashes the mode it was opened from as `Box<AppMode>` and
  restores it verbatim on every exit, so `Esc` from the overlay costs nothing
  — cursor, scroll, and any DB-less in-memory rows are the same objects, not
  a reload. (Nothing shows *through* it: like every modal here,
  `draw_modal_overlay` clears the viewport first.) *Skip to next* accumulates
  **ids**, not indices, and re-derives the lists each round, so the prompt
  survives them changing underneath it.
- **Quick-capture:** `AppMode::TodoQuickCapture`, reached from an embedded
  session view via leader → `N`, appends a one-line TODO to the scope
  `default_todo_scope(pi, fi)` resolves — the session feature's worktree
  list, or the project's at the repo root — auto-creating the list + session
  if none exists. The overlay *names* that list (`list_label`), because the
  target is not the one thing on screen. Learning Mode's keep-as-TODO (`a`)
  uses the same rule, and its jump-back searches every pane rather than
  guessing a scope.
- **Feature deletion:** `delete_feature` stops **before** anything
  destructive and opens `AppMode::TodoDeleteDisposition` when
  `pending_todo_disposition` finds unfinished items in the feature's worktree
  list — move them to the project list, move them to the global list, delete
  them with the worktree, or cancel. Deleting a worktree is hard to reverse,
  so the prompt is blocking and cancel leaves the feature intact.
  `apply_todo_disposition` is split out from the confirm handler so the
  re-filing can be tested without driving a real tmux kill and worktree
  removal. When *move to the project list* has to create that list, its host
  is a feature that **survives** the deletion (never the doomed one, which is
  why the state carries `feature_id`), and none at all when there is no
  survivor: hosting it on the feature being deleted would hand it straight to
  `handle_todos_host_feature_deleted` below, which drops an orphaned list —
  losing the items the user just chose to keep. Deleting a *project* removes
  its project list and every worktree
  list under it (`delete_todo_lists_for_project`); the global list belongs to
  no project and survives.
- **Host-feature deletion:** when the feature hosting the **project** list is
  deleted but the project survives, `complete_deleting_feature` calls
  `handle_todos_host_feature_deleted`, which either silently drops the
  orphaned list (no features remain) or opens `AppMode::TodosHostReassign` —
  a prompt to **re-home** the list onto a surviving feature
  (`set_todo_list_host_feature`) or **delete** it. `Esc` keeps the list by
  re-homing onto the first surviving feature. Worktree lists have no host to
  reassign: they were already settled by the disposition prompt.
