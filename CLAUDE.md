# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code)
when working with code in this repository.

## Build and Run

```bash
cargo build            # debug build
cargo run              # run the TUI (binary name: amf)
cargo build --release  # release build
cargo check            # type-check without building
cargo clippy           # lint
```

The binary is named `amf` (agent-mainframe). The package
name in Cargo.toml is `agent-mainframe`. There are no tests
yet.

## Runtime Requirements

- **tmux** must be installed and in PATH (checked at startup)
- **claude** CLI (Claude Code) is launched inside tmux
  sessions

## Architecture

Rust TUI application that manages multiple concurrent Claude
Code agent sessions, each running in its own tmux session.
Built with ratatui 0.29 / crossterm 0.28 / vt100 0.15.
Uses Rust 2024 edition.

### Data Model (project.rs)

```text
ProjectStore (version: u32, projects: Vec<Project>)
  └─ Project (id, name, repo: PathBuf, collapsed, features,
             created_at)
       └─ Feature (id, name, branch, workdir: PathBuf,
                   is_worktree, tmux_session, claude_session_id,
                   status: ProjectStatus, created_at,
                   last_accessed)

ProjectStatus: Active | Idle | Stopped
```

State is persisted in the SQLite database at
`~/.config/amf/amf.db` for every checkout. AMF resolves this single
global store regardless of which directory you launch it from.
Tmux sessions are prefixed `amf-` (e.g., `amf-mybranch`).

### App State & Modes (app/)

The `app/` directory is split into focused submodules:

```text
app/
├── mod.rs           # App struct, AppConfig, ZaiPlanConfig,
│                    # new(), save(), re-exports
├── state.rs         # AppMode, Selection, ViewState,
│                    # CreateProjectState, etc.
├── navigation.rs    # visible_items(), select_next/prev(),
│                    # selected_project/feature/session()
├── sync.rs          # sync_statuses(), thinking status
├── project_ops.rs   # toggle_collapse(), create/delete project,
│                    # browse path
├── feature_ops.rs   # create/start/stop/delete feature
├── session_ops.rs   # session picker, add/remove sessions
├── view.rs          # enter/exit view, leader key, scroll,
│                    # view navigation
├── switcher.rs      # session switcher
├── notifications.rs # scan_notifications(), handle select
├── hooks.rs         # lifecycle hooks
├── opencode.rs      # opencode session management
├── search.rs        # search and jump
├── commands.rs      # command picker
├── rename.rs        # session renaming
├── review.rs        # trigger_final_review()
├── review_destination.rs # final-review "dispatch fixes to…" picker:
│                    # this feature / a dedicated session / another
│                    # feature / a new companion feature (own worktree,
│                    # ReviewSource link, push-or-cherry-pick integration)
├── plan_interview.rs # guided discovery, AI rounds, plan review
├── todos.rs         # scoped TODOs overlay (worktree/project/global
│                    # panes, add, edit, toggle, reorder, move/copy,
│                    # spawn agent, delete-time disposition)
├── learning.rs      # Learning Mode overlay: browse, select,
│                    # prompt builders, headless queue, answer
│                    # actions (follow-up, deep dive, re-file,
│                    # keep, escalate)
├── prompt_overrides.rs # headless-prompt override manager overlay:
│                    # list every registry prompt + effective source,
│                    # edit → scope picker (feature/project/global) →
│                    # harness picker → save (DB or amf.json), clear
├── precall.rs       # pre-call notice: PrecallAction, PendingPrecall,
│                    # precall_gate() (blocks user-initiated headless
│                    # runs before spawn), precall_confirm re-dispatches
│                    # the start_* method, announce_headless_run() toast
│                    # for automated runs
├── resource_gate.rs # pre-start agent/memory gate, pending-start
│                    # stash + replay, autostart policy
├── editor_ops.rs    # kill_tracked_editors(): close editors AMF
│                    # opened, with killed/skipped report
├── dormant.rs       # dormant detection + overlay ops
├── setup.rs         # ensure_notification_hooks(),
│                    # ensure_notify_scripts(), load_config()
├── util.rs          # shorten_path(), slugify(),
│                    # detect_repo_path(), detect_branch()
└── tests.rs         # all #[cfg(test)] tests
```

Key App methods (spread across submodules):

- `new(store_path) -> Result<Self>`
- `save() -> Result<()>`
- `visible_items() -> Vec<VisibleItem>` - flattened tree
- `select_next/prev()` - wrapping navigation
- `sync_statuses()` - polls tmux sessions
- `selected_project() -> Option<&Project>`
- `selected_feature() -> Option<(&Project, &Feature)>`
- `toggle_collapse()`
- Project CRUD: `start_create_project()`,
  `create_project()`, `delete_project()`
- Feature CRUD: `start_create_feature()`,
  `create_feature()`, `start_feature()`,
  `stop_feature()`, `delete_feature()`
- View: `enter_view()`, `exit_view()`,
  `view_next/prev_feature()`, `switch_to_selected()`,
  `open_terminal()`
- Leader: `activate_leader()`, `deactivate_leader()`,
  `leader_timed_out()`

### Event Loop & Key Handling (main.rs)

`run_loop()` drives the event loop with 50ms poll in
Viewing mode, 250ms otherwise. Status sync every 5s.

Key dispatch per mode:

- `handle_normal_key()` - j/k nav, N/n create, Enter
  view/collapse, c start, x stop, s switch, d delete,
  h help, r refresh, K Learning Mode, q quit
- `handle_view_key()` - Ctrl+Q exit, Ctrl+Space leader,
  else forward to tmux via `crossterm_key_to_tmux()`
- `handle_leader_key()` - q/t/s/n/p/r/x/h after
  Ctrl+Space
- `handle_create_project_key()` - Enter/Tab/Backspace/Char
- `handle_create_feature_key()` - Enter/Backspace/Char
- `handle_delete_*_key()` - y confirm, n/Esc cancel
- `handle_help_key()` - Esc/q/h close

### External Tool Managers

**TmuxManager** (tmux.rs) - all static methods:

- `check_available()`, `session_exists(session)`
- `create_session(session, workdir)` - creates `claude` +
  `terminal` windows
- `launch_claude(session, resume_session_id)`
- `is_inside_tmux()`, `current_session()`
- `switch_client(session)`, `attach_session(session)`
- `kill_session(session)`, `list_sessions()` (filters
  `amf-*`)
- `capture_pane(session, window)`,
  `capture_pane_ansi(session, window)`
- `resize_pane(session, window, cols, rows)`
- `send_literal(session, window, text)`,
  `send_key_name(session, window, key_name)`,
  `send_keys(session, window, keys)`

**WorktreeManager** (worktree.rs) - all static methods:

- `repo_root(path) -> Result<PathBuf>`
- `is_worktree(path) -> bool`
- `create(repo, name, branch) -> Result<PathBuf>` -
  creates under `.worktrees/`, handles existing vs new
  branch
- `remove(repo, worktree_path)`
- `list(repo) -> Result<Vec<WorktreeInfo>>`
- `current_branch(path) -> Result<Option<String>>`

**ClaudeLauncher** (claude.rs):

- `check_available()`
- `launch_interactive(session, resume_id)`
- `run_headless(workdir, prompt) -> Result<String>`
- `run_headless_json(workdir, prompt) -> Result<String>`

**HeadlessRunner** (headless.rs):

- Harness-neutral one-shot runs for Claude, Codex, OpenCode, and Pi
- Restricted no-tools mode for context-complete prompts
- Read-only repository tools for directed plan revisions
- Harness selection and fallback for plan interviews

**Prompt registry** (`src/prompts/`): the single home for every headless
prompt AMF sends (see "Editable Headless Prompts" below). `mod.rs` holds
`PromptId` (15 stable ids), `PromptSpec` (title/summary/placeholders/
`default_template`/`harness_variants`), and `resolve_template_layered` /
`resolve_prompt_layered`. `defaults.rs` is the built-in template text moved
out of the call sites. `resolve.rs` has `PromptContext` +
`render_template` (unvalidated `{{token}}` substitution — missing/unknown
tokens render literally). `project.rs` reads project-scope overrides from
`amf.json`. Call sites resolve via `App::resolve_headless_prompt` /
`resolve_headless_template` (`app/mod.rs`), which assemble the feature (DB) →
project (`amf.json`) → global (DB) → built-in layers.

### UI Rendering (ui/)

`draw(frame, app)` in `ui/dashboard.rs` dispatches to:

- `draw_pane_view()` - full-screen embedded tmux with ANSI
  rendering via vt100 parser
- `draw_header()`, `draw_project_list()`,
  `draw_status_bar()`
- Dialog overlays in `ui/dialogs/`:
   - `project.rs` - create/delete project dialogs
   - `feature.rs` - create/delete feature, supervibe
     confirm, deleting feature progress
   - `session.rs` - rename session dialog
   - `help.rs` - keybindings help overlay
   - `browse.rs` - path browser dialog
   - `search.rs` - search dialog
   - `hooks.rs` - change reason, running hook, hook
     prompt dialogs
   - `plan_interview.rs` - discovery questions, loading frames,
     plan review, editing, critique, directed feedback, and isolated
     investigation
   - `todos.rs` - scoped TODOs pane view, delete confirm,
     move/copy scope chooser, spawn-target feature picker,
     delete-time disposition prompt, and quick-capture overlay
   - `learning.rs` - Learning Mode: file list, content pane,
     Q&A history, answer pane (markdown), starter/harness
     pickers, keep-as-TODO editor, help overlay
   - `prompt_overrides.rs` - override-manager list (source badge +
     `[F][P][G]` flags), template editor, scope picker, harness
     picker, help panel
   - `precall.rs` - pre-call notice (announce card ↔ full prompt view)
   - `resource_gate.rs` - pre-start agent/memory warning
   - `dormant.rs` - dormant-features overlay
   - `review_destination.rs` - final-review destination picker,
     companion-feature setup, and companion→source integration overlay
- `centered_rect(percent_x, percent_y, area) -> Rect`
- `ansi_to_ratatui_text(raw, cols, rows) -> Vec<Line>`

### Key Handlers (handlers/)

Key dispatch is split across focused modules:

- `handlers/normal.rs` - dashboard normal mode
- `handlers/view.rs` - embedded tmux view mode
- `handlers/dialog.rs` - project creation, help, delete
  confirms, rename
- `handlers/feature_creation.rs` - multi-step feature
  creation wizard
- `handlers/browse.rs` - path browser key handling
- `handlers/hooks.rs` - running hook, deleting feature,
  hook prompt handlers
- `handlers/picker.rs` - notification, session, command,
  opencode pickers
- `handlers/search.rs` - search mode
- `handlers/change_reason.rs` - diff review prompt
- `handlers/plan_interview.rs` - discovery and plan-review key handling
- `handlers/todos.rs` - scoped TODOs overlay (five layers) +
  quick-capture, spawn-target, and delete-disposition dispatch
- `handlers/learning.rs` - Learning Mode overlay key dispatch
  (layered: help → pickers → question prompt → answer pane)
- `handlers/prompt_overrides.rs` - override manager (layered: help →
  editor → scope picker → harness picker → list)
- `handlers/precall.rs` - pre-call notice (`v`/`e`/`Enter`/`Esc`)
- `handlers/dormant.rs` - dormant-features overlay key dispatch
- `handlers/review_destination.rs` - final-review destination picker,
  companion-feature setup, and integration-overlay key dispatch
- `handlers/mouse.rs` - mouse event handling

### Feature TODOs

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

### Learning Mode

A read-only code reader with an agent attached, for someone who did
not write the code in front of them. Built for a newcomer: nothing in
the mode mutates the repository, a blank prompt is never the only
option, and answers are pitched at a first-time reader by default. See
`docs/backlog/learning-mode-plan.md` for the full rationale.

- **Surface:** `AppMode::Learning(Box<LearningViewState>)`, opened with
  `K` on the dashboard (`open_learning_mode_for_selection` — a project
  row opens its first feature). It borrows the Final Review viewer's
  chrome, **not** its state machine, and creates **no `SessionKind`
  row**: Learning Mode is not a session and never appears in the tree or
  switcher.
- **Read-only invariant:** the only path out of it that can change files
  is escalation (`S`), which opens an ordinary agent session and says so
  in the seed. Keep it that way — relaxing it is a scope decision, not a
  convenience patch.
- **Browsing:** `BrowseScope::RepoTree` lists via
  `diff::list_repo_files` (`git ls-files`, with a capped plain walk for
  non-git projects); `BrowseScope::BranchChanges` uses
  `diff::load_snapshot`, the same call the diff viewer makes. In
  branch-changes scope `learning_load_selected_content` still hydrates
  the **whole file**, so an anchor keeps its surrounding context while
  the pane addresses diff rows. Repo-tree scope also pins a **Start
  here** orientation group (existence-checked well-known files plus a
  repo-level tour question) until the project has any history.
- **The repo-tree list is a tree, and `entries` is derived.** Repo-tree rows
  come from `flatten_tree` (pure: path list + `expanded_dirs` → rows,
  directories before files at each level); branch-changes stays flat. The
  authority on what is open is `LearningViewState::expanded_dirs`, **not** the
  `Dir` rows — every tree operation changes that set and rebuilds, so a cursor
  has to be restored by identity (`row_key`), never by index. Two constraints
  are load-bearing: `learning_rebuild_tree` works from the cached `repo_files`
  and must not re-read the repository (expanding a folder cannot cost a `git
  ls-files`), and `default_expanded_dirs` is seeded **once** per overlay
  (`expanded_seeded`) so a reload never re-opens what the user closed. A
  directory is navigation only — `LearningListEntry::path()` returns `None`
  for one, which is what keeps resting on a folder from moving the loaded file
  or the anchor. Size limits live per directory (`MAX_DIR_CHILDREN`, reported
  on the folder's own row); `MAX_REPO_ENTRIES` is now only a memory valve.
- **Anchors:** `LearningAnchor::{Project, File, Hunk, Lines}`; hunks
  exist only in branch-changes scope. The anchor is captured *with* the
  question (`AskAnchor`), not re-read at submit time, so a follow-up
  asked after browsing away still quotes its parent's code.
- **Anchor drift is derived, never stored.** `learning_check_anchor_drift`
  runs once per open (beside `reconcile_interrupted_qa`, which reconciles
  *runs* the same way this reconciles the *code*) and fills
  `LearningViewState::anchor_drift`, a side table keyed by row id.
  `check_anchor_drift` matches the row's `selection_text` against the file
  as it is now — trimmed, blank lines dropped, the stored position checked
  before the whole-file search, so a re-indent isn't movement and a copy
  made elsewhere doesn't unanchor the original. Dropping lines shifts where
  the evidence starts, so `ExpectedBlock::lead_offset` steps the stored
  position past them; without it a selection opening on a blank line reports
  as having slid down by its own whitespace. Two invariants: the row's
  `line_start`/`line_end` are **never rewritten** (they record where the
  question was asked, and keeping them is what lets the verdict be
  re-derived rather than believed once), and *no verdict* is the answer
  for everything there is no evidence to judge — an unreadable file, an
  empty selection, a `File` anchor whose file still exists. "Unreadable"
  includes a file that can't be stat'd at all: the `Gone` verdict is
  `ErrorKind::NotFound` specifically, not `Path::exists()`, which says the
  same "no" to a deleted file and to an unreadable parent directory. A
  diff-sourced selection (`selection_is_diff`) can be reported `Lost` but
  never `Reanchored`: its range comes from `new_line.or(old_line)`, so it
  is not a baseline to measure against. The verdict rides along into
  `escalation_seed` and `todo_body`, which would otherwise send an agent
  to read a location the code has left.
- **Two intents, one history.** `LearningQaIntent::{Explain, Action}` —
  `e` asks for a teaching answer, `c` for a change proposal. Intent only
  shapes prompt framing and affordance ordering, and is re-labelable
  afterwards (`i`) without rewriting the answer.
- **Level:** `LearningLevel::{Newcomer, Familiar}` is a per-session
  setting (`L`), not per-question. It changes prompt wording only — not
  tools, model, or visibility — and each row records the level it was
  answered at, so a reloaded answer explains why it reads the way it
  does.
- **Prompts** are pure functions: `build_prompt` over a
  `LearningPromptContext`, composed from `intent_instructions`,
  `level_instructions`, and `run_mode_instructions`. Run mode comes off
  the run that will *actually* be dispatched
  (`LearningRunMode::effective_for` downgrades every Codex request to
  `DeepDive`, since `codex exec` has no no-tools mode), so the label,
  the stored row, and the command always agree.
- **Execution:** `HeadlessRunner::run(..., restricted = true)` for the
  default answer, `run_read_only` for a deep dive (`D`). Runs are
  non-blocking and several may be in flight: a persistent `mpsc` channel
  on `App` plus a thread per run, drained by
  `poll_learning_answers_bg()` next to the other `poll_*_bg` calls in
  `main.rs`. An answer that lands after the overlay closed is still
  persisted (`finish_learning_qa_in_db`), and a row left `running` by a
  previous process is failed on load by `reconcile_interrupted_qa`
  rather than reloading as "thinking…" forever.
- **Threading has two distinct relationships.** `parent_qa_id` is a
  follow-up (`F`) — the parent's turn goes into the prompt.
  `deep_dive_of_qa_id` is a rerun (`D`) — the row it replaced is stored
  under it for reading side by side, but `learning_ancestor_turns` steps
  *over* it, so a follow-up on a verified answer never carries the
  shallow one's (possibly invented) evidence forward. Ordering goes
  through `thread_insert_index` for live inserts and `thread_rows` on
  reload, so there is one notion of order rather than two.
- **Acting on an answer:** `a` keeps it as a project TODO (via
  quick-capture's route, so the `SessionKind::Todos` session exists
  before the item does — a `todo_lists` row with no session is
  unreachable), `S` escalates to a live agent session
  (`create_agent_session_labeled` → `enter_view_without_auto_compose` →
  `open_compose_seeded`, editable and unsent). Both record their link on
  the row (`todo_id`, `spawned_session_id`) and a repeat press jumps to
  what exists rather than creating a second; a stale link is dropped and
  the replacement announced.
- **Persistence:** `learning_sessions` + `learning_qa`
  (`MIGRATION_019`, extended by `MIGRATION_020`'s `selection_is_diff`
  and `MIGRATION_021`'s `deep_dive_of_qa_id`), accessed via
  `src/db/learning.rs`. Kept out of the `ProjectStore` JSON like the
  todo tables, with `delete_learning_sessions_for_project` wired into
  project deletion. As with todos, the in-memory list is the overlay's
  source of truth and the mode works without a DB — it just says so
  rather than pretending history was kept.
- **Every refusal says why.** A missing key, a swallowed keypress, or a
  banner that describes a state the row isn't in are the failure modes
  this mode exists to avoid; new actions should state what happened and
  which key to press instead.

### Editable Headless Prompts

Every one-shot ("headless") AI call AMF makes runs a template from a central
registry that the user can view and override. See
`docs/backlog/editable-prompts-call-site-inventory.md` for the call-site map
and `AMF_PLAN.md` for the design decisions.

- **Registry (`src/prompts/`).** `PromptId::ALL` is the 15 stable ids
  (`plan_interview.round`/`.synthesis`/`.critique`/`.directed_revision`/
  `.investigation`/`.investigation_merge`, `learning.answer`,
  `review.walkthrough`/`.co_review`/`.changeset_overview`/`.diff_explain`,
  `pr_review.ai_review`, `review_memory.bootstrap`/`.compact`,
  `session.summary`). `defaults.rs` holds the built-in text. The 6
  plan-interview templates keep a single `{{interview_input}}` token carrying
  the exact JSON payload the models see today (the drift-guard test
  `plan_interview_defaults_stay_in_sync_with_the_tuned_prose` pins them to the
  `plan_interview::*_PROMPT` prose, which is duplicated because a `const`
  can't be `concat!`-ed); the other 9 use granular tokens.
- **Interpolation is unvalidated.** `render_template` substitutes `{{name}}`
  from a `PromptContext`; a token with no value — declared or not — is left
  literally, and substituted values are never re-scanned. An override may drop
  or add tokens freely.
- **Three override scopes.** Feature (keyed by workdir path) and global
  overrides live in `amf.db`'s `prompt_overrides` table (`MIGRATION_034`,
  `src/db/prompt_overrides.rs`: `OverrideScope::{Feature,Global}`, CRUD +
  `PromptOverrides` in-memory view with a no-DB fallback). Project overrides
  live in `amf.json` under `ExtensionConfig::prompt_overrides`
  (`HashMap<prompt_id, PromptOverrideEntry{ template?, harnesses }>`,
  `src/prompts/project.rs`) — **not** `.amf/prompts/`, because `.amf/` is
  gitignored dir-wide, and **not** merged from the global `extension` block.
  Harnesses are keyed by `AgentKind::slug()` (`claude`/`codex`/`opencode`/
  `pi`).
- **Precedence.** `resolve_template_layered` (`src/prompts/resolve.rs`):
  feature → project → global → built-in, nearest wins; within the winning
  layer a per-harness template beats the shared one, so a nearer *shared*
  override beats a farther *per-harness* one. Once any layer supplies an
  override the built-in default is never read (silent "default drift"). Call
  sites go through `App::resolve_headless_prompt` / `resolve_headless_template`
  (`app/mod.rs`), read fresh each call.
- **Manager overlay.** `AppMode::PromptOverrides(Box<PromptOverridesState>)`,
  `app/prompt_overrides.rs` / `handlers/prompt_overrides.rs` /
  `ui/dialogs/prompt_overrides.rs`. Dashboard `E` / leader `E`. List → edit
  the effective template → `Ctrl+S` → scope picker (Feature only with a
  feature + DB, Project only with a repo, always Global) → harness picker
  (shared / one) → save. `d`,`d` clears the effective override (every
  per-harness row at that scope).
- **Pre-call notice.** `AppMode::PromptPrecall(Box<PendingPrecall>)`,
  `app/precall.rs`. `precall_gate(action, harness, rendered)` is called by
  each **user-initiated** gated call site right before it spawns; it stashes
  the originating mode and returns `false` (caller returns without spawning).
  `precall_confirm` restores the mode, sets `precall_cleared`, and
  re-invokes the same `start_*` via `dispatch_precall` — the re-run
  re-resolves the prompt so an override saved from `e` applies. `precall_edit`
  opens the manager focused on the prompt (`App::precall_return` brings the
  notice back on close). `precall_cleared` is wiped after every
  `dispatch_precall` so a fall-through (e.g. a round with no harness →
  synthesis) can't leave a stale clearance. **Automated** runs
  (`learning.answer`, `session.summary`) call `announce_headless_run` — a
  toast, never the modal — so a queued batch can't deadlock.

### Agent Limits & Resource Health (resources/)

Guards against AMF quietly exhausting host memory. Everything here
is advisory: a tripped gate asks, it never refuses, and a missing
signal is always "no gate" rather than a block.

```text
resources/
├── mem.rs      # probe() -> Option<MemorySnapshot>: /proc/meminfo
│               # narrowed by cgroup v2 limits on Linux, sysctl +
│               # vm_stat on macOS, None everywhere else
├── limits.rs   # LiveHarnesses census + active_harness_sessions();
│               # HeadlessLease counts in-flight headless runs
├── procs.rs    # ps-backed process list/tree, pid liveness,
│               # SIGTERM-then-SIGKILL tree termination, VS Code
│               # window attribution
└── doctor.rs   # `amf doctor` checks + text/JSON rendering
                # (reads via AmfDb::open_read_only + setup::read_config:
                #  no file creation, no migration, no journal change)
```

- **Pre-start gate** (`app/resource_gate.rs`): `check_start_preconditions()`
  combines the agent count (harness sessions across all projects +
  headless leases, tripping at `active >= limit`) and available
  memory into one result. The gate lives in the **launch primitives**
  (`ensure_feature_running`, `ensure_feature_running_for_new_session`,
  `create_agent_session_labeled`, and the three
  `ensure_feature_running_with_*_session` pickers), not on entry points:
  each takes a `StartIntent` so a new caller must pick a policy.
   - `Approved` — an upstream gate already cleared this start.
   - `Ask(PendingStart)` — park in `AppMode::ConfirmResourceStart`,
     replayed by `confirm_pending_start()`. Used by the dashboard start,
     session adds, `enter_view`, and `switch_view_to_feature`.
   - `Warn(&str)` — toast and proceed, for flows whose resume state lives
     in the `AppMode` the dialog would replace (TODO spawn, PR triage,
     final review, saved-transcript pickers).

  The gate only fires when the call will actually launch, so re-entering a
  running feature never asks. Creation paths call `autostart_allowed()`
  instead, which warns and skips rather than prompting.
- **Headless accounting**: `HeadlessLease` is acquired inside
  `run_command` / `run_jsonl_command` (`headless.rs`), so every
  `HeadlessRunner` caller is counted; poll-driven runs hold a
  `LeasedChild` instead. Dropping one **kills** the run: it terminates the
  process tree and reaps it on a background thread, holding the lease until
  the process is really gone (`std::process::Child` only detaches on drop,
  which would leave an abandoned harness running but uncounted).
- **Editor tracking**: `launched_editors` (`MIGRATION_017`, plus
  `proc_started_at` in `MIGRATION_018`; `db/editors.rs`). VS Code launches
  with `--new-window` and is recorded **not-owned**; a background thread
  then attributes the new window process (new PID + worktree in argv) and
  only then marks it `dedicated`, storing the process's own start time.
  Reusing a running VS Code produces no new process, so that launch stays
  not-owned for good. `app/editor_ops.rs` revalidates identity before
  signalling — argv matched on **path boundaries** plus that start time, so
  a recycled PID never passes — and skips a window whose process is hosting
  more than one window (`procs::vscode_window_count`), because VS Code is a
  singleton and the others are the user's. It kills the process tree and
  returns a killed/skipped/pending report used by `do_stop_feature` and the
  dormant overlay.
- **Launch/stop race**: a stop during the seconds before attribution has
  nothing to kill, so `App::pending_editor_launches` hands the job over:
  `kill_tracked_editors` flips the launch's `PendingLaunchState` to
  `Reclaim` and the resolver closes the window it finds instead of recording
  it. The resolver holds the launch's mutex across deciding *and* writing,
  so either the stop claims it or the stop finds the row already owned.
- **Dormancy** (`app/dormant.rs`): idle (tmux `window_activity`) **and**
  unattended (`Feature::last_accessed`), both configurable; `z` opens
  `AppMode::Dormant`.

### Debug Logging

**NEVER use `println!` or `eprintln!` in TUI code** - it corrupts
the terminal display. Use the built-in debug log instead.

To view the debug log at runtime, press `D` from the dashboard.

**Log file location:** `~/.local/state/amf/debug.log`

You can tail this file in a separate terminal:
```bash
tail -f ~/.local/state/amf/debug.log
```

**Usage in code:**

```rust
// From anywhere with access to `app`:
app.log_debug("context", format!("value: {}", value));
app.log_info("context", "operation completed".to_string());
app.log_warn("context", "something unexpected".to_string());
app.log_error("context", format!("failed: {}", err));
```

**Log levels** (color-coded in UI):
- `DEBUG` (gray) - detailed tracing
- `INFO` (green) - normal operations
- `WARN` (yellow) - unexpected but handled
- `ERROR` (red) - failures

**Context strings** should be short identifiers like:
- `"amf"` - app lifecycle
- `"sync"` - status sync operations
- `"tmux"` - tmux interactions
- `"worktree"` - git worktree operations
- `"hooks"` - lifecycle hooks

Errors from `show_error()` are automatically logged to the
debug log with level ERROR.

### Key Design Patterns

- All external tool interaction (tmux, git, claude) goes
  through `std::process::Command` in dedicated manager
  structs
- Status sync polls tmux every 5 seconds to reconcile
  `ProjectStatus` with actual session state
- When running inside tmux, switching uses
  `switch-client`; outside tmux, the TUI exits and
  attaches via `should_switch` field
- First feature per project uses repo dir directly;
  subsequent features get git worktrees under
  `.worktrees/`
- ViewState embeds tmux pane content by capturing ANSI
  output and rendering through vt100 parser
- Leader key (Ctrl+Space) activates a 2-second chord
  window for view-mode commands
- **Never modify `~/.claude/settings.json` (global) or
  `~/.config/opencode/` (global opencode config) to inject
  hooks or settings.** Instead, write to the worktree's
  local `.claude/settings.local.json` (or `.opencode/` equivalent)
  via `ensure_notification_hooks()`. For non-worktree
  features (first feature that uses the repo dir directly),
  write to `{repo}/.claude/settings.local.json`. On startup,
  `cleanup_global_hooks()` actively removes any
  previously-injected global entries.

### Dependencies (Cargo.toml)

- ratatui 0.29, crossterm 0.28, vt100 0.15
- clap 4 (derive), serde 1, serde_json 1
- uuid 1 (v4), dirs 6, anyhow 1, chrono 0.4 (serde)

## Screenshot proof publication

Use `amf:screenshot` only for an explicit user request for visual proof. To
publish to an open PR, require separate explicit authorization and run
`scripts/dev/screenshot/publish-pages.sh --strict` only after the ref and
scenario are pushed. The command requires the `eldridgerdev` GitHub identity
and updates only the marked PR-body section. It dispatches the isolated capture
workflow (which alone checks out the ref), then downloads that run's rendered
frames and deploys the private Cloudflare Pages gallery **from the local
machine** — so it needs `CLOUDFLARE_API_TOKEN` in the environment (the owner
keeps it in `~/.secrets/cf-amf-pages.env`, sourced by the
`amf-publish-screenshots` shell wrapper; don't run `wrangler login`, it is
unreliable here), `CLOUDFLARE_ACCOUNT_ID` set, plus `wrangler` (or
`npx`). There is no `screenshot-pages`
environment and no per-run approval any more: the Pages credentials never enter
CI, and the capture-vs-deploy split (deploy never runs ref code) is what keeps
that safe. Never place raw ANSI/text captures or Actions artifact URLs in the
PR.
